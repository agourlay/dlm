//! Integration tests for the dlm binary, driven against a local Axum server.
//!
//! Each test:
//!   1. Spawns a fresh `TestServer` on a random port.
//!   2. Invokes the dlm binary as a subprocess (path from `CARGO_BIN_EXE_dlm`).
//!   3. Asserts on the resulting files in a per-test temp dir, the exit code,
//!      and (for header-related tests) the headers the server received.
//!
//! Subprocess output is captured and included in any panic message via
//! `DlmRun`'s `Display` impl, so failing tests print stdout/stderr automatically.

mod common;

use common::{FILE_BODY, TestServer};
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;
use tokio::process::Command;
use tokio::time::timeout;

/// Captured result of one dlm invocation. `Display` formats stdout/stderr so
/// failed assertions can print it via `assert!(cond, "{r}")` without ceremony.
struct DlmRun {
    code: i32,
    stdout: String,
    stderr: String,
}

impl std::fmt::Display for DlmRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "exit={}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            self.code, self.stdout, self.stderr
        )
    }
}

/// Run the dlm binary with `args` against a fresh, auto-managed temp dir.
async fn run_dlm(args: &[&str]) -> (DlmRun, TempDir) {
    let dir = TempDir::new().unwrap();
    let run = run_dlm_in(args, dir.path()).await;
    (run, dir)
}

/// Run the dlm binary with `args` and a caller-owned output dir. Useful when
/// the test pre-seeds the dir (e.g., `.part` files for resume tests).
async fn run_dlm_in(args: &[&str], dir: &Path) -> DlmRun {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dlm"));
    cmd.args(args).arg("-o").arg(dir);
    capture(cmd).await
}

/// Run dlm with no `-o` injected — for print-and-exit flags or args-validation
/// tests where the output dir doesn't matter.
async fn run_dlm_raw(args: &[&str]) -> DlmRun {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dlm"));
    cmd.args(args);
    capture(cmd).await
}

