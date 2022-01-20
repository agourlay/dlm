use crate::dlm_error::DlmError;

use crate::dlm_error::DlmError::Other;
use std::str;

pub struct FileLink {
    pub url: String,
    pub file_name_no_extension: String,
    pub extension: String,
    pub file_name: String,
}

const NO_EXT: &str = ".NO_EXT";

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
            let (extension, file_name_no_extension) = if last_segment_rev.contains('.') {
                let ext_rev: String = last_segment_rev.chars().take_while(|c| c != &'.').collect();
                let ext: String = ext_rev.chars().rev().collect();

                let tmp: String = url_decoded
                    .chars()
                    .rev()
                    .skip(ext.len())
                    .take_while(|c| c != &'/')
                    .collect();
                let file_name_no_extension: String = tmp.chars().rev().collect();
                (ext, file_name_no_extension)
            } else {
                let file_name_no_extension: String = last_segment_rev.chars().rev().collect();
                // no extension detected - give it a fake one
                (NO_EXT.to_string(), file_name_no_extension)
            };

            let file_name = format!("{}{}", file_name_no_extension, extension);
            let file_link = FileLink {
                url,
                file_name_no_extension,
                extension,
                file_name,
            };
            Ok(file_link)
        }
    }

    pub fn full_path(&self, output_dir: &str) -> String {
        format!("{}/{}", output_dir, self.file_name)
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
    use crate::dlm_error::DlmError;
    use crate::file_link::*;

    #[test]
    fn no_empty_string() {
        match FileLink::new("".to_string()) {
            Err(DlmError::Other { message }) => assert_eq!(
                message,
                "FileLink cannot be built from an empty URL".to_string()
            ),
            _ => assert_eq!(true, false),
        }
    }

    #[test]
    fn happy_case() {
        let url = "http://www.google.com/area51.txt".to_string();
        match FileLink::new(url.clone()) {
            Ok(fl) => {
                assert_eq!(fl.url, url);
                assert_eq!(fl.file_name, "area51.txt".to_string());
                assert_eq!(fl.extension, "txt".to_string());
                assert_eq!(fl.file_name_no_extension, "area51.".to_string());
            }
            _ => assert_eq!(true, false),
        }
    }

    #[test]
    fn full_path() {
        let url = "http://www.google.com/area51.txt".to_string();
        let fl = FileLink::new(url).unwrap();
        let full_path = fl.full_path("/secret-folder");
        assert_eq!(full_path, "/secret-folder/area51.txt".to_string())
    }

    #[test]
    fn trailing_slash() {
        let url = "http://www.google.com/area51/".to_string();
        match FileLink::new(url.clone()) {
            Err(DlmError::Other { message }) => assert_eq!(
                message,
                "FileLink cannot be built with an invalid extension 'http://www.google.com/area51/'".to_string()
            ),
            _ => assert_eq!(true, false),
        }
    }

    #[test]
    fn no_extension() {
        let url = "http://www.google.com/area51".to_string();
        let fl = FileLink::new(url).unwrap();
        let full_path = fl.full_path("/secret-folder");
        assert_eq!(full_path, "/secret-folder/area51.NO_EXT".to_string())
    }
}
