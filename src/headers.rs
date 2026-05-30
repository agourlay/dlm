use percent_encoding::percent_decode_str;
use reqwest::header::{
    ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, HeaderMap, LOCATION,
};
use std::path::Path;

use crate::file_link::cleanup_filename;

/// Extract total size from Content-Range header: `bytes 0-0/12345` → 12345
pub fn content_range_total_size(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit_once('/'))
        .and_then(|(_, total)| total.parse().ok())
}

pub fn content_length_value(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
}

pub fn supports_range_bytes(headers: &HeaderMap) -> bool {
    headers
        .get(ACCEPT_RANGES)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("bytes"))
}

pub fn content_disposition_value(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
}

pub fn location_value(headers: &HeaderMap) -> Option<&str> {
    headers.get(LOCATION).and_then(|v| v.to_str().ok())
}

/// Pull `(content-length, supports range, disposition filename)` out of a
/// response's headers. Prefers `Content-Range` total over `Content-Length`
/// because a `Range: bytes=0-0` probe makes `Content-Length` equal to 1.
pub fn parse_metadata_from(headers: &HeaderMap) -> (Option<u64>, bool, Option<String>) {
    let cl = content_range_total_size(headers).or_else(|| content_length_value(headers));
    let sr = supports_range_bytes(headers);
    let df = content_disposition_value(headers).and_then(parse_filename_header);
    (cl, sr, df)
}

pub fn parse_filename_header(content_disposition: &str) -> Option<String> {
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

/// Strip path components to prevent directory traversal attacks (a malicious
/// server could send `Content-Disposition: attachment; filename="../../etc/evil"`)
/// then run the standard filename cleanup so forbidden chars, control chars,
/// trailing dots/whitespace, and Windows reserved names are all neutralised.
fn sanitize_filename(name: &str) -> Option<String> {
    let file_name = Path::new(name).file_name()?.to_str()?;
    let cleaned = cleanup_filename(file_name);
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
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

/// Percent-decode a `filename*=` parameter value (lossy on invalid UTF-8).
fn percent_decode_filename(input: &str) -> String {
    percent_decode_str(input).decode_utf8_lossy().into_owned()
}

#[cfg(test)]
mod headers_tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn headers_with(name: reqwest::header::HeaderName, value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(name, HeaderValue::from_str(value).unwrap());
        h
    }

    #[test]
    fn supports_range_bytes_mixed_case() {
        let h = headers_with(ACCEPT_RANGES, "Bytes");
        assert!(supports_range_bytes(&h));
    }

    #[test]
    fn supports_range_bytes_other_unit() {
        let h = headers_with(ACCEPT_RANGES, "none");
        assert!(!supports_range_bytes(&h));
    }

    #[test]
    fn supports_range_bytes_missing_header() {
        assert!(!supports_range_bytes(&HeaderMap::new()));
    }

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
    fn parse_filename_header_strips_forbidden_chars() {
        let header = "attachment; filename=\"weird:name*?.txt\"";
        assert_eq!(
            parse_filename_header(header),
            Some("weird_name__.txt".to_owned())
        );
    }

    #[test]
    fn parse_filename_header_trims_trailing_dot() {
        let header = "attachment; filename=\"report.\"";
        assert_eq!(parse_filename_header(header), Some("report".to_owned()));
    }

    #[test]
    fn parse_filename_header_windows_reserved_name_escaped() {
        let header = "attachment; filename=\"CON\"";
        assert_eq!(parse_filename_header(header), Some("CON_".to_owned()));
    }

    #[test]
    fn parse_filename_header_dots_only_is_none() {
        let header = "attachment; filename=\"...\"";
        assert_eq!(parse_filename_header(header), None);
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
