#[derive(Debug)]
pub enum DlmError {
    ConnectionClosed,
    ResponseBodyError,
    DeadLineElapsedTimeout,
    ResponseStatusNotSuccess { message: String },
    UrlDecodeError { message: String },
    StdIoError { e: std::io::Error },
    Other { message: String },
}

const CONNECTION_CLOSED: &str = "connection closed before message completed";
const BODY_ERROR: &str = "error reading a body from connection";

impl std::convert::From<reqwest::Error> for DlmError {
    fn from(e: reqwest::Error) -> Self {
        //TODO use Reqwest's types instead of guessing from strings https://github.com/seanmonstar/reqwest/issues/757
        let e_string = e.to_string();
        if e_string.contains(BODY_ERROR) {
            DlmError::ResponseBodyError
        } else if e_string.contains(CONNECTION_CLOSED) {
            DlmError::ConnectionClosed
        } else {
            DlmError::Other { message: e_string }
        }
    }
}

impl std::convert::From<std::io::Error> for DlmError {
    fn from(e: std::io::Error) -> Self {
        DlmError::StdIoError { e }
    }
}

impl std::convert::From<tokio::time::Elapsed> for DlmError {
    fn from(_: tokio::time::Elapsed) -> Self {
        DlmError::DeadLineElapsedTimeout
    }
}
