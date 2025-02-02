use thiserror::Error;
use tokio::task::JoinError;
use tokio::time::error::Elapsed;

#[derive(Error, Debug)]
pub enum DlmError {
    #[error("the input file is empty")]
    EmptyInputFile,
    #[error("connection closed")]
    ConnectionClosed,
    #[error("connection timeout")]
    ConnectionTimeout,
    #[error("response body error")]
    ResponseBodyError,
    #[error("deadline elapsed timeout")]
    DeadLineElapsedTimeout,
    #[error("response status not success - {status_code:?}")]
    ResponseStatusNotSuccess { status_code: String },
    #[error("URL decode error - {message:?}")]
    UrlDecodeError { message: String },
    #[error("standard I/O error - {e}")]
    StdIoError { e: std::io::Error },
    #[error("task error - {e}")]
    TaskError { e: JoinError },
    #[error("channel error - {e}")]
    ChannelError { e: async_channel::RecvError },
    #[error("CLI argument error - {message:?}")]
    CliArgumentError { message: String },
    #[error("CLI argument error ({e})")]
    ClapError { e: clap::Error },
    #[error("Program interrupted")]
    ProgramInterrupted,
    #[error("other error - {message:?}")]
    Other { message: String },
}

const CONNECTION_CLOSED: &str = "connection closed before message completed";
const CONNECTION_TIMEOUT: &str = "error trying to connect: operation timed out";
const BODY_ERROR: &str = "error reading a body from connection";

impl From<reqwest::Error> for DlmError {
    fn from(e: reqwest::Error) -> Self {
        //TODO use Reqwest's types instead of guessing from strings https://github.com/seanmonstar/reqwest/issues/757
        let e_string = e.to_string();
        if e_string.contains(BODY_ERROR) {
            Self::ResponseBodyError
        } else if e_string.contains(CONNECTION_CLOSED) {
            Self::ConnectionClosed
        } else if e_string.contains(CONNECTION_TIMEOUT) {
            Self::ConnectionTimeout
        } else {
            Self::Other { message: e_string }
        }
    }
}

impl From<std::io::Error> for DlmError {
    fn from(e: std::io::Error) -> Self {
        Self::StdIoError { e }
    }
}

impl From<Elapsed> for DlmError {
    fn from(_: Elapsed) -> Self {
        Self::DeadLineElapsedTimeout
    }
}

impl From<JoinError> for DlmError {
    fn from(e: JoinError) -> Self {
        Self::TaskError { e }
    }
}

impl From<async_channel::RecvError> for DlmError {
    fn from(e: async_channel::RecvError) -> Self {
        Self::ChannelError { e }
    }
}

impl From<clap::Error> for DlmError {
    fn from(e: clap::Error) -> Self {
        Self::ClapError { e }
    }
}
