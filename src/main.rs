mod args;
mod client;
mod dlm_error;
mod downloader;
mod file_link;
mod progress_bar_manager;
mod retry;
mod user_agents;
mod utils;

use crate::DlmError::EmptyInputFile;
use crate::args::{Arguments, Input, get_args};
use crate::client::make_client;
use crate::dlm_error::DlmError;
use crate::downloader::download_link;
use crate::progress_bar_manager::ProgressBarManager;
use crate::retry::{retry_handler, retry_strategy};
use crate::user_agents::{UserAgent, random_user_agent};
use futures_util::stream::StreamExt;
use std::pin::Pin;
use tokio::io::AsyncBufReadExt;
use tokio::{fs as tfs, signal};
use tokio_retry::RetryIf;
use tokio_stream::Stream;
use tokio_stream::wrappers::LinesStream;
use tokio_util::sync::CancellationToken;

// Unification type
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
    let token_signal = token.clone();
    let signal_task_handler = tokio::spawn(async move {
        let mut counter = 0;
        // catch chain of interrupt signals
        loop {
            signal::ctrl_c()
                .await
                .expect("ctrl-c signal should not fail");
            token_signal.cancel();
            counter += 1;
            if counter > 1 {
                eprintln!("Received multiple interrupt signals - something is stuck");
            }
        }
    });

    // setup HTTP clients
    let client = make_client(
        user_agent.as_ref(),
        proxy.as_ref(),
        true,
        connection_timeout_secs,
        accept_invalid_certs,
    )?;
    let c_ref = &client;
    let client_no_redirect = make_client(
        user_agent.as_ref(),
        proxy.as_ref(),
        false,
        connection_timeout_secs,
        accept_invalid_certs,
    )?;
    let c_no_redirect_ref = &client_no_redirect;
    let accept_ref = accept.as_ref();
    // trim trailing slash if any
    let od_ref = &output_dir
        .strip_suffix('/')
        .unwrap_or(&output_dir)
        .to_string();

    let nb_of_lines = match &input {
        Input::File(input_file) => count_non_empty_lines(input_file).await?,
        Input::Url(_) => 1,
    };
    if nb_of_lines == 0 {
        return Err(EmptyInputFile);
    }

    // setup progress bar manager
    let pbm = ProgressBarManager::init(max_concurrent_downloads, nb_of_lines).await;
    let pbm_ref = &pbm;

    let stream: LineStream = match input {
        Input::File(input_file) => {
            // print startup info
            pbm.log_above_progress_bars(&format!(
                "Starting dlm with at most {max_concurrent_downloads} concurrent downloads"
            ));
            pbm.log_above_progress_bars(&format!(
                "Found {nb_of_lines} URLs in input file {input_file}"
            ));

            // start streaming lines from file
            let file = tfs::File::open(input_file).await?;
            let file_reader = tokio::io::BufReader::new(file);
            Box::pin(LinesStream::new(file_reader.lines()))
        }
        Input::Url(url) => {
            // print startup info
            pbm.log_above_progress_bars(&format!("Downloading single URL: {url}"));
            // fake single element stream
            Box::pin(tokio_stream::once(Ok(url)))
        }
    };

    let token_clone = &token.clone();
    stream
        .take_until(token_clone.cancelled()) // stop stream on signal
        .for_each_concurrent(max_concurrent_downloads as usize, |link_res| async move {
            // do not start new downloads if the program is stopped
            if token_clone.is_cancelled() {
                return;
            }
            let message = match link_res {
                Err(e) => Some(format!("Error with links iterator {e}")),
                Ok(link) if link.trim().is_empty() => Some("Skipping empty line".to_string()),
                Ok(link) => {
                    // claim a progress bar for the upcoming download
                    let dl_pb = pbm_ref
                        .rx
                        .recv()
                        .await
                        .expect("claiming progress bar should not fail");

                    // exponential backoff retries for network errors
                    let retry_strategy = retry_strategy(retry);

                    let processed = RetryIf::spawn(
                        retry_strategy,
                        || {
                            download_link(
                                &link,
                                c_ref,
                                c_no_redirect_ref,
                                connection_timeout_secs,
                                od_ref,
                                token_clone,
                                &dl_pb,
                                pbm_ref,
                                accept_ref,
                            )
                        },
                        |e: &DlmError| retry_handler(e, pbm_ref, &link),
                    )
                    .await;

                    // reset & release progress bar
                    ProgressBarManager::reset_progress_bar(&dl_pb);
                    pbm_ref
                        .tx
                        .send(dl_pb)
                        .await
                        .expect("releasing progress bar should not fail");

                    // extract result
                    match processed {
                        Ok(info) => Some(info),
                        Err(DlmError::ProgramInterrupted) => None, // no logs on interrupt
                        Err(e) => Some(format!("Unrecoverable error while processing {link}: {e}")),
                    }
                }
            };
            if let Some(message) = message {
                pbm_ref.log_above_progress_bars(&message);
                pbm_ref.increment_global_progress();
            }
        })
        .await;

    // stop signal handling
    signal_task_handler.abort();
    if token.is_cancelled() {
        Err(DlmError::ProgramInterrupted)
    } else {
        // cleanup phase
        pbm_ref.finish_all().await?;
        Ok(())
    }
}

async fn count_non_empty_lines(input_file: &str) -> Result<u64, DlmError> {
    let file = tfs::File::open(input_file).await?;
    let reader = tokio::io::BufReader::new(file);
    let mut lines = reader.lines();
    let mut count = 0;
    while let Some(line) = lines.next_line().await? {
        if !line.trim().is_empty() {
            count += 1;
        }
    }
    Ok(count)
}
