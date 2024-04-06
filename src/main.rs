mod args;
mod client;
mod dlm_error;
mod downloader;
mod file_link;
mod progress_bar_manager;
mod retry;
mod user_agents;
mod utils;

use crate::args::{get_args, Arguments};
use crate::client::make_client;
use crate::dlm_error::DlmError;
use crate::downloader::download_link;
use crate::progress_bar_manager::ProgressBarManager;
use crate::retry::{retry_handler, retry_strategy};
use crate::user_agents::{random_user_agent, UserAgent};
use crate::DlmError::EmptyInputFile;
use futures_util::stream::StreamExt;
use tokio::fs as tfs;
use tokio::io::AsyncBufReadExt;
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
    let Arguments {
        input_file,
        max_concurrent_downloads,
        output_dir,
        user_agent,
        proxy,
        retry,
        connection_timeout_secs,
    } = get_args()?;

    let nb_of_lines = count_non_empty_lines(&input_file).await?;
    if nb_of_lines == 0 {
        return Err(EmptyInputFile);
    }

    // setup HTTP clients
    let client = make_client(&user_agent, &proxy, true, connection_timeout_secs)?;
    let c_ref = &client;
    let client_no_redirect = make_client(&user_agent, &proxy, false, connection_timeout_secs)?;
    let c_no_redirect_ref = &client_no_redirect;
    // trim trailing slash if any
    let od_ref = &output_dir
        .strip_suffix('/')
        .unwrap_or(&output_dir)
        .to_string();

    // setup progress bar manager
    let pbm = ProgressBarManager::init(max_concurrent_downloads, nb_of_lines as u64).await;
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
                                &dl_pb,
                                pbm_ref,
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
