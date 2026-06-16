use indicatif::ProgressBar;
use reqwest::Client;
use reqwest::header::RANGE;
use std::cmp::Ordering;
use std::path::Path;
use tokio::io::AsyncWriteExt;
use tokio::time::{Duration, timeout};
use tokio::{fs as tfs, select};
use tokio_util::sync::CancellationToken;

use crate::ProgressBarManager;
use crate::client::{ClientConfig, make_client};
use crate::dlm_error::DlmError;
use crate::file_link::FileLink;
use crate::headers::{
    content_disposition_value, content_length_value, location_value, parse_filename_header,
    parse_metadata_from, supports_range_bytes,
};
use crate::utils::pretty_bytes_size;

pub struct DownloadContext<'a> {
    client: Client,
    client_no_redirect: Client,
    connection_timeout_secs: u32,
    output_dir: &'a Path,
    token: &'a CancellationToken,
    pb_manager: &'a ProgressBarManager,
}

impl<'a> DownloadContext<'a> {
    pub fn new(
        client_config: &ClientConfig<'_>,
        output_dir: &'a Path,
        token: &'a CancellationToken,
        pb_manager: &'a ProgressBarManager,
    ) -> Result<Self, DlmError> {
        Ok(Self {
            client: make_client(client_config, true)?,
            client_no_redirect: make_client(client_config, false)?,
            connection_timeout_secs: client_config.connection_timeout_secs,
            output_dir,
            token,
            pb_manager,
        })
    }

    /// Extract download metadata (content-length, range support, disposition filename).
    ///
    /// HEAD first. The disposition filename always comes from HEAD when HEAD
    /// succeeded; otherwise from the ranged-GET probe.
    async fn extract_metadata(
        &self,
        url: &str,
    ) -> Result<(Option<u64>, bool, Option<String>), DlmError> {
        let head = self.client.head(url).send().await?;
        let head_status = head.status();

        // HEAD outright rejected → derive the whole triple from a ranged GET.
        if head_status == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            self.pb_manager.log_above_progress_bars(&format!(
                "HEAD returned 405 for {url}, falling back to GET for metadata"
            ));
            return self.metadata_from_probe(url).await;
        }

        if !head_status.is_success() {
            return Err(DlmError::ResponseStatusNotSuccess {
                status_code: head_status.as_u16(),
            });
        }

        // HEAD succeeded — the disposition filename is taken from it.
        let disposition = content_disposition_value(head.headers()).and_then(parse_filename_header);

        // For length + range support: probe with a ranged GET when HEAD claims
        // `Content-Length: 0` (a sign HEAD is faked); trust HEAD when it
        // reports a real length; give up when no header is present.
        let (length, supports_range) = match content_length_value(head.headers()) {
            Some(0) => self.length_and_range_from_probe(url).await?,
            Some(n) => (Some(n), supports_range_bytes(head.headers())),
            None => (None, false),
        };

        Ok((length, supports_range, disposition))
    }

    /// Full-triple fallback used when HEAD is outright rejected (405).
    /// A failed probe is fatal here because we have no other source of metadata.
    async fn metadata_from_probe(
        &self,
        url: &str,
    ) -> Result<(Option<u64>, bool, Option<String>), DlmError> {
        let probe = self.range_probe(url).await?;
        if !probe.status().is_success() {
            return Err(DlmError::ResponseStatusNotSuccess {
                status_code: probe.status().as_u16(),
            });
        }
        Ok(parse_metadata_from(probe.headers()))
    }

    /// Length + range-support fallback used when HEAD succeeded but reported
    /// `Content-Length: 0`. A failed probe is recoverable: log it and give up
    /// on length/range, keeping the disposition filename from HEAD.
    async fn length_and_range_from_probe(
        &self,
        url: &str,
    ) -> Result<(Option<u64>, bool), DlmError> {
        let probe = self.range_probe(url).await?;
        if !probe.status().is_success() {
            self.pb_manager.log_above_progress_bars(&format!(
                "GET fallback for metadata returned {} for {url}, proceeding without content-length",
                probe.status()
            ));
            return Ok((None, false));
        }
        let (length, supports_range, _) = parse_metadata_from(probe.headers());
        Ok((length, supports_range))
    }

    /// Single-byte ranged GET used to coax metadata out of servers that don't
    /// answer HEAD properly. Returns the raw response for header inspection.
    async fn range_probe(&self, url: &str) -> Result<reqwest::Response, DlmError> {
        Ok(self
            .client
            .get(url)
            .header(RANGE, "bytes=0-0")
            .send()
            .await?)
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
                return already_completed_message(&final_file_path, &filename).await;
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
            return already_completed_message(&final_file_path, &filename).await;
        }

        // setup progress bar for the file
        pb_dl.set_message(ProgressBarManager::message_progress_bar(&filename));
        if let Some(total_size) = content_length {
            pb_dl.set_length(total_size);
        }

        let tmp_name = output_dir.join(format!("{filename}.part"));
        let resume_action = compute_resume_action(
            pb_dl,
            self.pb_manager,
            content_length,
            supports_range,
            &tmp_name,
        )
        .await?;

        // Fast-path: the .part already holds the complete body (e.g. a prior
        // run was killed between the last chunk and the rename). Finalize it
        // without issuing a GET.
        if matches!(resume_action, ResumeAction::AlreadyComplete) {
            let final_file_size = tfs::metadata(&tmp_name).await?.len();
            return finalize_download(&tmp_name, &final_file_path, &filename, final_file_size)
                .await;
        }

        // create/open file.part
        // no need for a BufWriter because the HTTP chunks are rather large
        let mut file = match &resume_action {
            ResumeAction::Resume(_) => {
                tfs::OpenOptions::new()
                    .append(true)
                    .create(false)
                    .open(&tmp_name)
                    .await?
            }
            // Fresh: truncate any stale .part (AlreadyComplete handled above).
            _ => tfs::File::create(&tmp_name).await?,
        };

        // build and send the download request
        let mut request = self.client.get(&file_link.url);
        if let ResumeAction::Resume(range) = &resume_action {
            request = request.header(RANGE, range);
        }
        let mut dl_response = request.send().await?;
        if !dl_response.status().is_success() {
            let status_code = dl_response.status().as_u16();
            return Err(DlmError::ResponseStatusNotSuccess { status_code });
        }

        // Start the speed/ETA clock from the first byte rather than from when the
        // progress bar was set up. The metadata probes, redirect resolution and
        // the initial GET round-trip above can take a long time on high-latency
        // servers and would otherwise skew the displayed speed and ETA.
        // `reset_elapsed` also resets the ETA estimator (Reset::Elapsed).
        pb_dl.reset_elapsed();

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

        finalize_download(&tmp_name, &final_file_path, &filename, final_file_size).await
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
}

