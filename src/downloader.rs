use indicatif::ProgressBar;
use reqwest::Client;
use reqwest::header::{ACCEPT, HeaderMap, RANGE};
use std::path::Path;
use tokio::io::AsyncWriteExt;
use tokio::time::{Duration, timeout};
use tokio::{fs as tfs, select};
use tokio_util::sync::CancellationToken;

use crate::ProgressBarManager;
use crate::client::make_client;
use crate::dlm_error::DlmError;
use crate::file_link::FileLink;
use crate::headers::{
    content_disposition_value, content_length_value, content_range_total_size, location_value,
    supports_range_bytes,
};
use crate::user_agents::UserAgent;
use crate::utils::pretty_bytes_size;

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
    output_dir: &'a Path,
    token: &'a CancellationToken,
    pb_manager: &'a ProgressBarManager,
    accept_header: Option<&'a str>,
}

impl<'a> DownloadContext<'a> {
    pub fn new(
        client_config: &ClientConfig<'_>,
        output_dir: &'a Path,
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

    /// Extract download metadata (content-length, range support, disposition filename)
    /// via HEAD request, falling back to GET if the server returns 405.
    async fn extract_metadata(
        &self,
        url: &str,
    ) -> Result<(Option<u64>, bool, Option<String>), DlmError> {
        let mut head_request = self.client.head(url);
        if let Some(accept) = self.accept_header {
            head_request = head_request.header(ACCEPT, accept);
        }
        let head_result = head_request.send().await?;
        let head_status = head_result.status();

        if head_status == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            // Server does not support HEAD, fall back to GET with minimal range
            self.pb_manager.log_above_progress_bars(&format!(
                "HEAD returned 405 for {url}, falling back to GET for metadata"
            ));
            let get_result = self
                .client
                .get(url)
                .header(RANGE, "bytes=0-0")
                .send()
                .await?;
            if get_result.status().is_success() {
                // Content-Length will be 1 (for the single byte requested),
                // so extract the total size from Content-Range header instead
                let cl = content_range_total_size(get_result.headers())
                    .or_else(|| content_length_value(get_result.headers()));
                let ar = supports_range_bytes(get_result.headers());
                let df =
                    content_disposition_value(get_result.headers()).and_then(parse_filename_header);
                Ok((cl, ar, df))
            } else {
                let status_code = get_result.status().as_u16();
                Err(DlmError::ResponseStatusNotSuccess { status_code })
            }
        } else if !head_status.is_success() {
            let status_code = head_status.as_u16();
            Err(DlmError::ResponseStatusNotSuccess { status_code })
        } else {
            let (content_length, supports_range) = self
                .try_hard_to_extract_headers(head_result.headers(), url)
                .await?;
            let disposition_filename =
                content_disposition_value(head_result.headers()).and_then(parse_filename_header);
            Ok((content_length, supports_range, disposition_filename))
        }
    }

    pub async fn download_link(
        &self,
        raw_link: &str,
        pb_dl: &ProgressBar,
    ) -> Result<String, DlmError> {
        let file_link = FileLink::new(raw_link)?;

        // When the filename is fully known from the URL, skip the HEAD request if the file exists
        if file_link.extension.is_some() {
            let filename = file_link.filename();
            let final_file_path = self.output_dir.join(&filename);
            if final_file_path.exists() {
                let final_file_size = tfs::metadata(&final_file_path).await?.len();
                let msg = format!(
                    "Skipping {} because the file is already completed [{}]",
                    filename,
                    pretty_bytes_size(final_file_size)
                );
                return Ok(msg);
            }
        }

        // select between stop signal and download
        select! {
            () = self.token.cancelled() => Err(DlmError::ProgramInterrupted),
            dl = self.download(file_link, pb_dl) => dl,
        }
    }

