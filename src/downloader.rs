use indicatif::ProgressBar;
use reqwest::Client;
use std::path::Path;
use tokio::fs as tfs;
use tokio::prelude::*;
use tokio::time::{timeout, Duration};

use crate::dlm_error::DlmError;
use crate::file_link::FileLink;
use crate::progress_bars::message_progress_bar;

pub async fn download_link(
    raw_link: &String,
    client: &Client,
    output_dir: &str,
    pb: &ProgressBar,
) -> Result<String, DlmError> {
    let file_link = FileLink::new(raw_link.clone())?;
    let final_name = &file_link.full_path(output_dir);
    if Path::new(final_name).exists() {
        let msg = format!("Skipping {} because it is already completed", final_name);
        Ok(msg)
    } else {
        let url = file_link.url.as_str();
        let head_result = client.head(url).send().await?;
        // get remote file size and range capabilities
        let content_length = head_result.content_length();
        let accept_ranges = head_result
            .headers()
            .get("Accept-Ranges")
            .and_then(|ct_len| ct_len.to_str().ok());

        if !head_result.status().is_success() {
            let message = format!("{} {}", url, head_result.status());
            Err(DlmError { message })
        } else {
            // Reset progress bar for next file
            pb.reset();
            let tmp_name = format!("{}/{}part", output_dir, file_link.file_name_no_extension);
            let query_range =
                compute_query_range(pb, content_length, accept_ranges, &tmp_name).await?;

            // create/open file.part
            let mut file = match query_range {
                Some(_) => {
                    tfs::OpenOptions::new()
                        .append(true)
                        .create(false)
                        .open(&tmp_name)
                        .await?
                }
                None => tfs::File::create(&tmp_name).await?,
            };

            // setup progress bar
            let total_size = content_length.unwrap();
            pb.set_length(total_size);
            pb.set_message(message_progress_bar(&file_link.file_name).as_str());
            let mut downloaded = pb.position();

            // building the request
            let mut request = client.get(url);
            if let Some(range) = query_range {
                request = request.header("Range", range)
            }

            // initiate file download
            let mut res = request.send().await?;
            // incremental save chunk by chunk into part file
            let chunk_timeout = Duration::from_secs(60);
            while let Some(chunk) = timeout(chunk_timeout, res.chunk()).await?? {
                file.write_all(&chunk).await?;
                let new_pb_position = downloaded + chunk.len() as u64;
                downloaded = new_pb_position;
                pb.set_position(new_pb_position);
            }

            // rename part file to final
            tfs::rename(&tmp_name, &final_name).await?;

            let msg = format!("Completed {}", final_name);
            Ok(msg)
        }
    }
}

async fn compute_query_range(
    pb: &ProgressBar,
    content_length: Option<u64>,
    accept_ranges: Option<&str>,
    tmp_name: &str,
) -> Result<Option<String>, DlmError> {
    if Path::new(&tmp_name).exists() {
        // get existing file size
        let tmp_size = tfs::File::open(&tmp_name).await?.metadata().await?.len();
        match (accept_ranges, content_length) {
            (Some("bytes"), Some(cl)) => {
                pb.set_position(tmp_size);
                let range_msg = format!("bytes={}-{}", tmp_size, cl);
                Ok(Some(range_msg))
            }
            _ => {
                let log = format!(
                    "Found part file for {} with size {} but it will be overridden because the server does not support querying a range of bytes",
                    tmp_name, tmp_size
                );
                pb.println(log);
                Ok(None)
            }
        }
    } else {
        Ok(None)
    }
}
