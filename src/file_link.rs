use crate::dlm_error::DlmError;
use percent_encoding::percent_decode_str;

/// Base names Windows reserves for legacy device files; we suffix `_` to dodge
/// them. Matched case-insensitively against the part of the name before the
/// first `.`, since Windows reserves them whatever the extension (`CON.txt`,
/// `nul.tar.gz`).
const FORBIDDEN_WINDOWS_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Extensions of server-side scripts. A URL ending in one names the page that
/// produces the download (`get.php?id=1`), not the file itself, so it is not
/// trusted as a file name: dlm asks the server for the real one, and keeps
/// the query in the fallback name so `?id=1` and `?id=2` do not collide.
const PAGE_EXTENSIONS: &[&str] = &["php", "asp", "aspx", "jsp", "jspx", "cgi", "pl", "cfm"];

/// Whether `name` ends in an extension that identifies a file - not a
/// server-side script, see [`PAGE_EXTENSIONS`].
fn has_file_extension(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, ext)| {
        !ext.is_empty() && !PAGE_EXTENSIONS.iter().any(|p| p.eq_ignore_ascii_case(ext))
    })
}

/// Suffix of the file a download is written to until it completes.
pub const PART_SUFFIX: &str = ".part";

/// Most filesystems (ext4, NTFS, APFS) reject single path components longer
/// than this.
const MAX_COMPONENT_BYTES: usize = 255;

/// Cap on filename byte length. Leaves room for [`PART_SUFFIX`], so the
/// in-progress `<name>.part` file fits the filesystem limit as well.
const MAX_FILENAME_BYTES: usize = MAX_COMPONENT_BYTES - PART_SUFFIX.len();

/// Longest extension (dot included) preserved when a name is truncated.
/// Anything longer is more likely part of the name than a real extension.
const MAX_KEPT_EXTENSION_BYTES: usize = 16;

/// Returns a filename that is safe to use on Windows, Linux, and macOS.
///
/// - Replaces `/ \ : * ? " < > | ^` with `_` (forbidden on Windows; `/` and
///   `:` also break paths on Unix-likes).
/// - Replaces ASCII control characters with `_`.
/// - Trims trailing dots and whitespace (Windows rejects either).
/// - Trims leading whitespace, but keeps leading dots so hidden files like
///   `.gitignore` survive.
/// - Suffixes Windows reserved device names (CON, PRN, AUX, NUL, COM1-9,
///   LPT1-9) with `_`, with or without an extension (`CON.txt` → `CON_.txt`).
/// - Truncates to 250 bytes at a UTF-8 char boundary, leaving room for the
///   `.part` suffix. A short extension is kept, the stem is cut instead.
///
/// Does not strip path components — callers handling header-supplied names
/// must run `Path::new(input).file_name()` first to defeat `../`-style
/// traversal.
pub fn cleanup_filename(input: &str) -> String {
    let mut result: String = input
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '^' => '_',
            c if c.is_control() => '_',
            _ => c,
        })
        .collect();

    let trimmed_end = result.trim_end_matches(|c: char| c.is_whitespace() || c == '.');
    let trimmed = trimmed_end.trim_start_matches(char::is_whitespace);
    result = trimmed.to_string();

    // Windows ignores trailing spaces before the extension, so `CON .txt` is
    // reserved too - compare the stem with those trimmed.
    let stem_end = result.find('.').unwrap_or(result.len());
    let stem = result[..stem_end].trim_end();
    if FORBIDDEN_WINDOWS_NAMES
        .iter()
        .any(|name| name.eq_ignore_ascii_case(stem))
    {
        result.insert(stem.len(), '_');
    }

    if result.len() > MAX_FILENAME_BYTES {
        result = truncate_keeping_extension(&result);
    }

    result
}

/// Cut `name` down to [`MAX_FILENAME_BYTES`] at a UTF-8 char boundary,
/// shortening the stem so a short extension survives (`<long>.iso` stays an
/// `.iso`).
fn truncate_keeping_extension(name: &str) -> String {
    let extension = name
        .rfind('.')
        .filter(|&i| i > 0 && name.len() - i <= MAX_KEPT_EXTENSION_BYTES)
        .map_or("", |i| &name[i..]);
    let mut end = MAX_FILENAME_BYTES - extension.len();
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    let stem = &name[..end];
    if extension.is_empty() {
        // the cut may land on a dot or a space, which Windows rejects at the end
        stem.trim_end_matches(|c: char| c.is_whitespace() || c == '.')
            .to_string()
    } else {
        format!("{stem}{extension}")
    }
}