async fn capture(mut cmd: Command) -> DlmRun {
    let output = cmd.output().await.expect("failed to spawn dlm");
    DlmRun {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// Wrap a future in a 10-second timeout. Used by tests that assert dlm
/// doesn't hang — without this, a regression would block until cargo's
/// per-test timeout (60s+), which is poor signal.
async fn no_hang<T>(fut: impl Future<Output = T>) -> T {
    timeout(Duration::from_secs(10), fut)
        .await
        .expect("dlm did not finish within 10s")
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

#[tokio::test]
async fn basic_download() {
    let server = TestServer::start().await;
    let url = server.url("/file/hello.bin");

    let (r, dir) = run_dlm(&[&url]).await;

    assert_eq!(r.code, 0, "{r}");
    assert_eq!(read(&dir.path().join("hello.bin")), FILE_BODY);
}

#[tokio::test]
async fn resume_via_range() {
    let server = TestServer::start().await;
    let url = server.url("/file/resumed.bin");
    let tmp = TempDir::new().unwrap();
    // Pre-seed a half-finished .part file from a prior "interrupted" run.
    let part = tmp.path().join("resumed.bin.part");
    std::fs::write(&part, &FILE_BODY[..FILE_BODY.len() / 2]).unwrap();

    let r = run_dlm_in(&[&url], tmp.path()).await;

    assert_eq!(r.code, 0, "{r}");
    assert_eq!(read(&tmp.path().join("resumed.bin")), FILE_BODY);
    assert!(!part.exists(), ".part file should be renamed away");
}

#[tokio::test]
async fn head_405_falls_back_to_get() {
    let server = TestServer::start().await;
    let url = server.url("/reject-head/page.bin");

    let (r, dir) = run_dlm(&[&url]).await;

    assert_eq!(r.code, 0, "{r}");
    assert_eq!(read(&dir.path().join("page.bin")), FILE_BODY);
}

#[tokio::test]
async fn head_zero_content_length_falls_back_to_get_probe() {
    let server = TestServer::start().await;
    let url = server.url("/zero-cl/data.bin");

    let (r, dir) = run_dlm(&[&url]).await;

    assert_eq!(r.code, 0, "{r}");
    assert_eq!(read(&dir.path().join("data.bin")), FILE_BODY);
}

#[tokio::test]
async fn redirect_resolves_filename() {
    // The URL itself has no extension. dlm follows the redirect to
    // /file/foo.bin and uses that as the saved filename.
    let server = TestServer::start().await;
    let url = server.url("/redirect-no-ext");

    let (r, dir) = run_dlm(&[&url]).await;

    assert_eq!(r.code, 0, "{r}");
    assert_eq!(read(&dir.path().join("foo.bin")), FILE_BODY);
}

#[tokio::test]
async fn content_disposition_supplies_filename() {
    // URL says "blob" (no extension). Server's Content-Disposition wins.
    let server = TestServer::start().await;
    let url = server.url("/disposition/from-header.bin");

    let (r, dir) = run_dlm(&[&url]).await;

    assert_eq!(r.code, 0, "{r}");
    assert_eq!(read(&dir.path().join("from-header.bin")), FILE_BODY);
}

#[tokio::test]
async fn basic_auth_succeeds() {
    let server = TestServer::start().await;
    let url = server.url("/auth/secret.bin");

    let (r, dir) = run_dlm(&[&url, "--user", "alice:s3cret"]).await;

    assert_eq!(r.code, 0, "{r}");
    assert_eq!(read(&dir.path().join("secret.bin")), FILE_BODY);
}

#[tokio::test]
async fn basic_auth_missing_no_file_left() {
    // Server returns 401 when Authorization is absent. dlm logs the per-link
    // error but doesn't propagate it to the exit code (pre-existing behavior).
    // The asserted invariant is: no file ends up on disk.
    let server = TestServer::start().await;
    let url = server.url("/auth/secret.bin");

    let (_r, dir) = run_dlm(&[&url]).await;

    assert!(!dir.path().join("secret.bin").exists());
}

#[tokio::test]
async fn custom_headers_reach_server() {
    let server = TestServer::start().await;
    let url = server.url("/echo-headers");

    let (r, _dir) = run_dlm(&[&url, "-H", "X-Test-One: alpha", "-H", "X-Test-Two: bravo"]).await;
    assert_eq!(r.code, 0, "{r}");

    let headers = server.last_echo_headers();
    assert_eq!(
        headers.get("x-test-one").and_then(|v| v.to_str().ok()),
        Some("alpha")
    );
    assert_eq!(
        headers.get("x-test-two").and_then(|v| v.to_str().ok()),
        Some("bravo")
    );
}

#[tokio::test]
async fn default_user_agent_identifies_dlm() {
    let server = TestServer::start().await;
    let url = server.url("/echo-headers");

    let (r, _) = run_dlm(&[&url]).await;
    assert_eq!(r.code, 0, "{r}");

    let headers = server.last_echo_headers();
    let ua = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ua.starts_with("dlm/"),
        "expected default UA to start with 'dlm/', got '{ua}'"
    );
}

#[tokio::test]
async fn retry_then_succeed_on_transient_500s() {
    let server = TestServer::start().await;
    server.set_flaky_fails(2); // first 2 requests get 503, then it works.
    let url = server.url("/flaky");

    // The polite retry strategy waits 500ms between fixed attempts, so two
    // retries take ~1s — slow enough to be visible, fast enough for CI.
    let (r, dir) = run_dlm(&[&url]).await;

    assert_eq!(r.code, 0, "{r}");
    // The flaky route saves under the URL's last segment.
    assert_eq!(read(&dir.path().join("flaky")), FILE_BODY);
}

#[tokio::test]
async fn input_file_concurrent_downloads() {
    let server = TestServer::start().await;
    let tmp = TempDir::new().unwrap();
    let input = tmp.path().join("links.txt");
    std::fs::write(
        &input,
        format!(
            "{}\n{}\n",
            server.url("/file/one.bin"),
            server.url("/file/two.bin"),
        ),
    )
    .unwrap();

    let r = run_dlm_in(
        &["-i", input.to_str().unwrap(), "--max-concurrent", "2"],
        tmp.path(),
    )
    .await;

    assert_eq!(r.code, 0, "{r}");
    assert_eq!(read(&tmp.path().join("one.bin")), FILE_BODY);
    assert_eq!(read(&tmp.path().join("two.bin")), FILE_BODY);
}

#[tokio::test]
async fn skips_already_completed_file() {
    let server = TestServer::start().await;
    let url = server.url("/file/already.bin");
    let tmp = TempDir::new().unwrap();
    // Pre-populate the destination with the final file. dlm should skip it.
    std::fs::write(tmp.path().join("already.bin"), b"unchanged").unwrap();

    let r = run_dlm_in(&[&url], tmp.path()).await;

    assert_eq!(r.code, 0, "{r}");
    // File untouched (still our placeholder, not FILE_BODY).
    assert_eq!(
        std::fs::read(tmp.path().join("already.bin")).unwrap(),
        b"unchanged"
    );
}

#[tokio::test]
async fn custom_user_agent_reaches_server() {
    let server = TestServer::start().await;
    let url = server.url("/echo-headers");

    let (r, _) = run_dlm(&[&url, "--user-agent", "MyTestAgent/1.0"]).await;
    assert_eq!(r.code, 0, "{r}");

    let headers = server.last_echo_headers();
    let ua = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(ua, "MyTestAgent/1.0");
}

#[tokio::test]
async fn random_user_agent_reaches_server() {
    let server = TestServer::start().await;
    let url = server.url("/echo-headers");

    let (r, _) = run_dlm(&[&url, "--random-user-agent"]).await;
    assert_eq!(r.code, 0, "{r}");

    let headers = server.last_echo_headers();
    let ua = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    // The built-in pool is browser UAs; all start with "Mozilla/5.0".
    assert!(
        ua.starts_with("Mozilla/5.0"),
        "expected a browser UA, got '{ua}'"
    );
}

#[tokio::test]
async fn list_user_agents_prints_and_exits() {
    let r = run_dlm_raw(&["--list-user-agents"]).await;

    assert_eq!(r.code, 0, "{r}");
    let lines: Vec<&str> = r.stdout.lines().collect();
    assert!(
        lines.len() >= 16,
        "expected at least 16 UAs, got {} lines: {r}",
        lines.len()
    );
    assert!(
        lines.iter().all(|l| l.contains("Mozilla/5.0")),
        "every line should contain a Mozilla/5.0 UA: {r}"
    );
}

#[tokio::test]
async fn not_found_leaves_no_file_on_disk() {
    let server = TestServer::start().await;
    let url = server.url("/never-found");

    let (_r, dir) = run_dlm(&[&url]).await;

    // 404 is not retryable; dlm logs the error and exits.
    // Per current behavior the exit code isn't checked, but the important
    // invariant is that no `.part` file or final file is left behind.
    let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert!(
        entries.is_empty(),
        "expected empty output dir, got {} entries",
        entries.len()
    );
}

#[tokio::test]
async fn input_file_skips_comments_and_blanks() {
    let server = TestServer::start().await;
    let tmp = TempDir::new().unwrap();
    let input = tmp.path().join("mixed.txt");
    std::fs::write(
        &input,
        format!(
            "# header comment\n\n{}\n   # indented comment\n{}\n\n",
            server.url("/file/one.bin"),
            server.url("/file/two.bin"),
        ),
    )
    .unwrap();

    let r = run_dlm_in(&["-i", input.to_str().unwrap()], tmp.path()).await;

    assert_eq!(r.code, 0, "{r}");
    assert_eq!(read(&tmp.path().join("one.bin")), FILE_BODY);
    assert_eq!(read(&tmp.path().join("two.bin")), FILE_BODY);
}

#[tokio::test]
async fn download_when_server_lacks_range_support() {
    // Server doesn't advertise Accept-Ranges and ignores Range; dlm should
    // still produce the full file (just without resume capability).
    let server = TestServer::start().await;
    let url = server.url("/no-range/foo.bin");

    let (r, dir) = run_dlm(&[&url]).await;

    assert_eq!(r.code, 0, "{r}");
    assert_eq!(read(&dir.path().join("foo.bin")), FILE_BODY);
}

#[tokio::test]
async fn resume_attempt_against_no_range_server_overwrites() {
    // .part file exists but server doesn't support Range. dlm logs that
    // it will overwrite, then re-downloads from scratch.
    let server = TestServer::start().await;
    let url = server.url("/no-range/over.bin");
    let tmp = TempDir::new().unwrap();
    let part = tmp.path().join("over.bin.part");
    std::fs::write(&part, b"stale junk that should be discarded").unwrap();

    let r = run_dlm_in(&[&url], tmp.path()).await;

    assert_eq!(r.code, 0, "{r}");
    assert_eq!(read(&tmp.path().join("over.bin")), FILE_BODY);
    assert!(!part.exists(), ".part should be renamed away");
}

#[tokio::test]
async fn accept_header_via_custom_header_flag() {
    let server = TestServer::start().await;
    let url = server.url("/echo-headers");

    let (r, _) = run_dlm(&[&url, "-H", "Accept: application/json"]).await;
    assert_eq!(r.code, 0, "{r}");

    let headers = server.last_echo_headers();
    assert_eq!(
        headers.get("accept").and_then(|v| v.to_str().ok()),
        Some("application/json")
    );
}

#[tokio::test]
async fn retry_count_zero_disables_retries() {
    // Server fails 5 times then succeeds; with --retry 0 dlm gives up after
    // the first failure, so no file ends up on disk.
    let server = TestServer::start().await;
    server.set_flaky_fails(5);
    let url = server.url("/flaky");

    let (_r, dir) = run_dlm(&[&url, "--retry", "0"]).await;

    assert!(!dir.path().join("flaky").exists());
}

#[tokio::test]
async fn input_file_mixed_success_and_failure() {
    // One URL returns 200, another returns 404. Both are attempted; the
    // 200 lands on disk, the 404 leaves nothing.
    let server = TestServer::start().await;
    let tmp = TempDir::new().unwrap();
    let input = tmp.path().join("links.txt");
    std::fs::write(
        &input,
        format!(
            "{}\n{}\n",
            server.url("/file/good.bin"),
            server.url("/never-found"),
        ),
    )
    .unwrap();

    let _r = run_dlm_in(&["-i", input.to_str().unwrap()], tmp.path()).await;

    assert_eq!(read(&tmp.path().join("good.bin")), FILE_BODY);
    assert!(!tmp.path().join("never-found").exists());
}

#[tokio::test]
async fn incomplete_download_no_file_left() {
    // HEAD claims 64 KiB, GET serves 32 KiB → IncompleteDownload.
    // `--retry 0` keeps the test fast (otherwise the retryable error would
    // exhaust the default 10 retries with exponential backoff).
    let server = TestServer::start().await;
    let url = server.url("/short/data.bin");

    let (_r, dir) = run_dlm(&[&url, "--retry", "0"]).await;

    assert!(
        !dir.path().join("data.bin").exists(),
        "no final file should be produced on incomplete download"
    );
    // The .part file remains on disk because dlm errors before the rename —
    // proves the IncompleteDownload code path was hit (vs. some earlier error).
    assert!(
        dir.path().join("data.bin.part").exists(),
        ".part file should remain after IncompleteDownload"
    );
}

#[tokio::test]
async fn filename_star_utf8_disposition_decoded_on_disk() {
    let server = TestServer::start().await;
    // No extension in the URL, so dlm has to use the disposition filename.
    let url = server.url("/disposition-star");

    let (r, dir) = run_dlm(&[&url]).await;

    assert_eq!(r.code, 0, "{r}");
    assert_eq!(
        read(&dir.path().join("my file.txt")),
        FILE_BODY,
        "RFC 6266 filename*= should be percent-decoded into the on-disk name"
    );
}

#[tokio::test]
async fn query_string_preserved_in_request() {
    let server = TestServer::start().await;
    // The /check-query route 200s only if the request URL carried
    // `?token=abc123`. If dlm strips it, the response is 400 and no file lands.
    let url = server.url("/check-query/data.bin?token=abc123");

    let (r, dir) = run_dlm(&[&url]).await;

    assert_eq!(r.code, 0, "{r}");
    // The saved filename should be the URL's path basename (no query suffix).
    assert_eq!(read(&dir.path().join("data.bin")), FILE_BODY);
}

#[tokio::test]
async fn output_dir_must_exist() {
    // Build a guaranteed-nonexistent path inside a real tempdir so we can't
    // accidentally pass via a hardcoded path that happens to exist on CI.
    let tmp = TempDir::new().unwrap();
    let bad = tmp.path().join("does-not-exist");

    let r = run_dlm_raw(&[
        "http://example.invalid/foo.bin",
        "-o",
        bad.to_str().unwrap(),
    ])
    .await;

    assert_ne!(r.code, 0, "should exit non-zero on bad -o: {r}");
    assert!(
        r.stderr.contains("outputDir") || r.stderr.to_lowercase().contains("does not exist"),
        "stderr should mention the bad output dir: {r}"
    );
}

#[tokio::test]
async fn output_dir_is_a_file_errors() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("not-a-dir.txt");
    std::fs::write(&file_path, b"i am a file, not a dir").unwrap();

    let r = run_dlm_raw(&[
        "http://example.invalid/foo.bin",
        "-o",
        file_path.to_str().unwrap(),
    ])
    .await;

    assert_ne!(r.code, 0, "should exit non-zero when -o is a file: {r}");
    assert!(
        r.stderr.contains("outputDir") || r.stderr.to_lowercase().contains("does not exist"),
        "stderr should mention the bad output dir: {r}"
    );
}

#[tokio::test]
async fn connection_timeout_zero_is_rejected() {
    // A zero timeout would make reqwest's connect/read timeouts fire instantly,
    // breaking every download — clap must reject it at parse time.
    let r = run_dlm_raw(&[
        "http://example.invalid/foo.bin",
        "--connection-timeout",
        "0",
    ])
    .await;

    assert_ne!(
        r.code, 0,
        "should exit non-zero on --connection-timeout 0: {r}"
    );
    assert!(
        r.stderr.contains("connection-timeout") && r.stderr.contains("1.."),
        "stderr should explain the value is out of range: {r}"
    );
}

#[tokio::test]
async fn max_concurrent_zero_is_rejected() {
    // 0 concurrent downloads leaves no progress bar to claim, so every download
    // would block forever — clap must reject it at parse time.
    let r = run_dlm_raw(&["http://example.invalid/foo.bin", "--max-concurrent", "0"]).await;

    assert_ne!(r.code, 0, "should exit non-zero on --max-concurrent 0: {r}");
    assert!(
        r.stderr.contains("max-concurrent") && r.stderr.contains("1.."),
        "stderr should explain the value is out of range: {r}"
    );
}

#[tokio::test]
async fn range_416_during_resume_does_not_loop() {
    // Pre-seed a partial .part. Server's HEAD reports the file as resumable,
    // but any GET-with-Range returns 416. dlm must surface the error and
    // give up — 416 is not in the retryable set, so no retry storm. The
    // `no_hang` wrapper turns a regression into a clear timeout, not a 60s
    // hang waiting for cargo's per-test deadline.
    let server = TestServer::start().await;
    let url = server.url("/always-416/file.bin");
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("file.bin.part"), &FILE_BODY[..1024]).unwrap();

    let _r = no_hang(run_dlm_in(&[&url], tmp.path())).await;

    assert!(
        !tmp.path().join("file.bin").exists(),
        "no final file should be produced on 416"
    );
}

