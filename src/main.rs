mod args;
mod client;
mod dlm_error;
mod downloader;
mod file_link;
mod headers;
mod progress_bar_manager;
mod retry;
mod user_agents;
mod utils;

use crate::DlmError::EmptyInputFile;
use crate::args::{Arguments, Input, get_args};
use crate::dlm_error::DlmError;
use crate::downloader::{ClientConfig, DownloadContext};
use crate::progress_bar_manager::ProgressBarManager;
use crate::retry::{retry_handler, retry_strategy};
use futures_util::stream::StreamExt;
use std::pin::Pin;
use tokio::io::AsyncBufReadExt;
use tokio::{fs as tfs, signal};
use tokio_retry::RetryIf;
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
        accept,
        accept_invalid_certs,
    } = get_args()?;

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
        accept_invalid_certs,
    };
    let ctx = DownloadContext::new(
        &client_config,
        output_dir.as_path(),
        token,
        pbm,
        accept.as_deref(),
    )?;
    let ctx = &ctx;

    process_downloads(stream, ctx, token, pbm, retry, max_concurrent_downloads).await;

    // stop signal handling
    signal_task_handler.abort();
    if token.is_cancelled() {
        Err(DlmError::ProgramInterrupted)
    } else {
        pbm.finish_all().await?;
        Ok(())
    }
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
    retry: u32,
    max_concurrent_downloads: u32,
) {
    stream
        .take_until(token.cancelled())
        .for_each_concurrent(max_concurrent_downloads as usize, |link_res| async move {
            if token.is_cancelled() {
                return;
            }
            let message = match link_res {
                Err(e) => Some(format!("Error with links iterator {e}")),
                Ok(link) => {
                    if is_empty_line(&link) {
                        None
                    } else {
                        // claim a progress bar for the upcoming download
                        let dl_pb = pbm.claim_progress_bar().await;

                        // exponential backoff retries for network errors
                        let retry_strategy = retry_strategy(retry);

                        let processed = RetryIf::spawn(
                            retry_strategy,
                            || ctx.download_link(&link, &dl_pb),
                            |e: &DlmError| retry_handler(e, pbm, &link),
                        )
                        .await;

                        // reset & release progress bar
                        pbm.release_progress_bar(dl_pb).await;

                        match processed {
                            Ok(info) => Some(info),
                            Err(DlmError::ProgramInterrupted) => None,
                            Err(e) => Some(format!("Error for {link}: {e}")),
                        }
                    }
                }
            };
            if let Some(message) = message {
                pbm.log_above_progress_bars(&message);
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
