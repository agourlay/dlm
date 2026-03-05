use indicatif::ProgressBar;
use reqwest::Client;
use reqwest::header::{
    ACCEPT, ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, HeaderMap, LOCATION, RANGE,
};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio::time::{Duration, timeout};
use tokio::{fs as tfs, select};
use tokio_util::sync::CancellationToken;

use crate::ProgressBarManager;
use crate::client::make_client;
use crate::dlm_error::DlmError;
use crate::file_link::FileLink;
use crate::user_agents::UserAgent;
use crate::utils::pretty_bytes_size;

const NO_EXTENSION: &str = "NO_EXTENSION_FOUND";

pub struct ClientConfig<'a> {
    pub user_agent: Option<&'a UserAgent>,
    pub proxy: Option<&'a str>,
    pub connection_timeout_secs: u32,
    pub accept_invalid_certs: bool,
}

pub struct DownloadContext<'a> {
    client: Client,
    client_no_redirect: Client,
    connection_timeout_secs: u32,
    output_dir: &'a str,
    token: &'a CancellationToken,
    pb_manager: &'a ProgressBarManager,
    accept_header: Option<&'a str>,
}

impl<'a> DownloadContext<'a> {
    pub fn new(
        client_config: &ClientConfig<'_>,
        output_dir: &'a str,
        token: &'a CancellationToken,
        pb_manager: &'a ProgressBarManager,
        accept_header: Option<&'a str>,
    ) -> Result<Self, DlmError> {
        let client = make_client(
            client_config.user_agent,
            client_config.proxy,
            true,
            client_config.connection_timeout_secs,
            client_config.accept_invalid_certs,
        )?;
        let client_no_redirect = make_client(
            client_config.user_agent,
            client_config.proxy,
            false,
            client_config.connection_timeout_secs,
            client_config.accept_invalid_certs,
        )?;
        Ok(Self {
            client,
            client_no_redirect,
            connection_timeout_secs: client_config.connection_timeout_secs,
            output_dir,
            token,
            pb_manager,
            accept_header,
        })
    }
}

pub async fn download_link(
    raw_link: &str,
    ctx: &DownloadContext<'_>,
    pb_dl: &ProgressBar,
) -> Result<String, DlmError> {
    // select between stop signal and download
    select! {
        () = ctx.token.cancelled() => Err(DlmError::ProgramInterrupted),
        dl = download(raw_link, ctx, pb_dl) => dl,
    }
}

async fn download(
    raw_link: &str,
    ctx: &DownloadContext<'_>,
    pb_dl: &ProgressBar,
) -> Result<String, DlmError> {
    let file_link = FileLink::new(raw_link)?;
    let (extension, filename_without_extension) = match file_link.extension {
        Some(ext) => (ext, file_link.filename_without_extension),
        None => {
            fetch_filename_extension(
                &file_link.url,
                &file_link.filename_without_extension,
                &ctx.client,
                &ctx.client_no_redirect,
                ctx.pb_manager,
            )
            .await?
        }
    };
    let filename_with_extension = format!("{filename_without_extension}.{extension}");
    let final_file_path = PathBuf::from(ctx.output_dir).join(&filename_with_extension);

    // skip completed download
    if final_file_path.exists() {
        let final_file_size = tfs::metadata(&final_file_path).await?.len();
        let msg = format!(
            "Skipping {} because the file is already completed [{}]",
            filename_with_extension,
            pretty_bytes_size(final_file_size)
        );
        return Ok(msg);
    }

    let url = file_link.url.as_str();
    let mut head_request = ctx.client.head(url);
    if let Some(accept) = ctx.accept_header {
        head_request = head_request.header(ACCEPT, accept);
    }

    // check existence with HEAD
    let head_result = head_request.send().await?;
    if !head_result.status().is_success() {
        let status_code = format!("{}", head_result.status());
        return Err(DlmError::ResponseStatusNotSuccess { status_code });
    }

    let (content_length, accept_ranges) =
        try_hard_to_extract_headers(head_result.headers(), url, &ctx.client).await?;
    drop(head_result);

    // setup progress bar for the file
    pb_dl.set_message(ProgressBarManager::message_progress_bar(
        &filename_with_extension,
    ));
    if let Some(total_size) = content_length {
        pb_dl.set_length(total_size);
    }

    let tmp_name = Path::new(ctx.output_dir).join(format!("{filename_with_extension}.part"));
    let query_range = compute_query_range(
        pb_dl,
        ctx.pb_manager,
        content_length,
        accept_ranges,
        &tmp_name,
    )
    .await?;

    // create/open file.part
    // no need for a BufWriter because the HTTP chunks are rather large
    let mut file = match &query_range {
        Some(_range) => {
            tfs::OpenOptions::new()
                .append(true)
                .create(false)
                .open(&tmp_name)
                .await?
        }
        None => tfs::File::create(&tmp_name).await?,
    };

    // building the request
    let mut request = ctx.client.get(url);
    if let Some(range) = query_range {
        request = request.header(RANGE, range);
    }

    if let Some(accept) = ctx.accept_header {
        request = request.header(ACCEPT, accept);
    }

    // initiate file download
    let mut dl_response = request.send().await?;
    if !dl_response.status().is_success() {
        let status_code = format!("{}", dl_response.status());
        return Err(DlmError::ResponseStatusNotSuccess { status_code });
    }

    // incremental save chunk by chunk into part file
    let chunk_timeout = Duration::from_secs(u64::from(ctx.connection_timeout_secs));
    while let Some(chunk) = timeout(chunk_timeout, dl_response.chunk()).await?? {
        file.write_all(&chunk).await?;
        pb_dl.inc(chunk.len() as u64);
    }
    file.flush().await?; // flush buffer → OS
    file.sync_all().await?; // sync OS → disk
    let final_file_size = file.metadata().await?.len();

    // check download complete
    if let Some(expected) = content_length
        && final_file_size != expected
    {
        let message =
            format!("Incomplete download content_length:{expected} vs file_size:{final_file_size}");
        return Err(DlmError::other(message));
    }

    // check if the destination already has a finished file
    if tfs::metadata(&final_file_path).await.is_ok() {
        let message = format!(
            "Can't finalize download because the file {} already exists",
            final_file_path.display()
        );
        return Err(DlmError::other(message));
    }

    // rename part file to final
    tfs::rename(&tmp_name, final_file_path).await?;
    let msg = format!(
        "Completed {} [{}]",
        filename_with_extension,
        pretty_bytes_size(final_file_size)
    );
    Ok(msg)
}