/// What to do with a `.part` file when (re)starting a download.
#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
enum ResumeAction {
    /// Download the whole body fresh, truncating any existing `.part`.
    Fresh,
    /// Resume an existing `.part` by requesting an open-ended range from the
    /// given offset (e.g. `bytes=1024-`).
    Resume(String),
    /// The `.part` already holds the complete body — finalize without a GET.
    AlreadyComplete,
}

/// Decide how to (re)start a download given the state of any `.part` file on
/// disk and what the server advertises. Also primes the progress bar position.
async fn compute_resume_action(
    pb_dl: &ProgressBar,
    pb_manager: &ProgressBarManager,
    content_length: Option<u64>,
    supports_range: bool,
    tmp_name: &Path,
) -> Result<ResumeAction, DlmError> {
    if !tmp_name.exists() {
        if !supports_range {
            let log = format!(
                "The download of file {} should not be interrupted because the server does not support resuming the download (range bytes)",
                tmp_name.display()
            );
            pb_manager.log_above_progress_bars(&log);
        }
        return Ok(ResumeAction::Fresh);
    }

    // get existing file size
    let tmp_size = tfs::metadata(tmp_name).await?.len();
    match (supports_range, content_length) {
        (true, Some(cl)) => match tmp_size.cmp(&cl) {
            // already fully downloaded — finalize without re-fetching
            Ordering::Equal => {
                pb_dl.set_position(tmp_size);
                Ok(ResumeAction::AlreadyComplete)
            }
            // stale/corrupt .part bigger than the resource — start over
            Ordering::Greater => {
                let log = format!(
                    "Found part file {} with size {tmp_size} larger than the expected {cl} bytes, restarting the download from scratch",
                    tmp_name.display()
                );
                pb_manager.log_above_progress_bars(&log);
                pb_dl.set_position(0);
                Ok(ResumeAction::Fresh)
            }
            // genuine partial — resume from the current offset. An open-ended
            // `bytes=N-` range lets the server stream to EOF and dodges the
            // off-by-one of naming an explicit (inclusive) last byte index.
            Ordering::Less => {
                // set the progress bar to the current size; the elapsed/ETA
                // clock is (re)started from the first byte in `download`.
                pb_dl.set_position(tmp_size);
                Ok(ResumeAction::Resume(format!("bytes={tmp_size}-")))
            }
        },
        // range supported but unknown total — can't tell where the body ends,
        // so the .part can't be safely resumed
        (true, None) => {
            let log = format!(
                "Found part file {} with size {tmp_size} but it will be overridden because the server did not report a content length",
                tmp_name.display()
            );
            pb_manager.log_above_progress_bars(&log);
            pb_dl.set_position(0);
            Ok(ResumeAction::Fresh)
        }
        // server can't serve a partial body — restart from scratch
        (false, _) => {
            let log = format!(
                "Found part file {} with size {tmp_size} but it will be overridden because the server does not support resuming the download (range bytes)",
                tmp_name.display()
            );
            pb_manager.log_above_progress_bars(&log);
            pb_dl.set_position(0);
            Ok(ResumeAction::Fresh)
        }
    }
}

