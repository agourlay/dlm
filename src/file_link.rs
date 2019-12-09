use crate::dlm_error::DlmError;

use std::str;

pub struct FileLink {
    pub url: String,
    pub file_name_no_extension: String,
    pub extension: String,
    pub file_name: String,
}

impl FileLink {
    pub fn new(url: String) -> Result<FileLink, DlmError> {
        let extension: String = {
            let tmp: String = url.chars().rev().take_while(|c| c != &'.').collect();
            tmp.chars().rev().collect()
        };

        let file_name_no_extension: String = {
            let tmp: String = url_decode(url.as_str())?
                .chars()
                .rev()
                .skip(extension.len())
                .take_while(|c| c != &'/')
                .collect();
            tmp.chars().rev().collect()
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

    pub fn full_path(&self, output_dir: &str) -> String {
        format!(
            "{}/{}{}",
            output_dir, self.file_name_no_extension, self.extension
        )
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
    String::from_utf8(unescaped_bytes).or_else(|e| {
        Err(DlmError {
            message: e.to_string(),
        })
    })
}
