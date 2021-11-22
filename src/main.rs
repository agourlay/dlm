mod args;
mod dlm_error;
mod downloader;
mod file_link;
mod progress_bar_manager;

use crate::args::get_args;
use crate::dlm_error::DlmError;
use crate::downloader::download_link;
use crate::progress_bar_manager::ProgressBarManager;
use futures_retry::{FutureRetry, RetryPolicy};
use futures_util::stream::StreamExt;
use reqwest::Client;
use std::time::Duration;
use tokio::fs as tfs;
use tokio::io::AsyncBufReadExt;
use tokio_stream::wrappers::LinesStream;

#[tokio::main]
async fn main() -> Result<(), DlmError> {
    let (input_file, max_concurrent_downloads, output_dir) = get_args();
    let nb_of_lines = count_lines(&input_file).await?;
    let file = tfs::File::open(input_file).await?;
    let file_reader = tokio::io::BufReader::new(file);

    let client = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(0)
        .build()?;
    let od_ref = &output_dir;
    let c_ref = &client;

    let pbm = ProgressBarManager::init(max_concurrent_downloads, nb_of_lines as u64);
    let main_pb_ref = pbm.get_main_pb_ref();
    let rx_ref = pbm.get_rx_ref();
    let tx_ref = pbm.get_tx_ref();

    let stream = LinesStream::new(file_reader.lines());
    stream
        .for_each_concurrent(max_concurrent_downloads, |link_res| async move {
            let pb = rx_ref.recv().expect("claiming channel should not fail");
            let message = match link_res {
                Err(e) => format!("Error with links iterator {}", e),
                Ok(link) if link.trim().is_empty() => "Skipping empty line".to_string(),
                Ok(link) => {
                    let processed = FutureRetry::new(
                        || download_link(&link, c_ref, od_ref, &pb),
                        retry_on_connection_drop,
                    );
                    match processed.await {
                        Ok((info, _)) => info,
                        Err((e, _)) => format!("Error: {:?}", e),
                    }
                }
            };
            ProgressBarManager::logger(&pb, message);
            tx_ref.send(pb).expect("releasing channel should not fail");
            main_pb_ref.inc(1);
        })
        .await;

    pbm.finish_all();
    Ok(())
}

// TODO can we do this without loading the whole file in memory?
async fn count_lines(input_file: &str) -> Result<i32, DlmError> {
    let file = tfs::File::open(input_file).await?;
    let file_reader = tokio::io::BufReader::new(file);
    let stream = LinesStream::new(file_reader.lines());
    let line_nb = stream.fold(0, |acc, _| async move { acc + 1 }).await;
    Ok(line_nb)
}

fn retry_on_connection_drop(e: DlmError) -> RetryPolicy<DlmError> {
    match e {
        DlmError::ConnectionClosed
        | DlmError::ResponseBodyError
        | DlmError::DeadLineElapsedTimeout => RetryPolicy::WaitRetry(Duration::from_secs(10)),
        _ => RetryPolicy::ForwardError(e),
    }
}