/// Build the "already completed, skipping" message for a destination file
/// that exists before the download starts.
async fn already_completed_message(
    final_file_path: &Path,
    filename: &str,
) -> Result<String, DlmError> {
    let final_file_size = tfs::metadata(final_file_path).await?.len();
    Ok(format!(
        "Skipping {filename} because the file is already completed [{}]",
        pretty_bytes_size(final_file_size)
    ))
}

/// Move a completed `.part` to its final path, guarding against a file that
/// appeared at the destination in the meantime.
async fn finalize_download(
    tmp_name: &Path,
    final_file_path: &Path,
    filename: &str,
    final_file_size: u64,
) -> Result<String, DlmError> {
    // check if the destination already has a finished file
    if tfs::metadata(final_file_path).await.is_ok() {
        let message = format!(
            "Can't finalize download because the file {} already exists",
            final_file_path.display()
        );
        return Err(DlmError::other(message));
    }

    // rename part file to final
    tfs::rename(tmp_name, final_file_path).await?;
    Ok(format!(
        "Completed {} [{}]",
        filename,
        pretty_bytes_size(final_file_size)
    ))
}

#[cfg(test)]
mod compute_resume_action_tests {
    use super::*;
    use indicatif::ProgressBar;
    use std::path::PathBuf;
    use tempfile::tempdir;

    /// Create a `.part` file of exactly `size` bytes inside `dir`.
    fn make_part(dir: &Path, size: u64) -> PathBuf {
        let path = dir.join("file.part");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(size).unwrap();
        path
    }

    #[tokio::test]
    async fn no_part_file_starts_fresh() {
        let dir = tempdir().unwrap();
        let tmp_name = dir.path().join("missing.part");
        let pb = ProgressBar::hidden();
        let pbm = ProgressBarManager::hidden();

        let action = compute_resume_action(&pb, &pbm, Some(100), true, &tmp_name)
            .await
            .unwrap();

        assert_eq!(action, ResumeAction::Fresh);
    }

    #[tokio::test]
    async fn no_part_file_starts_fresh_even_without_range_support() {
        let dir = tempdir().unwrap();
        let tmp_name = dir.path().join("missing.part");
        let pb = ProgressBar::hidden();
        let pbm = ProgressBarManager::hidden();

        let action = compute_resume_action(&pb, &pbm, Some(100), false, &tmp_name)
            .await
            .unwrap();

        assert_eq!(action, ResumeAction::Fresh);
    }

    #[tokio::test]
    async fn part_equal_to_content_length_is_already_complete() {
        let dir = tempdir().unwrap();
        let tmp_name = make_part(dir.path(), 100);
        let pb = ProgressBar::hidden();
        let pbm = ProgressBarManager::hidden();

        let action = compute_resume_action(&pb, &pbm, Some(100), true, &tmp_name)
            .await
            .unwrap();

        assert_eq!(action, ResumeAction::AlreadyComplete);
        assert_eq!(pb.position(), 100);
    }

    #[tokio::test]
    async fn part_larger_than_content_length_restarts_fresh() {
        let dir = tempdir().unwrap();
        let tmp_name = make_part(dir.path(), 150);
        let pb = ProgressBar::hidden();
        pb.set_position(150);
        let pbm = ProgressBarManager::hidden();

        let action = compute_resume_action(&pb, &pbm, Some(100), true, &tmp_name)
            .await
            .unwrap();

        assert_eq!(action, ResumeAction::Fresh);
        assert_eq!(pb.position(), 0);
    }

    #[tokio::test]
    async fn part_smaller_than_content_length_resumes_from_offset() {
        let dir = tempdir().unwrap();
        let tmp_name = make_part(dir.path(), 40);
        let pb = ProgressBar::hidden();
        let pbm = ProgressBarManager::hidden();

        let action = compute_resume_action(&pb, &pbm, Some(100), true, &tmp_name)
            .await
            .unwrap();

        assert_eq!(action, ResumeAction::Resume("bytes=40-".to_owned()));
        assert_eq!(pb.position(), 40);
    }

    #[tokio::test]
    async fn part_without_range_support_is_overridden() {
        let dir = tempdir().unwrap();
        let tmp_name = make_part(dir.path(), 40);
        let pb = ProgressBar::hidden();
        pb.set_position(40);
        let pbm = ProgressBarManager::hidden();

        let action = compute_resume_action(&pb, &pbm, Some(100), false, &tmp_name)
            .await
            .unwrap();

        assert_eq!(action, ResumeAction::Fresh);
        assert_eq!(pb.position(), 0);
    }

    #[tokio::test]
    async fn part_without_content_length_is_overridden() {
        let dir = tempdir().unwrap();
        let tmp_name = make_part(dir.path(), 40);
        let pb = ProgressBar::hidden();
        pb.set_position(40);
        let pbm = ProgressBarManager::hidden();

        let action = compute_resume_action(&pb, &pbm, None, true, &tmp_name)
            .await
            .unwrap();

        assert_eq!(action, ResumeAction::Fresh);
        assert_eq!(pb.position(), 0);
    }
}
