use crate::dlm_error::DlmError;

use crate::dlm_error::DlmError::Other;
use std::str;

pub struct FileLink {
    pub url: String,
    pub filename_without_extension: String,
    pub extension: Option<String>,
}

impl FileLink {
    pub fn new(url: String) -> Result<FileLink, DlmError> {
        let trimmed = url.trim();
        if trimmed.is_empty() {
            Err(Other {
                message: "FileLink cannot be built from an empty URL".to_string(),
            })
        } else if trimmed.ends_with('/') {
            let message = format!(
                "FileLink cannot be built with an invalid extension '{}'",
                trimmed
            );
            Err(Other { message })
        } else {
            let url_decoded = url_decode(url.as_str())?;
            let last_segment_rev: String = url_decoded
                .chars()
                .rev()
                .take_while(|c| c != &'/')
                .collect();
            // ideally the last_segment is the filename
            let last_segment = last_segment_rev.chars().rev().collect::<String>();
            let (extension, filename_without_extension) =
                Self::extract_extension_from_filename(last_segment);

            let file_link = FileLink {
                url,
                filename_without_extension,
                extension,
            };
            Ok(file_link)
        }
    }

    pub fn extract_extension_from_filename(filename: String) -> (Option<String>, String) {
        if filename.contains('.') {
            let after_dot_rev: String = filename.chars().rev().take_while(|c| c != &'.').collect();
            // remove potential query params
            let ext: String = after_dot_rev
                .chars()
                .rev()
                .take_while(|c| c != &'?')
                .collect();

            let tmp: String = filename
                .chars()
                .rev()
                .skip(after_dot_rev.len() + 1) // after_dot_rev to exclude query params and '+ 1' for the dot
                .collect();

            let filename_without_extension: String = tmp.chars().rev().collect();
            (Some(ext), filename_without_extension)
        } else {
            // no extension found, the file name will be used
            // sanitize as it contains query params
            // which are not allowed in filenames on some OS
            let sanitized = filename.replace(['?', '&'], "-");
            (None, sanitized)
        }
    }
}

// taken from https://github.com/bt/rust_urlencoding/blob/master/src/lib.rs#L20
fn url_decode(data: &str) -> Result<String, DlmError> {
    let mut unescaped_bytes: Vec<u8> = Vec::new();
    let mut bytes = data.bytes();
    // If validate_urlencoded_str returned Ok, then we know
    // every '%' is followed by 2 hex characters
    while let Some(b) = bytes.next() {
        match b as char {
            '%' => {
                let bytes_to_decode = &[bytes.next().unwrap(), bytes.next().unwrap()];
                let hex_str = str::from_utf8(bytes_to_decode).unwrap();
                unescaped_bytes.push(u8::from_str_radix(hex_str, 16).unwrap());
            }
            _ => {
                // Assume whoever did the encoding intended what we got
                unescaped_bytes.push(b);
            }
        }
    }
    String::from_utf8(unescaped_bytes).map_err(|e| DlmError::UrlDecodeError {
        message: e.to_string(),
    })
}

#[cfg(test)]
mod file_link_tests {
    use crate::file_link::*;

    #[test]
    fn no_empty_string() {
        match FileLink::new("".to_string()) {
            Err(Other { message }) => assert_eq!(
                message,
                "FileLink cannot be built from an empty URL".to_string()
            ),
            _ => assert_eq!(true, false),
        }
    }

    #[test]
    fn happy_case() {
        let url = "https://www.google.com/area51.txt".to_string();
        match FileLink::new(url.clone()) {
            Ok(fl) => {
                assert_eq!(fl.url, url);
                assert_eq!(fl.filename_without_extension, "area51".to_string());
                assert_eq!(fl.extension, Some("txt".to_string()));
            }
            _ => assert_eq!(true, false),
        }
    }

    #[test]
    fn trailing_slash() {
        let url = "https://www.google.com/area51/".to_string();
        match FileLink::new(url) {
            Err(Other { message }) => assert_eq!(
                message,
                "FileLink cannot be built with an invalid extension 'https://www.google.com/area51/'".to_string()
            ),
            _ => assert_eq!(true, false),
        }
    }

    #[test]
    fn no_extension() {
        let url = "https://www.google.com/area51".to_string();
        let fl = FileLink::new(url.clone()).unwrap();
        assert_eq!(fl.extension, None);
        assert_eq!(fl.filename_without_extension, "area51");
        assert_eq!(fl.url, url);
    }

    #[test]
    fn no_extension_use_query_params() {
        let url = "https://oeis.org/search?q=id:A000001&fmt=json".to_string();
        let fl = FileLink::new(url.clone()).unwrap();
        assert_eq!(fl.extension, None);
        assert_eq!(
            fl.filename_without_extension,
            "search-q=id:A000001-fmt=json"
        );
        assert_eq!(fl.url, url);
    }

    #[test]
    fn extract_extension_ok() {
        let (ext, filename) = FileLink::extract_extension_from_filename("area51.txt".to_string());
        assert_eq!(filename, "area51");
        assert_eq!(ext, Some("txt".to_string()));
    }

    #[test]
    fn extract_extension_with_query_param() {
        let url =
            "https://releases.ubuntu.com/21.10/ubuntu-21.10-desktop-amd64.iso?id=123".to_string();
        let fl = FileLink::new(url.clone()).unwrap();
        assert_eq!(fl.extension, Some("iso".to_string()));
        assert_eq!(fl.filename_without_extension, "ubuntu-21.10-desktop-amd64");
        assert_eq!(fl.url, url);
    }

    #[test]
    fn extract_extension_with_query_param_bis() {
        let url = "https://atom-installer.github.com/v1.58.0/atom-amd64.deb?s=1627025597&ext=.deb"
            .to_string();
        let fl = FileLink::new(url.clone()).unwrap();
        assert_eq!(fl.extension, Some("deb".to_string()));
        assert_eq!(fl.url, url);
        // FIXME
        //assert_eq!(fl.filename_without_extension, "atom-amd64");
    }
}
