use percent_encoding::percent_decode_str;
use reqwest::StatusCode;
use reqwest::header::{
    ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, HeaderMap, LOCATION,
};
use std::path::Path;

use crate::file_link::cleanup_filename;

/// Total size from a `Content-Range` header: `bytes 0-0/12345` → 12345.
/// `None` when the header is missing or the total is unknown (`bytes 0-0/*`).
pub fn content_range_total_size(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit_once('/'))
        .and_then(|(_, total)| total.trim().parse().ok())
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

/// Pull `(content-length, supports range, disposition filename)` out of the
/// response to a `Range: bytes=0-0` probe.
///
/// A partial answer (`206`, or any response carrying `Content-Range`) takes
/// its size from the `Content-Range` total only: its `Content-Length` is the
/// size of the one-byte slice, so a total the server leaves unknown
/// (`bytes 0-0/*`) stays unknown. A `206` also proves range support on its
/// own, whether or not `Accept-Ranges` is sent along. A server that ignored
/// the range answers `200` with the whole body, whose `Content-Length` is the
/// real size.
pub fn parse_metadata_from(
    status: StatusCode,
    headers: &HeaderMap,
) -> (Option<u64>, bool, Option<String>) {
    let partial = status == StatusCode::PARTIAL_CONTENT;
    let length = if partial || headers.contains_key(CONTENT_RANGE) {
        content_range_total_size(headers)
    } else {
        content_length_value(headers)
    };
    let supports_range = partial || supports_range_bytes(headers);
    let disposition = content_disposition_value(headers).and_then(parse_filename_header);
    (length, supports_range, disposition)
}

