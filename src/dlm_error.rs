#[derive(Debug)]
pub struct DlmError {
    pub message: String,
}

impl std::convert::From<reqwest::Error> for DlmError {
    fn from(e: reqwest::Error) -> Self {
        DlmError {
            message: e.to_string(),
        }
    }
}

impl std::convert::From<std::io::Error> for DlmError {
    fn from(e: std::io::Error) -> Self {
        DlmError {
            message: e.to_string(),
        }
    }
}
