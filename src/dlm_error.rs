use tokio::task::JoinError;
use tokio::time::error::Elapsed;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DlmError {
    #[error("connection closed")]
    ConnectionClosed,
    #[error("connection timeout")]
    ConnectionTimeout,
    #[error("response body error")]
    ResponseBodyError,
    #[error("deadline elapsed timeout")]
    DeadLineElapsedTimeout,
    #[error("response status not success ({message:?})")]
    ResponseStatusNotSuccess { message: String },
    #[error("URL decode error ({message:?})")]
    UrlDecodeError { message: String },
    #[error("standard I/O error ({e:?})")]
    StdIoError { e: std::io::Error },
    #[error("task error ({e:?})")]
    TaskError { e: JoinError },
    #[error("channel error ({e:?})")]
    ChannelError { e: async_channel::RecvError },
    #[error("other error ({message:?})")]
    Other { message: String },
}

const CONNECTION_CLOSED: &str = "connection closed before message completed";
const CONNECTION_TIMEOUT: &str = "error trying to connect: operation timed out";
const BODY_ERROR: &str = "error reading a body from connection";

impl std::convert::From<reqwest::Error> for DlmError {
    fn from(e: reqwest::Error) -> Self {
        //TODO use Reqwest's types instead of guessing from strings https://github.com/seanmonstar/reqwest/issues/757
        let e_string = e.to_string();
        if e_string.contains(BODY_ERROR) {
            DlmError::ResponseBodyError
        } else if e_string.contains(CONNECTION_CLOSED) {
            DlmError::ConnectionClosed
        } else if e_string.contains(CONNECTION_TIMEOUT) {
            DlmError::ConnectionTimeout
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

impl std::convert::From<Elapsed> for DlmError {
    fn from(_: Elapsed) -> Self {
        DlmError::DeadLineElapsedTimeout
    }
}

impl std::convert::From<JoinError> for DlmError {
    fn from(e: JoinError) -> Self {
        DlmError::TaskError { e }
    }
}

impl std::convert::From<async_channel::RecvError> for DlmError {
    fn from(e: async_channel::RecvError) -> Self {
        DlmError::ChannelError { e }
    }
}