/// The file name a `Content-Disposition` header suggests, sanitized.
///
/// The RFC 6266 `filename*` parameter (RFC 8187 encoded, e.g.
/// `UTF-8''my%20file.txt` or `UTF-8'en'my%20file.txt`) wins over the plain
/// `filename` one, which is still used when `filename*` is missing or cannot
/// be decoded.
pub fn parse_filename_header(content_disposition: &str) -> Option<String> {
    let params = header_params(content_disposition);
    let param = |name: &str| {
        params
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    };
    if let Some(name) = param("filename*")
        .and_then(decode_ext_value)
        .and_then(|decoded| sanitize_filename(&decoded))
    {
        return Some(name);
    }
    param("filename").and_then(sanitize_filename)
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

/// The `name=value` parameters of a header value such as
/// `attachment; filename="a;b.txt"`, names lowercased, values unquoted.
///
/// Quoted values may hold `;` and backslash-escaped characters (RFC 9110
/// quoted-string), so the value is read up to its closing quote rather than
/// up to the next `;`. Segments without `=` - the disposition type itself -
/// are skipped.
fn header_params(header: &str) -> Vec<(String, String)> {
    let mut params = Vec::new();
    let mut chars = header.chars().peekable();
    while chars.peek().is_some() {
        let mut name = String::new();
        let mut has_value = false;
        for c in chars.by_ref() {
            match c {
                '=' => {
                    has_value = true;
                    break;
                }
                ';' => break,
                _ => name.push(c),
            }
        }
        if !has_value {
            continue;
        }
        while chars.next_if(|c| c.is_whitespace()).is_some() {}

        let mut value = String::new();
        if chars.next_if_eq(&'"').is_some() {
            while let Some(c) = chars.next() {
                match c {
                    '\\' => value.extend(chars.next()),
                    '"' => break,
                    _ => value.push(c),
                }
            }
            // whatever trails the closing quote, up to the next parameter
            for c in chars.by_ref() {
                if c == ';' {
                    break;
                }
            }
        } else {
            for c in chars.by_ref() {
                if c == ';' {
                    break;
                }
                value.push(c);
            }
            value = value.trim().to_string();
        }
        params.push((name.trim().to_ascii_lowercase(), value));
    }
    params
}

/// Decode an RFC 8187 extended value: `charset'language'percent-encoded`.
/// The language tag is optional and ignored. UTF-8 and ISO-8859-1 - the two
/// charsets the RFC names - are supported; any other gives `None` so the
/// plain `filename` parameter is used instead. Invalid UTF-8 is decoded
/// lossily.
fn decode_ext_value(value: &str) -> Option<String> {
    let mut parts = value.splitn(3, '\'');
    let charset = parts.next()?.trim();
    let _language = parts.next()?;
    let bytes: Vec<u8> = percent_decode_str(parts.next()?).collect();
    let decoded = if charset.eq_ignore_ascii_case("utf-8") {
        String::from_utf8_lossy(&bytes).into_owned()
    } else if charset.eq_ignore_ascii_case("iso-8859-1") {
        // ISO-8859-1 bytes are the first 256 Unicode code points
        bytes.iter().map(|&b| char::from(b)).collect()
    } else {
        return None;
    };
    (!decoded.is_empty()).then_some(decoded)
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
    fn ext_value_percent_decoding() {
        assert_eq!(
            decode_ext_value("UTF-8''my%20file.txt").unwrap(),
            "my file.txt"
        );
        // malformed escapes are kept as is
        assert_eq!(decode_ext_value("UTF-8''file%").unwrap(), "file%");
        assert_eq!(decode_ext_value("UTF-8''file%2").unwrap(), "file%2");
        assert_eq!(decode_ext_value("UTF-8''file%GG").unwrap(), "file%GG");
    }

    #[test]
    fn ext_value_with_language_tag() {
        assert_eq!(
            decode_ext_value("UTF-8'en'na%C3%AFve.txt").unwrap(),
            "na\u{ef}ve.txt"
        );
    }

    #[test]
    fn ext_value_iso_8859_1() {
        // 0xE9 is 'é' in ISO-8859-1, and invalid on its own in UTF-8
        assert_eq!(
            decode_ext_value("iso-8859-1''caf%E9.txt").unwrap(),
            "caf\u{e9}.txt"
        );
    }

    #[test]
    fn ext_value_unknown_charset_or_shape_is_none() {
        assert_eq!(decode_ext_value("koi8-r''file.txt"), None);
        assert_eq!(decode_ext_value("file.txt"), None);
        assert_eq!(decode_ext_value("UTF-8''"), None);
    }

    #[test]
    fn parse_filename_header_star_with_language_tag() {
        let header = "attachment; filename*=UTF-8'en'my%20report.pdf";
        assert_eq!(
            parse_filename_header(header),
            Some("my report.pdf".to_owned())
        );
    }

    #[test]
    fn parse_filename_header_star_iso_8859_1() {
        let header = "attachment; filename*=ISO-8859-1''r%E9sum%E9.pdf";
        assert_eq!(
            parse_filename_header(header),
            Some("r\u{e9}sum\u{e9}.pdf".to_owned())
        );
    }

    /// An undecodable `filename*` must not hide a usable `filename`.
    #[test]
    fn parse_filename_header_falls_back_when_star_is_undecodable() {
        let header = "attachment; filename*=koi8-r''x.txt; filename=\"plain.txt\"";
        assert_eq!(parse_filename_header(header), Some("plain.txt".to_owned()));
    }

    #[test]
    fn parse_filename_header_semicolon_inside_quotes() {
        let header = "attachment; filename=\"a;b.txt\"; size=10";
        assert_eq!(parse_filename_header(header), Some("a;b.txt".to_owned()));
    }

    #[test]
    fn parse_filename_header_escaped_quote_inside_quotes() {
        // the escaped quote is a literal `"`, sanitized to `_`
        let header = r#"attachment; filename="say \"hi\".txt""#;
        assert_eq!(
            parse_filename_header(header),
            Some("say _hi_.txt".to_owned())
        );
    }

    #[test]
    fn parse_filename_header_param_name_must_match_exactly() {
        let header = "attachment; myfilename=\"wrong.txt\"; filename=\"right.txt\"";
        assert_eq!(parse_filename_header(header), Some("right.txt".to_owned()));
    }

    #[test]
    fn parse_filename_header_param_name_is_case_insensitive() {
        let header = "attachment; FileName=\"upper.txt\"";
        assert_eq!(parse_filename_header(header), Some("upper.txt".to_owned()));
    }

    #[test]
    fn parse_filename_header_without_disposition_type() {
        assert_eq!(
            parse_filename_header("filename=\"bare.txt\""),
            Some("bare.txt".to_owned())
        );
    }

    fn probe_headers(pairs: &[(reqwest::header::HeaderName, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (name, value) in pairs {
            h.insert(name.clone(), HeaderValue::from_str(value).unwrap());
        }
        h
    }

    #[test]
    fn probe_206_takes_the_content_range_total() {
        let h = probe_headers(&[
            (CONTENT_RANGE, "bytes 0-0/12345"),
            (CONTENT_LENGTH, "1"),
            (ACCEPT_RANGES, "bytes"),
        ]);
        let (len, range, _) = parse_metadata_from(StatusCode::PARTIAL_CONTENT, &h);
        assert_eq!(len, Some(12345));
        assert!(range);
    }

    /// `bytes 0-0/*`: the server does not know the total. The one-byte
    /// `Content-Length` of the slice must not be taken for the file size.
    #[test]
    fn probe_206_with_unknown_total_has_unknown_length() {
        let h = probe_headers(&[(CONTENT_RANGE, "bytes 0-0/*"), (CONTENT_LENGTH, "1")]);
        let (len, _, _) = parse_metadata_from(StatusCode::PARTIAL_CONTENT, &h);
        assert_eq!(len, None);
    }

    /// A 206 proves range support even without `Accept-Ranges`.
    #[test]
    fn probe_206_supports_range_without_accept_ranges() {
        let h = probe_headers(&[(CONTENT_RANGE, "bytes 0-0/100"), (CONTENT_LENGTH, "1")]);
        let (len, range, _) = parse_metadata_from(StatusCode::PARTIAL_CONTENT, &h);
        assert_eq!(len, Some(100));
        assert!(range);
    }

    /// A server that ignored the range sends the whole body with `200`.
    #[test]
    fn probe_200_takes_the_content_length() {
        let h = probe_headers(&[(CONTENT_LENGTH, "100")]);
        let (len, range, _) = parse_metadata_from(StatusCode::OK, &h);
        assert_eq!(len, Some(100));
        assert!(!range);

        let h = probe_headers(&[(CONTENT_LENGTH, "100"), (ACCEPT_RANGES, "bytes")]);
        let (_, range, _) = parse_metadata_from(StatusCode::OK, &h);
        assert!(range);
    }
}