async fn try_hard_to_extract_headers(
    head_headers: &HeaderMap,
    url: &str,
    client: &Client,
) -> Result<(Option<u64>, Option<String>), DlmError> {
    let tuple = match content_length_value(head_headers) {
        Some(0) => {
            // if "content-length": "0" then it is likely the server does not support HEAD, let's try harder with a GET
            // Use Range: bytes=0-0 to minimize data transfer if supported
            let get_result = client.get(url).header(RANGE, "bytes=0-0").send().await?;
            // Extract headers before dropping the response to avoid buffering the body
            let cl = content_length_value(get_result.headers());
            let ar = accept_ranges_value(get_result.headers());
            drop(get_result);
            (cl, ar)
        }
        ct_option @ Some(_) => (ct_option, accept_ranges_value(head_headers)),
        _ => (None, None),
    };
    Ok(tuple)
}

fn content_length_value(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
}

fn accept_ranges_value(headers: &HeaderMap) -> Option<String> {
    headers
        .get(ACCEPT_RANGES)
        .and_then(|v| v.to_str().ok())
        .map(ToString::to_string)
}

fn content_disposition_value(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
}

fn location_value(headers: &HeaderMap) -> Option<&str> {
    headers.get(LOCATION).and_then(|v| v.to_str().ok())
}

