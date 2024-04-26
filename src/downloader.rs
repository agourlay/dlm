use indicatif::ProgressBar;
use reqwest::header::HeaderMap;
use reqwest::Client;
use std::path::Path;
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;
use tokio::time::{timeout, Duration};
use tokio::{fs as tfs, select};

use crate::dlm_error::DlmError;
use crate::file_link::FileLink;
use crate::utils::pretty_bytes_size;
use crate::ProgressBarManager;

const NO_EXTENSION: &str = "NO_EXTENSION_FOUND";

// TODO consider using a dedicated struct for the download link function
#[allow(clippy::too_many_arguments)]
pub async fn download_link(
    raw_link: &str,
    client: &Client,
    client_no_redirect: &Client,
    connection_timeout_secs: usize,
    output_dir: &str,
    broadcast_handle: &broadcast::Sender<()>,
    pb_dl: &ProgressBar,
    pb_manager: &ProgressBarManager,
) -> Result<String, DlmError> {
    // TODO extract downloader in dedicated task with own receiver because the signal could have been sent before the subscription
    // generate new subscription to stop signal
    let mut stop_receiver = broadcast_handle.subscribe();

    let file_link = FileLink::new(raw_link.to_string())?;
    let (extension, filename_without_extension) = match file_link.extension {
        Some(ext) => (ext, file_link.filename_without_extension),
        None => {
            fetch_filename_extension(
                &file_link.url,
                &file_link.filename_without_extension,
                client,
                client_no_redirect,
                pb_manager,
            )
            .await?
        }
    };
    let filename_with_extension = format!("{}.{}", filename_without_extension, extension);
    let final_file_path = &format!("{}/{}", output_dir, filename_with_extension);
    if Path::new(final_file_path).exists() {
        let final_file_size = tfs::File::open(final_file_path)
            .await?
            .metadata()
            .await?
            .len();
        let msg = format!(
            "Skipping {} because the file is already completed [{}]",
            filename_with_extension,
            pretty_bytes_size(final_file_size)
        );
        Ok(msg)
    } else {
        let url = file_link.url.as_str();
        let head_result = client.head(url).send().await?;
        if !head_result.status().is_success() {
            let status_code = format!("{}", head_result.status());
            Err(DlmError::ResponseStatusNotSuccess { status_code })
        } else {
            let (content_length, accept_ranges) =
                try_hard_to_extract_headers(head_result.headers(), url, client).await?;
            // setup progress bar for the file
            pb_dl.set_message(ProgressBarManager::message_progress_bar(
                &filename_with_extension,
            ));
            if let Some(total_size) = content_length {
                pb_dl.set_length(total_size);
            };

            let tmp_name = format!("{}/{}.part", output_dir, filename_without_extension);
            let query_range =
                compute_query_range(pb_dl, pb_manager, content_length, accept_ranges, &tmp_name)
                    .await?;

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
                let status_code = format!("{}", head_result.status());
                Err(DlmError::ResponseStatusNotSuccess { status_code })
            } else {
                // incremental save chunk by chunk into part file
                let chunk_timeout = Duration::from_secs(connection_timeout_secs as u64);
                loop {
                    // select between stop signal and chunk download
                    select! {
                        Ok(_) = stop_receiver.recv() => {
                            file.flush().await?;
                            return Err(DlmError::ProgramInterrupted);
                        }
                        chunk = timeout(chunk_timeout, dl_response.chunk()) => {
                            // unpack chunk
                            let chunk = chunk??;
                            if let Some(chunk) = chunk {
                                file.write_all(&chunk).await?;
                                file.flush().await?;
                                pb_dl.inc(chunk.len() as u64);
                            } else {
                                // end of download
                                // final flush
                                file.flush().await?;
                                break;
                            }
                        }
                    }
                }
                let final_file_size = file.metadata().await?.len();
                // rename part file to final
                tfs::rename(&tmp_name, final_file_path).await?;
                let msg = format!(
                    "Completed {} [{}]",
                    filename_with_extension,
                    pretty_bytes_size(final_file_size)
                );
                Ok(msg)
            }
        }
    }
}

async fn try_hard_to_extract_headers(
    head_headers: &HeaderMap,
    url: &str,
    client: &Client,
) -> Result<(Option<u64>, Option<String>), DlmError> {
    let tuple = match content_length_value(head_headers) {
        Some(0) => {
            // if "content-length": "0" then it is likely the server does not support HEAD, let's try harder with a GET
            let get_result = client.get(url).send().await?;
            let get_headers = get_result.headers();
            (
                content_length_value(get_headers),
                accept_ranges_value(get_headers),
            )
        }
        ct_option @ Some(_) => (ct_option, accept_ranges_value(head_headers)),
        _ => (None, None),
    };
    Ok(tuple)
}

fn content_length_value(headers: &HeaderMap) -> Option<u64> {
    headers
        .get("content-length")
        .and_then(|ct_len| ct_len.to_str().ok())
        .and_then(|ct_len| ct_len.parse().ok())
}