    async fn download(
        &self,
        mut file_link: FileLink,
        pb_dl: &ProgressBar,
    ) -> Result<String, DlmError> {
        // extract metadata with a HEAD request, falling back to GET if needed
        let (content_length, supports_range, disposition_filename) =
            self.extract_metadata(&file_link.url).await?;

        // resolve filename and extension if not already known from the URL
        if file_link.extension.is_none() {
            self.resolve_filename(&mut file_link, disposition_filename)
                .await?;
        }

        let filename = file_link.filename();
        let output_dir = self.output_dir;
        let final_file_path = output_dir.join(&filename);

        // skip completed download (needed for the case where filename was resolved via headers)
        if final_file_path.exists() {
            let final_file_size = tfs::metadata(&final_file_path).await?.len();
            let msg = format!(
                "Skipping {} because the file is already completed [{}]",
                filename,
                pretty_bytes_size(final_file_size)
            );
            return Ok(msg);
        }

        // setup progress bar for the file
        pb_dl.set_message(ProgressBarManager::message_progress_bar(&filename));
        if let Some(total_size) = content_length {
            pb_dl.set_length(total_size);
        }

        let tmp_name = output_dir.join(format!("{filename}.part"));
        let query_range = compute_query_range(
            pb_dl,
            self.pb_manager,
            content_length,
            supports_range,
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
        let mut request = self.client.get(&file_link.url);
        if let Some(range) = query_range {
            request = request.header(RANGE, range);
        }

        if let Some(accept) = self.accept_header {
            request = request.header(ACCEPT, accept);
        }

        // initiate file download
        let mut dl_response = request.send().await?;
        if !dl_response.status().is_success() {
            let status_code = dl_response.status().as_u16();
            return Err(DlmError::ResponseStatusNotSuccess { status_code });
        }

        // incremental save chunk by chunk into part file
        let chunk_timeout = Duration::from_secs(u64::from(self.connection_timeout_secs));
        while let Some(chunk) = timeout(chunk_timeout, dl_response.chunk()).await?? {
            file.write_all(&chunk).await?;
            pb_dl.inc(chunk.len() as u64);
        }
        file.flush().await?; // flush buffer → OS
        file.sync_all().await?; // sync OS → disk
        let final_file_size = file.metadata().await?.len();

        // check download complete
        match content_length {
            Some(expected) if final_file_size != expected => {
                return Err(DlmError::IncompleteDownload {
                    expected,
                    actual: final_file_size,
                });
            }
            None => {
                self.pb_manager.log_above_progress_bars(&format!(
                    "No Content-Length available for {}, cannot verify download completeness",
                    filename
                ));
            }
            _ => {}
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
            filename,
            pretty_bytes_size(final_file_size)
        );
        Ok(msg)
    }

    /// Resolve filename when the URL does not contain the extension (e.g. redirect).
    /// Mutates the FileLink in place with the resolved extension and filename.
    async fn resolve_filename(
        &self,
        file_link: &mut FileLink,
        disposition_filename: Option<String>,
    ) -> Result<(), DlmError> {
        // try to get the file name from the Content-Disposition header
        if let Some(fh) = disposition_filename {
            let (ext, filename) = FileLink::extract_extension_from_filename(&fh);
            if ext.is_some() {
                file_link.extension = ext;
                file_link.filename_without_extension = filename;
                return Ok(());
            }
            let msg = format!(
                "Could not determine file extension based on header {filename} for {}",
                file_link.url
            );
            self.pb_manager.log_above_progress_bars(&msg);
            return Ok(());
        }

        // check if it is maybe a redirect
        match self
            .compute_filename_from_location_header(&file_link.url)
            .await?
        {
            None => {
                let msg = format!("No extension found for {}", file_link.url);
                self.pb_manager.log_above_progress_bars(&msg);
            }
            Some(fl) => {
                file_link.extension = fl.extension;
                file_link.filename_without_extension = fl.filename_without_extension;
            }
        }
        Ok(())
    }

    async fn compute_filename_from_location_header(
        &self,
        url: &str,
    ) -> Result<Option<FileLink>, DlmError> {
        let head_result = self.client_no_redirect.head(url).send().await?;
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

    /// Try harder to extract content-length and range support when HEAD returns content-length: 0.
    async fn try_hard_to_extract_headers(
        &self,
        head_headers: &HeaderMap,
        url: &str,
    ) -> Result<(Option<u64>, bool), DlmError> {
        let tuple = match content_length_value(head_headers) {
            Some(0) => {
                // if "content-length": "0" then it is likely the server does not support HEAD, let's try harder with a GET
                // Use Range: bytes=0-0 to minimize data transfer if supported
                let get_result = self
                    .client
                    .get(url)
                    .header(RANGE, "bytes=0-0")
                    .send()
                    .await?;
                if !get_result.status().is_success() {
                    let status = get_result.status();
                    self.pb_manager.log_above_progress_bars(&format!(
                        "GET fallback for metadata returned {status} for {url}, proceeding without content-length"
                    ));
                    (None, false)
                } else {
                    let cl = content_range_total_size(get_result.headers())
                        .or_else(|| content_length_value(get_result.headers()));
                    let ar = supports_range_bytes(get_result.headers());
                    (cl, ar)
                }
            }
            ct_option @ Some(_) => (ct_option, supports_range_bytes(head_headers)),
            _ => (None, false),
        };
        Ok(tuple)
    }
}

async fn compute_query_range(
    pb_dl: &ProgressBar,
    pb_manager: &ProgressBarManager,
    content_length: Option<u64>,
    supports_range: bool,
    tmp_name: &Path,
) -> Result<Option<String>, DlmError> {
    if tmp_name.exists() {
        // get existing file size
        let tmp_size = tfs::metadata(tmp_name).await?.len();
        match (supports_range, content_length) {
            (true, Some(cl)) => {
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
    } else if !supports_range {
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

fn parse_filename_header(content_disposition: &str) -> Option<String> {
    // Try RFC 6266 filename*= (UTF-8 encoded) first, then fall back to filename=
    // e.g. filename*=UTF-8''my%20file.txt
    if let Some(star_value) = find_param(content_disposition, "filename*=") {
        // strip encoding prefix like "UTF-8''" or "utf-8''"
        if let Some((_, name)) = star_value.split_once("''") {
            let decoded = percent_decode_filename(name);
            if !decoded.is_empty() {
                return sanitize_filename(&decoded);
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
            return sanitize_filename(unquoted);
        }
    }
    None
}

/// Strip path components to prevent directory traversal attacks.
/// A malicious server could send `Content-Disposition: attachment; filename="../../etc/evil"`.
fn sanitize_filename(name: &str) -> Option<String> {
    // Use Path to extract just the file name, stripping any directory components
    let file_name = Path::new(name).file_name()?.to_str()?;
    if file_name.is_empty() {
        None
    } else {
        Some(file_name.to_string())
    }
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
            match (bytes.next(), bytes.next()) {
                (Some(hi), Some(lo)) => {
                    let hex = [hi, lo];
                    match std::str::from_utf8(&hex)
                        .ok()
                        .and_then(|s| u8::from_str_radix(s, 16).ok())
                    {
                        Some(d) => result.push(d),
                        None => {
                            // invalid hex, preserve all three bytes
                            result.push(b);
                            result.push(hi);
                            result.push(lo);
                        }
                    }
                }
                (Some(hi), None) => {
                    // incomplete sequence, preserve both bytes
                    result.push(b);
                    result.push(hi);
                }
                _ => {
                    // trailing '%', preserve it
                    result.push(b);
                }
            }
        } else {
            result.push(b);
        }
    }
    String::from_utf8_lossy(&result).into_owned()
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

    #[test]
    fn parse_filename_header_path_traversal() {
        let header = "attachment; filename=\"../../../etc/passwd\"";
        assert_eq!(parse_filename_header(header), Some("passwd".to_owned()));
    }

    #[test]
    fn parse_filename_header_path_traversal_star() {
        let header = "attachment; filename*=UTF-8''..%2F..%2Fevil.txt";
        assert_eq!(parse_filename_header(header), Some("evil.txt".to_owned()));
    }

    #[test]
    fn parse_filename_header_absolute_path() {
        let header = "attachment; filename=\"/tmp/evil.sh\"";
        assert_eq!(parse_filename_header(header), Some("evil.sh".to_owned()));
    }

    #[test]
    fn percent_decode_valid() {
        assert_eq!(percent_decode_filename("my%20file.txt"), "my file.txt");
    }

    #[test]
    fn percent_decode_trailing_percent() {
        assert_eq!(percent_decode_filename("file%"), "file%");
    }

    #[test]
    fn percent_decode_incomplete_sequence() {
        assert_eq!(percent_decode_filename("file%2"), "file%2");
    }

    #[test]
    fn percent_decode_invalid_hex() {
        assert_eq!(percent_decode_filename("file%GG"), "file%GG");
    }
}
