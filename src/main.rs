mod args;
mod dlm_error;
mod downloader;
mod file_link;
mod progress_bar_manager;

use crate::args::get_args;
use crate::dlm_error::DlmError;
use crate::downloader::download_link;
use crate::progress_bar_manager::ProgressBarManager;
use futures_util::stream::StreamExt;
use reqwest::Client;
use std::time::Duration;
use tokio::fs as tfs;
use tokio::io::AsyncBufReadExt;
use tokio_retry::strategy::{jitter, ExponentialBackoff};
use tokio_retry::RetryIf;
use tokio_stream::wrappers::LinesStream;

#[tokio::main]
async fn main() {
    let result = main_result().await;
    std::process::exit(match result {
        Ok(_) => 0,
        Err(err) => {
            eprintln!("{}", err);
            1
        }
    });
}

async fn main_result() -> Result<(), DlmError> {
    // CLI args
    let (input_file, max_concurrent_downloads, output_dir) = get_args()?;

    // setup HTTP client
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(0)
        .build()?;
    let c_ref = &client;
    let od_ref = &output_dir;

    // setup progress bar manager
    let nb_of_lines = count_non_empty_lines(&input_file).await?;
    let (rendering_handle, pbm) =
        ProgressBarManager::init(max_concurrent_downloads, nb_of_lines as u64).await;
    let pbm_ref = &pbm;

    // print startup info
    let msg_header = format!(
        "Starting dlm with at most {} concurrent downloads",
        max_concurrent_downloads
    );
    pbm.log_above_progress_bars(msg_header);
    let msg_count = format!("Found {} URLs in input file {}", nb_of_lines, input_file);
    pbm.log_above_progress_bars(msg_count);

    // start streaming lines from file
    let file = tfs::File::open(input_file).await?;
    let file_reader = tokio::io::BufReader::new(file);
    let line_stream = LinesStream::new(file_reader.lines());
    line_stream
        .for_each_concurrent(max_concurrent_downloads, |link_res| async move {
            let message = match link_res {
                Err(e) => format!("Error with links iterator {}", e),
                Ok(link) if link.trim().is_empty() => "Skipping empty line".to_string(),
                Ok(link) => {
                    // claim a progress bar for the upcoming download
                    let dl_pb = pbm_ref
                        .rx
                        .recv()
                        .await
                        .expect("claiming progress bar should not fail");

                    // exponential backoff retries for network errors
                    let retry_strategy = ExponentialBackoff::from_millis(1000)
                        .map(jitter) // add jitter to delays
                        .take(10); // limit to 10 retries

                    let processed = RetryIf::spawn(
                        retry_strategy,
                        || download_link(&link, c_ref, od_ref, &dl_pb),
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
                        Ok(info) => info,
                        Err(e) => format!("Unrecoverable error while processing {}: {}", link, e),
                    }
                }
            };
            pbm_ref.log_above_progress_bars(message);
            pbm_ref.increment_global_progress();
        })
        .await;

    // cleanup phase
    pbm_ref.finish_all().await?;
    rendering_handle.await?;
    Ok(())
}

async fn count_non_empty_lines(input_file: &str) -> Result<i32, DlmError> {
    let file = tfs::File::open(input_file).await?;
    let file_reader = tokio::io::BufReader::new(file);
    let stream = LinesStream::new(file_reader.lines());
    let line_nb = stream
        .fold(0, |acc, rl| async move {
            match rl {
                Ok(l) if !l.trim().is_empty() => acc + 1,
                _ => acc,
            }
        })
        .await;
    Ok(line_nb)
}

fn retry_handler(e: &DlmError, pbm: &ProgressBarManager, link: &str) -> bool {
    let should_retry = is_network_error(e);
    if should_retry {
        let msg = format!("Retrying {} after error {:?}", link, e);
        pbm.log_above_progress_bars(msg)
    }
    should_retry
}

fn is_network_error(e: &DlmError) -> bool {
    matches!(
        e,
        DlmError::ConnectionClosed | DlmError::ResponseBodyError | DlmError::DeadLineElapsedTimeout
    )
}
