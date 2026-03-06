use reqwest::header::{
    ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, HeaderMap, LOCATION,
};

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
        .is_some_and(|v| v == "bytes")
}

pub fn content_disposition_value(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
}

pub fn location_value(headers: &HeaderMap) -> Option<&str> {
    headers.get(LOCATION).and_then(|v| v.to_str().ok())
}
