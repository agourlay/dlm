mod args;
mod client;
mod dlm_error;
mod downloader;
mod file_link;
mod headers;
mod progress_bar_manager;
mod retry;
mod stats;
mod user_agents;
mod utils;

use crate::DlmError::EmptyInputFile;
use crate::args::{Arguments, Input, get_args};
use crate::client::ClientConfig;
use crate::dlm_error::DlmError;
use crate::downloader::{DownloadContext, DownloadOutcome};
use crate::progress_bar_manager::ProgressBarManager;
use crate::retry::{retry_handler, retry_strategy, with_retries};
use crate::stats::RunStats;
use futures_util::stream::StreamExt;
use std::pin::Pin;
use tokio::io::AsyncBufReadExt;
use tokio::{fs as tfs, signal};
use tokio_stream::Stream;
use tokio_stream::wrappers::LinesStream;
use tokio_util::sync::CancellationToken;

// type alias for the URL stream
type LineStream = Pin<Box<dyn Stream<Item = Result<String, std::io::Error>> + Send>>;

#[tokio::main]
async fn main() {
    let result = main_result().await;
    std::process::exit(match result {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("{err}");
            1
        }
    });
}

async fn main_result() -> Result<(), DlmError> {
    // CLI args
    let Arguments {
        input,
        max_concurrent_downloads,
        output_dir,
        user_agent,
        proxy,
        retry,
        connection_timeout_secs,
        read_timeout_secs,
        insecure,
        no_color,
        headers,
        basic_auth,
    } = get_args()?;

    // Settled before anything is printed. `console` already turns colours off
    // for a non-terminal and honours NO_COLOR; this is the explicit override
    // for terminals that claim more than they can render.
    if no_color {
        console::set_colors_enabled(false);
    }

    // start the clock before any I/O so the reported duration matches what
    // wrapping the command in `time` would show
    let stats = RunStats::new();
    let stats = &stats;

    // setup interruption signal handler
    let token = CancellationToken::new();
    let signal_task_handler = spawn_signal_handler(token.clone());

    let nb_of_lines = match &input {
        Input::File(input_file) => count_non_empty_lines(input_file).await?,
        Input::Url(_) => 1,
    };
    if nb_of_lines == 0 {
        return Err(EmptyInputFile);
    }

    // setup progress bar manager
    let pbm = ProgressBarManager::init(max_concurrent_downloads, nb_of_lines).await;
    let pbm = &pbm;

    let stream = build_url_stream(input, pbm, max_concurrent_downloads, nb_of_lines).await?;

    let token = &token;
    let client_config = ClientConfig {
        user_agent: user_agent.as_ref(),
        proxy: proxy.as_deref(),
        connection_timeout_secs,
        read_timeout_secs,
        insecure,
        basic_auth: basic_auth.as_ref().map(|(u, p)| (u.as_str(), p.as_str())),
        headers: &headers,
    };
    let ctx = DownloadContext::new(&client_config, output_dir.as_path(), token, pbm, stats)?;
    let ctx = &ctx;

    process_downloads(
        stream,
        ctx,
        token,
        pbm,
        stats,
        retry,
        max_concurrent_downloads,
    )
    .await;

    // stop signal handling
    signal_task_handler.abort();
    let interrupted = token.is_cancelled();

    // An interrupted run gets its report too: the downloads that did finish
    // are worth accounting for, and so are the bytes already on disk for the
    // ones that will be resumed. Every in-flight download has released its
    // progress bar by now - `for_each_concurrent` only resolves once they all
    // returned - so collecting the bars cannot block here.
    pbm.finish_all().await?;
    pbm.print_report(&stats.summary_lines(interrupted));

    if interrupted {
        return Err(DlmError::ProgramInterrupted);
    }

    // A run where some links could not be downloaded is a failed run - callers
    // scripting around dlm should not have to parse the output to notice.
    let failed = stats.failed_count();
    if failed > 0 {
        return Err(DlmError::DownloadsFailed {
            failed,
            processed: stats.processed_count(),
        });
    }
    Ok(())
}