#[derive(Debug)]
pub struct FileLink {
    pub url: String,
    pub filename_without_extension: String,
    pub extension: Option<String>,
}

impl FileLink {
    pub fn new(url: &str) -> Result<Self, DlmError> {
        let trimmed = url.trim();
        if trimmed.is_empty() {
            return Err(DlmError::other(
                "FileLink cannot be built from an empty URL".to_string(),
            ));
        }

        let parsed = reqwest::Url::parse(trimmed).map_err(|e| {
            DlmError::other(format!("FileLink could not parse URL '{trimmed}': {e}"))
        })?;

        // `path_segments` keeps segments percent-encoded; we decode after the
        // split so a literal '%2F' inside a segment is not mistaken for a
        // path separator. Query and fragment are already separated by the
        // parser, so we don't need to strip them ourselves.
        let path_part = parsed
            .path_segments()
            .and_then(|mut s| s.next_back())
            .filter(|s| !s.is_empty())
            .map(|s| percent_decode_str(s).decode_utf8_lossy().into_owned());
        let query = parsed.query().filter(|q| !q.is_empty());
        // When the path yields a name with a file extension we drop the
        // query — it's noise that doesn't help disambiguate. Otherwise (no
        // extension, a script page like `get.php`, or no segment at all) the
        // name is only a placeholder: it carries no extension, so the real one
        // is asked from the server (Content-Disposition, redirect target), and
        // it keeps the query so URLs that differ only in their parameters
        // don't collide on the same placeholder filename.
        let (raw_name, known_extension) = match (path_part, query) {
            (Some(p), _) if has_file_extension(&p) => (p, true),
            (Some(p), Some(q)) => {
                let q_decoded = percent_decode_str(q).decode_utf8_lossy();
                (format!("{p}?{q_decoded}"), false)
            }
            (Some(p), None) => (p, false),
            (None, Some(q)) => (
                percent_decode_str(q).decode_utf8_lossy().into_owned(),
                false,
            ),
            (None, None) => {
                return Err(DlmError::other(format!(
                    "FileLink could not derive a filename from '{trimmed}', it has neither a path nor a query"
                )));
            }
        };
        let safe_name = cleanup_filename(&raw_name);
        if safe_name.is_empty() {
            let message = format!("FileLink could not derive a usable filename from '{trimmed}'");
            return Err(DlmError::other(message));
        }

        // a placeholder name may hold dots (`get.php`, `?v=1.2`) that are no
        // extension, so it is not split
        let (extension, filename_without_extension) = if known_extension {
            Self::extract_extension_from_filename(&safe_name)
        } else {
            (None, safe_name)
        };

        Ok(Self {
            url: url.to_string(),
            filename_without_extension,
            extension,
        })
    }

    pub fn filename(&self) -> String {
        let joined = match &self.extension {
            Some(ext) => format!("{}.{ext}", self.filename_without_extension),
            None => self.filename_without_extension.clone(),
        };
        // Re-run cleanup on the joined name so the combined byte length still
        // fits the byte limit when the extension pushes over.
        cleanup_filename(&joined)
    }

    /// Splits a filename on its rightmost `.`. Assumes the input is already
    /// sanitized via `cleanup_filename`.
    pub fn extract_extension_from_filename(filename: &str) -> (Option<String>, String) {
        if let Some((before, after)) = filename.rsplit_once('.') {
            (Some(after.to_string()), before.to_string())
        } else {
            (None, filename.to_string())
        }
    }
}

#[cfg(test)]
mod file_link_tests {
    use crate::file_link::*;

