use hyper::HeaderMap;
use indicatif::ProgressBar;
use reqwest::Client;
use std::path::Path;
use tokio::fs as tfs;
use tokio::io::AsyncWriteExt;
use tokio::time::{timeout, Duration};

use crate::dlm_error::DlmError;
use crate::file_link::FileLink;
use crate::ProgressBarManager;

pub async fn download_link(
    raw_link: &str,
    client: &Client,
    output_dir: &str,
    pb: &ProgressBar,
) -> Result<String, DlmError> {
    let file_link = FileLink::new(raw_link.to_string())?;
    let final_name = &file_link.full_path(output_dir);
    if Path::new(final_name).exists() {
        let final_file_size = tfs::File::open(&final_name).await?.metadata().await?.len();
        let msg = format!(
            "Skipping {} because the file is already completed [{}]",
            file_link.file_name,
            pretty_file_size(final_file_size)
        );
        Ok(msg)
    } else {
        let url = file_link.url.as_str();
        let head_result = client.head(url).send().await?;
        if !head_result.status().is_success() {
            let message = format!("{} {}", url, head_result.status());
            Err(DlmError::ResponseStatusNotSuccess { message })
        } else {
            let (content_length, accept_ranges) =
                try_hard_to_extract_headers(head_result.headers(), url, client).await?;
            // setup progress bar for the file
            pb.set_message(ProgressBarManager::message_progress_bar(
                &file_link.file_name,
            ));
            if let Some(total_size) = content_length {
                pb.set_length(total_size);
            };

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

            // building the request
            let mut request = client.get(url);
            if let Some(range) = query_range {
                request = request.header("Range", range)
            }

            // initiate file download
            let mut dl_response = request.send().await?;
            if !dl_response.status().is_success() {
                let message = format!("{} {}", url, dl_response.status());
                Err(DlmError::ResponseStatusNotSuccess { message })
            } else {
                // incremental save chunk by chunk into part file
                let chunk_timeout = Duration::from_secs(60);
                while let Some(chunk) = timeout(chunk_timeout, dl_response.chunk()).await?? {
                    file.write_all(&chunk).await?;
                    pb.inc(chunk.len() as u64);
                }
                let final_file_size = file.metadata().await?.len();
                // rename part file to final
                tfs::rename(&tmp_name, &final_name).await?;
                let msg = format!(
                    "Completed {} [{}]",
                    file_link.file_name,
                    pretty_file_size(final_file_size)
                );
                Ok(msg)
            }
        }
    }
}

const KILOBYTE: f64 = 1024.0;
const MEGABYTE: f64 = KILOBYTE * KILOBYTE;
const GIGABYTE: f64 = KILOBYTE * MEGABYTE;

fn pretty_file_size(len: u64) -> String {
    let float_len = len as f64;
    let (unit, value) = if float_len > GIGABYTE {
        ("GiB", float_len / GIGABYTE)
    } else if float_len > MEGABYTE {
        ("MiB", float_len / MEGABYTE)
    } else if float_len > KILOBYTE {
        ("KiB", float_len / KILOBYTE)
    } else {
        ("bytes", float_len)
    };
    format!("{:.2}{}", value, unit)
}

async fn try_hard_to_extract_headers(
    head_headers: &HeaderMap,
    url: &str,
    client: &Client,
) -> Result<(Option<u64>, Option<String>), DlmError> {
    let tuple = match content_length(head_headers) {
        Some(0) => {
            // if "content-length": "0" then it is likely the server does not support HEAD, let's try harder with a GET
            let get_result = client.get(url).send().await?;
            let get_headers = get_result.headers();
            (content_length(get_headers), accept_ranges(get_headers))
        }
        ct_option @ Some(_) => (ct_option, accept_ranges(head_headers)),
        _ => (None, None),
    };
    Ok(tuple)
}

fn content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get("content-length")
        .and_then(|ct_len| ct_len.to_str().ok())
        .and_then(|ct_len| ct_len.parse().ok())
}

fn accept_ranges(headers: &HeaderMap) -> Option<String> {
    headers
        .get("accept-ranges")
        .and_then(|ct_len| ct_len.to_str().ok())
        .map(|v| v.to_string())
}

async fn compute_query_range(
    pb: &ProgressBar,
    content_length: Option<u64>,
    accept_ranges: Option<String>,
    tmp_name: &str,
) -> Result<Option<String>, DlmError> {
    if Path::new(&tmp_name).exists() {
        // get existing file size
        let tmp_size = tfs::File::open(&tmp_name).await?.metadata().await?.len();
        match (accept_ranges, content_length) {
            (Some(range), Some(cl)) if range == "bytes" => {
                pb.set_position(tmp_size);
                let range_msg = format!("bytes={}-{}", tmp_size, cl);
                Ok(Some(range_msg))
            }
            _ => {
                let log = format!(
                    "Found part file {} with size {} but it will be overridden because the server does not support resuming the download (range bytes)",
                    tmp_name, tmp_size
                );
                ProgressBarManager::log_above_progress_bar(pb, log);
                Ok(None)
            }
        }
    } else {
        if accept_ranges.is_none() {
            let log = format!(
                "The download of file {} should not be interrupted because the server does not support resuming the download (range bytes)",
                tmp_name
            );
            ProgressBarManager::log_above_progress_bar(pb, log);
        };
        Ok(None)
    }
}

#[cfg(test)]
mod downloader_tests {
    use crate::downloader::*;

    #[test]
    fn pretty_file_size_gb() {
        let size: u64 = 1_200_000_000;
        assert_eq!(pretty_file_size(size), "1.12GiB");
    }

    #[test]
    fn pretty_file_size_mb() {
        let size: u64 = 1_200_000;
        assert_eq!(pretty_file_size(size), "1.14MiB");
    }

    #[test]
    fn pretty_file_size_kb() {
        let size: u64 = 1_200;
        assert_eq!(pretty_file_size(size), "1.17KiB");
    }
}