fn spawn_signal_handler(token: CancellationToken) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // first interrupt: graceful shutdown
        signal::ctrl_c()
            .await
            .expect("ctrl-c signal should not fail");
        token.cancel();
        // second interrupt: force exit
        signal::ctrl_c()
            .await
            .expect("ctrl-c signal should not fail");
        eprintln!("Received second interrupt signal - force exiting");
        std::process::exit(1);
    })
}

async fn build_url_stream(
    input: Input,
    pbm: &ProgressBarManager,
    max_concurrent_downloads: u32,
    nb_of_lines: u64,
) -> Result<LineStream, DlmError> {
    match input {
        Input::File(input_file) => {
            pbm.log_above_progress_bars(&format!(
                "Starting dlm with at most {max_concurrent_downloads} concurrent downloads"
            ));
            pbm.log_above_progress_bars(&format!(
                "Found {nb_of_lines} URLs in input file {input_file}"
            ));
            let file = tfs::File::open(input_file).await?;
            let file_reader = tokio::io::BufReader::new(file);
            Ok(Box::pin(LinesStream::new(file_reader.lines())))
        }
        Input::Url(url) => {
            pbm.log_above_progress_bars(&format!("Downloading single URL: {url}"));
            Ok(Box::pin(tokio_stream::once(Ok(url))))
        }
    }
}

async fn process_downloads(
    stream: LineStream,
    ctx: &DownloadContext<'_>,
    token: &CancellationToken,
    pbm: &ProgressBarManager,
    stats: &RunStats,
    retry: u32,
    max_concurrent_downloads: u32,
) {
    stream
        .take_until(token.cancelled())
        .for_each_concurrent(max_concurrent_downloads as usize, |link_res| async move {
            if token.is_cancelled() {
                return;
            }
            // Each arm logs at its own level - an outcome and a failure do not
            // read the same - and reports whether the link reached a verdict.
            let link_processed = match link_res {
                Err(e) => {
                    // a link that could not even be read cannot be downloaded
                    stats.record_failed();
                    pbm.error_above_progress_bars(&format!("Error with links iterator {e}"));
                    true
                }
                Ok(link) => {
                    if is_empty_line(&link) {
                        false
                    } else {
                        // claim a progress bar for the upcoming download
                        let dl_pb = pbm.claim_progress_bar().await;

                        // polite fixed-then-exponential retries for network errors,
                        // raced against ctrl-c so a download sleeping through its
                        // backoff (up to minutes) does not hold up the shutdown
                        let processed = tokio::select! {
                            () = token.cancelled() => Err(DlmError::ProgramInterrupted),
                            res = with_retries(
                                retry_strategy(retry),
                                || ctx.download_link(&link, &dl_pb),
                                |e: &DlmError| retry_handler(e, pbm, &link),
                            ) => res,
                        };

                        // reset & release progress bar
                        pbm.release_progress_bar(dl_pb).await;

                        match processed {
                            Ok(outcome) => {
                                match &outcome {
                                    DownloadOutcome::Completed(_) => stats.record_completed(),
                                    DownloadOutcome::Skipped(_) => stats.record_skipped(),
                                }
                                pbm.log_above_progress_bars(&outcome.into_message());
                                true
                            }
                            // an interrupted download is not a failure, the run
                            // is being torn down and already exits non-zero
                            Err(DlmError::ProgramInterrupted) => false,
                            Err(e) => {
                                stats.record_failed();
                                pbm.error_above_progress_bars(&format!("Error for {link}: {e}"));
                                true
                            }
                        }
                    }
                }
            };
            if link_processed {
                pbm.increment_global_progress();
            }
        })
        .await;
}

fn is_empty_line(line: &str) -> bool {
    let line = line.trim();
    line.is_empty() || line.starts_with('#')
}

async fn count_non_empty_lines(input_file: &str) -> Result<u64, DlmError> {
    let file = tfs::File::open(input_file).await?;
    let reader = tokio::io::BufReader::new(file);
    let mut lines = reader.lines();
    let mut count = 0;
    while let Some(line) = lines.next_line().await? {
        if !is_empty_line(&line) {
            count += 1;
        }
    }
    Ok(count)
}