#[tokio::test]
async fn part_file_already_at_expected_size() {
    // The .part already holds the full body — e.g. dlm was killed between the
    // last chunk and the rename. dlm recognises it as complete, skips the GET
    // entirely, and finalises by renaming the .part to its final name.
    let server = TestServer::start().await;
    let url = server.url("/file/complete.bin");
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("complete.bin.part"), FILE_BODY).unwrap();

    let r = no_hang(run_dlm_in(&[&url, "--retry", "0"], tmp.path())).await;

    assert_eq!(r.code, 0, "a complete .part should finalise cleanly: {r}");
    assert_eq!(
        read(&tmp.path().join("complete.bin")),
        FILE_BODY,
        "the complete .part should be renamed to the final file"
    );
    assert!(
        !tmp.path().join("complete.bin.part").exists(),
        ".part should be renamed away after finalising"
    );
}

#[tokio::test]
async fn part_file_larger_than_expected_total() {
    // `.part` exceeds HEAD's reported Content-Length — e.g., a stale file
    // copied in by hand. dlm discards it and restarts the download from
    // scratch, producing the correct final file.
    let server = TestServer::start().await;
    let url = server.url("/file/oversize.bin");
    let tmp = TempDir::new().unwrap();
    let oversize = vec![0u8; FILE_BODY.len() + 16 * 1024];
    std::fs::write(tmp.path().join("oversize.bin.part"), &oversize).unwrap();

    let r = no_hang(run_dlm_in(&[&url, "--retry", "0"], tmp.path())).await;

    assert_eq!(r.code, 0, "oversize .part should restart and finish: {r}");
    assert_eq!(
        read(&tmp.path().join("oversize.bin")),
        FILE_BODY,
        "the stale oversized .part should be discarded and re-downloaded"
    );
    assert!(
        !tmp.path().join("oversize.bin.part").exists(),
        ".part should be renamed away after finishing"
    );
}