    #[test]
    fn no_empty_string() {
        match FileLink::new("") {
            Err(DlmError::Other { message }) => assert_eq!(
                message,
                "FileLink cannot be built from an empty URL".to_string()
            ),
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[test]
    fn happy_case() {
        let url = "https://www.google.com/area51.txt";
        match FileLink::new(url) {
            Ok(fl) => {
                assert_eq!(fl.url, url);
                assert_eq!(fl.filename_without_extension, "area51".to_string());
                assert_eq!(fl.extension, Some("txt".to_string()));
            }
            other => panic!("unexpected error, got {other:?}"),
        }
    }

    #[test]
    fn trailing_slash() {
        let url = "https://www.google.com/area51/";
        match FileLink::new(url) {
            Err(DlmError::Other { message }) => assert_eq!(
                message,
                "FileLink could not derive a filename from 'https://www.google.com/area51/', it has neither a path nor a query".to_string()
            ),
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[test]
    fn no_extension() {
        let url = "https://www.google.com/area51";
        let fl = FileLink::new(url).unwrap();
        assert_eq!(fl.extension, None);
        assert_eq!(fl.filename_without_extension, "area51");
        assert_eq!(fl.url, url);
    }

    #[test]
    fn no_extension_query_string_kept() {
        // No extension on the path → the query is preserved (sanitized) so
        // URLs that share a path stem but differ in their parameters don't
        // collide on the same placeholder filename.
        let url = "https://oeis.org/search?q=id:A000001&fmt=json";
        let fl = FileLink::new(url).unwrap();
        assert_eq!(fl.extension, None);
        assert_eq!(
            fl.filename_without_extension,
            "search_q=id_A000001&fmt=json"
        );
        assert_eq!(fl.url, url);
    }

    #[test]
    fn no_extension_query_string_disambiguates() {
        // Two URLs that share a path stem and differ only in their query
        // parameters must produce different placeholder filenames. Otherwise
        // the downloader's "skip if final exists" check silently turns the
        // second download into a no-op against the first URL's content when
        // neither URL serves a Content-Disposition header or redirect.
        let fl1 = FileLink::new("https://api.example.com/get?id=1").unwrap();
        let fl2 = FileLink::new("https://api.example.com/get?id=2").unwrap();
        assert_ne!(
            fl1.filename(),
            fl2.filename(),
            "URLs differing only in query params must not collide on the placeholder filename"
        );
    }

    /// A script page names the producer of the download, not the file, so
    /// URLs differing only in their query must not collide, and the name is
    /// a placeholder the server's Content-Disposition can replace.
    #[test]
    fn script_page_urls_keep_their_query_and_no_extension() {
        let fl1 = FileLink::new("https://example.com/get.php?id=1").unwrap();
        let fl2 = FileLink::new("https://example.com/get.php?id=2").unwrap();
        assert_eq!(fl1.extension, None);
        assert_eq!(fl1.filename(), "get.php_id=1");
        assert_ne!(fl1.filename(), fl2.filename());

        let fl = FileLink::new("https://example.com/download.ASPX").unwrap();
        assert_eq!(fl.extension, None);
        assert_eq!(fl.filename(), "download.ASPX");
    }

    /// Dots in the query of a placeholder name are not an extension.
    #[test]
    fn dots_in_a_placeholder_query_are_not_an_extension() {
        let fl = FileLink::new("https://example.com/latest?version=1.2").unwrap();
        assert_eq!(fl.extension, None);
        assert_eq!(fl.filename(), "latest_version=1.2");
    }

    #[test]
    fn filename_with_extension() {
        let fl = FileLink::new("https://example.com/area51.txt").unwrap();
        assert_eq!(fl.filename(), "area51.txt");
    }

    #[test]
    fn filename_without_extension() {
        let fl = FileLink::new("https://example.com/area51").unwrap();
        assert_eq!(fl.filename(), "area51");
    }

    #[test]
    fn extract_extension_ok() {
        let (ext, filename) = FileLink::extract_extension_from_filename("area51.txt");
        assert_eq!(filename, "area51");
        assert_eq!(ext, Some("txt".to_string()));
    }

    #[test]
    fn extract_extension_with_query_param() {
        let url = "https://releases.ubuntu.com/21.10/ubuntu-21.10-desktop-amd64.iso?id=123";
        let fl = FileLink::new(url).unwrap();
        assert_eq!(fl.extension, Some("iso".to_string()));
        assert_eq!(fl.filename_without_extension, "ubuntu-21.10-desktop-amd64");
        assert_eq!(fl.url, url);
    }

    #[test]
    fn extract_extension_with_query_param_bis() {
        let url = "https://atom-installer.github.com/v1.58.0/atom-amd64.deb?s=1627025597&ext=.deb";
        let fl = FileLink::new(url).unwrap();
        assert_eq!(fl.extension, Some("deb".to_string()));
        assert_eq!(fl.url, url);
        assert_eq!(fl.filename_without_extension, "atom-amd64");
    }

    #[test]
    fn extract_extension_with_parts() {
        let url = "https://www.google.com/area51/alien-archive.tar.00";
        let fl = FileLink::new(url).unwrap();
        assert_eq!(fl.extension, Some("00".to_string()));
        // Only the last dot splits the extension, but `filename()` joins the
        // two back, so each part keeps its own name and `.part` file.
        assert_eq!(fl.filename_without_extension, "alien-archive.tar");
        assert_eq!(fl.filename(), "alien-archive.tar.00");
        assert_eq!(fl.url, url);
    }

    #[test]
    fn percent_encoded_space_in_filename() {
        let url = "https://example.com/path/My%20Report.pdf";
        let fl = FileLink::new(url).unwrap();
        assert_eq!(fl.extension, Some("pdf".to_string()));
        assert_eq!(fl.filename_without_extension, "My Report");
    }

    #[test]
    fn encoded_slash_in_segment_is_not_a_separator() {
        // %2F is a literal '/' inside a single path segment — must not split on it.
        let url = "https://example.com/files/My%2FReport.pdf";
        let fl = FileLink::new(url).unwrap();
        assert_eq!(fl.extension, Some("pdf".to_string()));
        // '/' is sanitized to '_' so the name remains usable on disk.
        assert_eq!(fl.filename_without_extension, "My_Report");
    }

    #[test]
    fn empty_path_falls_back_to_query() {
        // No usable path segment — the query is used as a placeholder name.
        // The real filename comes from Content-Disposition or the redirect.
        let url = "https://download.mozilla.org/?product=firefox-latest-ssl&os=osx&lang=en-US";
        let fl = FileLink::new(url).unwrap();
        assert_eq!(fl.url, url);
        assert_eq!(fl.extension, None);
        assert_eq!(
            fl.filename_without_extension,
            "product=firefox-latest-ssl&os=osx&lang=en-US"
        );
    }

    #[test]
    fn empty_path_no_query_errors() {
        // No path and no query — there is nothing to derive a name from.
        let url = "https://example.com/";
        match FileLink::new(url) {
            Err(DlmError::Other { message }) => assert!(
                message.contains("neither a path nor a query"),
                "unexpected message: {message}"
            ),
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[test]
    fn fragment_in_url_is_stripped() {
        let url = "https://example.com/file.bin#section";
        let fl = FileLink::new(url).unwrap();
        assert_eq!(fl.extension, Some("bin".to_string()));
        assert_eq!(fl.filename_without_extension, "file");
    }

    #[test]
    fn cleanup_replaces_forbidden_chars() {
        assert_eq!(
            cleanup_filename("a:b*c?d|e\"f<g>h^i.txt"),
            "a_b_c_d_e_f_g_h_i.txt"
        );
        assert_eq!(
            cleanup_filename("path/with\\separators.bin"),
            "path_with_separators.bin"
        );
    }

    #[test]
    fn cleanup_replaces_control_chars() {
        assert_eq!(cleanup_filename("a\u{0}b\u{1}c\u{1f}d.txt"), "a_b_c_d.txt");
    }

    #[test]
    fn cleanup_trims_trailing_dots_and_whitespace() {
        assert_eq!(cleanup_filename("file.txt..  "), "file.txt");
        assert_eq!(cleanup_filename("file.txt   "), "file.txt");
        assert_eq!(cleanup_filename("file.txt."), "file.txt");
    }

    #[test]
    fn cleanup_trims_leading_whitespace_only() {
        assert_eq!(cleanup_filename("   file.txt"), "file.txt");
        // Leading dots are kept — they are needed for hidden files on Unix.
        assert_eq!(cleanup_filename(".gitignore"), ".gitignore");
        assert_eq!(cleanup_filename("..hidden.txt"), "..hidden.txt");
    }

    #[test]
    fn cleanup_collapses_to_empty_for_dots_only() {
        assert_eq!(cleanup_filename(".."), "");
        assert_eq!(cleanup_filename("..."), "");
        assert_eq!(cleanup_filename("   "), "");
    }

    #[test]
    fn cleanup_escapes_windows_reserved_names() {
        assert_eq!(cleanup_filename("CON"), "CON_");
        assert_eq!(cleanup_filename("nul"), "nul_");
        assert_eq!(cleanup_filename("LPT3"), "LPT3_");
        assert_eq!(cleanup_filename("aux"), "aux_");
    }

    #[test]
    fn cleanup_escapes_windows_reserved_names_with_extension() {
        assert_eq!(cleanup_filename("CON.txt"), "CON_.txt");
        assert_eq!(cleanup_filename("nul.tar.gz"), "nul_.tar.gz");
        assert_eq!(cleanup_filename("Com1.log"), "Com1_.log");
        assert_eq!(cleanup_filename("CON .txt"), "CON_ .txt");
    }

    #[test]
    fn cleanup_keeps_names_that_only_contain_a_reserved_word() {
        assert_eq!(cleanup_filename("CONFIG.txt"), "CONFIG.txt");
        assert_eq!(cleanup_filename("console.log"), "console.log");
        assert_eq!(cleanup_filename("my.CON.txt"), "my.CON.txt");
        assert_eq!(cleanup_filename(".nul"), ".nul");
    }

    #[test]
    fn reserved_name_with_extension_from_url_is_escaped() {
        let fl = FileLink::new("https://example.com/CON.txt").unwrap();
        assert_eq!(fl.filename(), "CON_.txt");
    }

    #[test]
    fn cleanup_truncates_to_leave_room_for_the_part_suffix() {
        let cleaned = cleanup_filename(&"a".repeat(300));
        assert_eq!(cleaned.len(), 250);
        assert!(cleaned.len() + PART_SUFFIX.len() <= 255);
    }

    #[test]
    fn cleanup_truncation_keeps_a_short_extension() {
        let cleaned = cleanup_filename(&format!("{}.iso", "a".repeat(300)));
        assert_eq!(cleaned.len(), 250);
        assert!(cleaned.ends_with("aaa.iso"), "{cleaned}");
    }

    #[test]
    fn cleanup_truncation_does_not_keep_an_overlong_extension() {
        let cleaned = cleanup_filename(&format!("{}.{}", "a".repeat(300), "b".repeat(50)));
        assert_eq!(cleaned, "a".repeat(250));
    }

    #[test]
    fn cleanup_truncation_does_not_end_on_a_dot_or_space() {
        // cut at 250 lands right after "x. " - trimmed back to the "x"
        let name = format!("{}x. {}", "a".repeat(247), "b".repeat(100));
        assert_eq!(cleanup_filename(&name), format!("{}x", "a".repeat(247)));
    }

    #[test]
    fn long_url_name_and_its_part_file_fit_the_filesystem_limit() {
        let url = format!("https://example.com/{}.bin", "a".repeat(300));
        let name = FileLink::new(&url).unwrap().filename();
        assert!(name.ends_with(".bin"), "{name}");
        assert!(name.len() + PART_SUFFIX.len() <= 255, "{}", name.len());
    }

    #[test]
    fn cleanup_truncates_at_utf8_char_boundary() {
        // Each 'ä' is 2 bytes; 200 of them = 400 bytes, well over 250.
        let cleaned = cleanup_filename(&"ä".repeat(200));
        assert!(cleaned.len() <= 250);
        // String::truncate panics on a non-boundary, so this implicitly
        // confirms we landed on one — assert it explicitly anyway.
        assert!(cleaned.is_char_boundary(cleaned.len()));
    }

    #[test]
    fn new_errors_when_segment_collapses_to_empty() {
        // Last segment "..." is not a special path component so the URL
        // parser keeps it as-is, but cleanup trims trailing dots and the
        // result is empty.
        let url = "https://example.com/files/...";
        match FileLink::new(url) {
            Err(DlmError::Other { message }) => {
                assert!(
                    message.contains("could not derive a usable filename"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected Other error, got {other:?}"),
        }
    }

    #[test]
    fn new_errors_on_unparseable_url() {
        match FileLink::new("not-a-url") {
            Err(DlmError::Other { message }) => {
                assert!(
                    message.contains("could not parse URL"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected Other error, got {other:?}"),
        }
    }
}