fn accept_ranges_value(headers: &HeaderMap) -> Option<String> {
    headers
        .get("accept-ranges")
        .and_then(|ct_len| ct_len.to_str().ok())
        .map(|v| v.to_string())
}

fn content_disposition_value(headers: &HeaderMap) -> Option<String> {
    headers
        .get("content-disposition")
        .and_then(|ct_len| ct_len.to_str().ok())
        .map(|v| v.to_string())
}

fn location_value(headers: &HeaderMap) -> Option<String> {
    headers
        .get("location")
        .and_then(|ct_len| ct_len.to_str().ok())
        .map(|v| v.to_string())
}

async fn compute_query_range(
    pb_dl: &ProgressBar,
    pb_manager: &ProgressBarManager,
    content_length: Option<u64>,
    accept_ranges: Option<String>,
    tmp_name: &str,
) -> Result<Option<String>, DlmError> {
    if Path::new(&tmp_name).exists() {
        // get existing file size
        let tmp_size = tfs::File::open(&tmp_name).await?.metadata().await?.len();
        match (accept_ranges, content_length) {
            (Some(range), Some(cl)) if range == "bytes" => {
                pb_dl.set_position(tmp_size);
                let range_msg = format!("bytes={}-{}", tmp_size, cl);
                Ok(Some(range_msg))
            }
            _ => {
                let log = format!(
                    "Found part file {} with size {} but it will be overridden because the server does not support resuming the download (range bytes)",
                    tmp_name, tmp_size
                );
                pb_manager.log_above_progress_bars(log);
                pb_dl.set_position(0);
                Ok(None)
            }
        }
    } else {
        if accept_ranges.is_none() {
            let log = format!(
                "The download of file {} should not be interrupted because the server does not support resuming the download (range bytes)",
                tmp_name
            );
            pb_manager.log_above_progress_bars(log);
        };
        Ok(None)
    }
}

// necessary when the URL does not contain clearly the filename (in case of a redirect for instance)
async fn fetch_filename_extension(
    url: &str,
    filename_without_extension: &str,
    client: &Client,
    client_no_redirect: &Client,
    pb_manager: &ProgressBarManager,
) -> Result<(String, String), DlmError> {
    // try get the file name from the HTTP headers
    match compute_filename_from_disposition_header(url, client).await? {
        Some(fh) => {
            let (ext, filename) = FileLink::extract_extension_from_filename(fh);
            match ext {
                Some(e) => Ok((e, filename)),
                None => {
                    let msg = format!(
                        "Could not determine file extension based on header {} for {}",
                        filename, url
                    );
                    pb_manager.log_above_progress_bars(msg);
                    Ok((
                        NO_EXTENSION.to_owned(),
                        filename_without_extension.to_string(),
                    ))
                }
            }
        }
        None => {
            // check if it is maybe a redirect
            match compute_filename_from_location_header(url, client_no_redirect).await? {
                None => {
                    let msg = format!(
                        "Using placeholder file extension as it could not be determined for {}",
                        url
                    );
                    pb_manager.log_above_progress_bars(msg);
                    Ok((
                        NO_EXTENSION.to_owned(),
                        filename_without_extension.to_string(),
                    ))
                }
                Some(fl) => match fl.extension {
                    Some(ext) => Ok((ext, fl.filename_without_extension)),
                    None => Ok((
                        NO_EXTENSION.to_owned(),
                        fl.filename_without_extension.to_string(),
                    )),
                },
            }
        }
    }
}

async fn compute_filename_from_disposition_header(
    url: &str,
    client: &Client,
) -> Result<Option<String>, DlmError> {
    let head_result = client.head(url).send().await?;
    if !head_result.status().is_success() {
        let status_code = format!("{}", head_result.status());
        Err(DlmError::ResponseStatusNotSuccess { status_code })
    } else {
        // https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Content-Disposition#as_a_response_header_for_the_main_body
        let content_disposition = content_disposition_value(head_result.headers());
        Ok(content_disposition.and_then(parse_filename_header))
    }
}

fn parse_filename_header(content_disposition: String) -> Option<String> {
    content_disposition
        .split("attachment; filename=")
        .last()
        .and_then(|s| s.strip_prefix('"'))
        .and_then(|s| s.strip_suffix('"'))
        .map(|s| s.to_string())
}

async fn compute_filename_from_location_header(
    url: &str,
    client_no_redirect: &Client,
) -> Result<Option<FileLink>, DlmError> {
    let head_result = client_no_redirect.head(url).send().await?;
    if head_result.status().is_redirection() {
        // https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Location
        match location_value(head_result.headers()) {
            None => Ok(None),
            Some(location) => {
                let fl = FileLink::new(location)?;
                Ok(Some(fl))
            }
        }
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod downloader_tests {
    use crate::downloader::*;

    #[test]
    fn parse_filename_header_ok() {
        let header_value = "attachment; filename=\"code-stable-x64-1639562789.tar.gz\"";
        let parsed = parse_filename_header(header_value.to_string());
        assert_eq!(parsed, Some("code-stable-x64-1639562789.tar.gz".to_owned()));
    }
}
