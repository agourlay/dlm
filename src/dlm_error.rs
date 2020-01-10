#[derive(Debug)]
pub struct DlmError {
    // TODO make a nice type safe enum
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

impl std::convert::From<tokio::time::Elapsed> for DlmError {
    fn from(e: tokio::time::Elapsed) -> Self {
        DlmError {
            message: e.to_string(),
        }
    }
}