#[tokio::test]
async fn server_disconnects_mid_body_does_not_hang() {
    // Server sends one small chunk then drops the body stream. dlm should
    // surface the failure as a body / connection error and exit, not hang
    // waiting for more data. `--retry 0` to skip the retry budget; `no_hang`
    // turns a regression into a 10s timeout, not a 60s cargo hang.
    let server = TestServer::start().await;
    let url = server.url("/cut-stream/data.bin");

    let (_r, dir) = no_hang(run_dlm(&[&url, "--retry", "0"])).await;

    assert!(
        !dir.path().join("data.bin").exists(),
        "no final file should be produced when the connection is cut mid-body"
    );
}

#[tokio::test]
async fn server_silent_before_headers_does_not_hang() {
    // Server accepts the connection but never sends response headers. Without a
    // read timeout, dlm's `send()` would block forever; with one it gives up
    // after `--read-timeout` (1s here). `--retry 0` so the (retryable) timeout
    // isn't re-attempted; `no_hang` turns a regression into a 10s timeout, not
    // a 60s cargo hang. 1s is well under both the 10s no_hang bound and the
    // server's 30s stall.
    let server = TestServer::start().await;
    let url = server.url("/stall/data.bin");

    let (_r, dir) = no_hang(run_dlm(&[&url, "--retry", "0", "--read-timeout", "1"])).await;

    assert!(
        !dir.path().join("data.bin").exists(),
        "no file should be produced when the server never sends headers"
    );
}

#[tokio::test]
async fn slow_first_byte_is_tolerated() {
    // Server sends headers immediately but delays the first body byte ~2s — a
    // high time-to-first-byte, slow-to-respond server. That is longer than the
    // 1s connection/stall timeout but within the 5s read timeout, so the
    // download must succeed rather than time out on the first chunk and retry
    // (the bug in issue #461, where one per-chunk timeout was tied to
    // --connection-timeout and tripped on slow starts). `--retry 0` proves a
    // single attempt succeeds.
    let server = TestServer::start().await;
    let url = server.url("/slow-first-byte/data.bin");

    let (r, dir) = no_hang(run_dlm(&[
        &url,
        "--retry",
        "0",
        "--connection-timeout",
        "1",
        "--read-timeout",
        "5",
    ]))
    .await;

    assert_eq!(
        r.code, 0,
        "slow first byte should download successfully: {r}"
    );
    assert_eq!(read(&dir.path().join("data.bin")), FILE_BODY);
}