async fn compute_query_range(
    pb_dl: &ProgressBar,
    pb_manager: &ProgressBarManager,
    content_length: Option<u64>,
    accept_ranges: Option<String>,
    tmp_name: &Path,
) -> Result<Option<String>, DlmError> {
    if tmp_name.exists() {
        // get existing file size
        let tmp_size = tfs::metadata(tmp_name).await?.len();
        match (accept_ranges, content_length) {
            (Some(range), Some(cl)) if range == "bytes" => {
                // set the progress bar to the current size
                pb_dl.set_position(tmp_size);
                // reset the elapsed time to avoid showing a really large speed
                pb_dl.reset_elapsed();
                let range_msg = format!("bytes={tmp_size}-{cl}");
                Ok(Some(range_msg))
            }
            _ => {
                let log = format!(
                    "Found part file {} with size {tmp_size} but it will be overridden because the server does not support resuming the download (range bytes)",
                    tmp_name.display()
                );
                pb_manager.log_above_progress_bars(&log);
                pb_dl.set_position(0);
                Ok(None)
            }
        }
    } else if accept_ranges.is_none() {
        let log = format!(
            "The download of file {} should not be interrupted because the server does not support resuming the download (range bytes)",
            tmp_name.display()
        );
        pb_manager.log_above_progress_bars(&log);
        Ok(None)
    } else {
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
    // try to get the file name from the HTTP headers
    match compute_filename_from_disposition_header(url, client).await? {
        Some(fh) => {
            let (ext, filename) = FileLink::extract_extension_from_filename(&fh);
            if let Some(e) = ext {
                Ok((e, filename))
            } else {
                let msg = format!(
                    "Could not determine file extension based on header {filename} for {url}"
                );
                pb_manager.log_above_progress_bars(&msg);
                Ok((
                    NO_EXTENSION.to_owned(),
                    filename_without_extension.to_string(),
                ))
            }
        }
        None => {
            // check if it is maybe a redirect
            match compute_filename_from_location_header(url, client_no_redirect).await? {
                None => {
                    let msg = format!("No extension found for {url}");
                    pb_manager.log_above_progress_bars(&msg);
                    Ok((
                        NO_EXTENSION.to_owned(),
                        filename_without_extension.to_string(),
                    ))
                }
                Some(fl) => match fl.extension {
                    Some(ext) => Ok((ext, fl.filename_without_extension)),
                    None => Ok((NO_EXTENSION.to_owned(), fl.filename_without_extension)),
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
    if head_result.status().is_success() {
        // https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Content-Disposition#as_a_response_header_for_the_main_body
        let content_disposition = content_disposition_value(head_result.headers());
        Ok(content_disposition.and_then(parse_filename_header))
    } else {
        let status_code = format!("{}", head_result.status());
        Err(DlmError::ResponseStatusNotSuccess { status_code })
    }
}

fn parse_filename_header(content_disposition: &str) -> Option<String> {
    // Try RFC 6266 filename*= (UTF-8 encoded) first, then fall back to filename=
    // e.g. filename*=UTF-8''my%20file.txt
    if let Some(star_value) = find_param(content_disposition, "filename*=") {
        // strip encoding prefix like "UTF-8''" or "utf-8''"
        if let Some((_, name)) = star_value.split_once("''") {
            let decoded = percent_decode_filename(name);
            if !decoded.is_empty() {
                return Some(decoded);
            }
        }
    }
    // Standard filename= parameter (quoted or unquoted)
    if let Some(value) = find_param(content_disposition, "filename=") {
        let unquoted = value
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(value);
        if !unquoted.is_empty() {
            return Some(unquoted.to_string());
        }
    }
    None
}

/// Extract the value of a named parameter from a header value.
/// Handles both `; param=value` and `; param="value"` forms.
fn find_param<'a>(header: &'a str, param: &str) -> Option<&'a str> {
    // Case-insensitive search for the parameter name
    let lower = header.to_ascii_lowercase();
    let param_lower = param.to_ascii_lowercase();
    let idx = lower.find(&param_lower)?;
    let value_start = idx + param.len();
    let rest = &header[value_start..];
    // Value ends at next `;` (or end of string), trimmed
    let value = rest.split(';').next()?.trim();
    if value.is_empty() { None } else { Some(value) }
}

/// Minimal percent-decoding for filename*= values
fn percent_decode_filename(input: &str) -> String {
    let mut result = Vec::new();
    let mut bytes = input.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let decoded = bytes.next().zip(bytes.next()).and_then(|(hi, lo)| {
                let hex = [hi, lo];
                let s = std::str::from_utf8(&hex).ok()?;
                u8::from_str_radix(s, 16).ok()
            });
            match decoded {
                Some(d) => result.push(d),
                None => result.push(b), // keep '%' as-is on malformed input
            }
        } else {
            result.push(b);
        }
    }
    String::from_utf8_lossy(&result).into_owned()
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
    fn parse_filename_header_quoted() {
        let header = "attachment; filename=\"code-stable-x64-1639562789.tar.gz\"";
        assert_eq!(
            parse_filename_header(header),
            Some("code-stable-x64-1639562789.tar.gz".to_owned())
        );
    }

    #[test]
    fn parse_filename_header_unquoted() {
        let header = "attachment; filename=report.pdf";
        assert_eq!(parse_filename_header(header), Some("report.pdf".to_owned()));
    }

    #[test]
    fn parse_filename_header_inline() {
        let header = "inline; filename=\"preview.png\"";
        assert_eq!(
            parse_filename_header(header),
            Some("preview.png".to_owned())
        );
    }

    #[test]
    fn parse_filename_header_star_utf8() {
        let header = "attachment; filename*=UTF-8''my%20file.txt";
        assert_eq!(
            parse_filename_header(header),
            Some("my file.txt".to_owned())
        );
    }

    #[test]
    fn parse_filename_header_star_takes_precedence() {
        let header = "attachment; filename=\"fallback.txt\"; filename*=UTF-8''preferred.txt";
        assert_eq!(
            parse_filename_header(header),
            Some("preferred.txt".to_owned())
        );
    }

    #[test]
    fn parse_filename_header_no_filename() {
        let header = "attachment";
        assert_eq!(parse_filename_header(header), None);
    }
}
