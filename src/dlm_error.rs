use thiserror::Error;
use tokio::task::JoinError;
use tokio::time::error::Elapsed;

#[derive(Error, Debug)]
pub enum DlmError {
    #[error("the input file is empty")]
    EmptyInputFile,
    #[error("connection error")]
    ConnectError,
    #[error("connection timeout")]
    ConnectionTimeout,
    #[error("response body error")]
    ResponseBodyError,
    #[error("deadline elapsed timeout")]
    DeadLineElapsedTimeout,
    #[error("response status not success - {status_code}")]
    ResponseStatusNotSuccess { status_code: u16 },
    #[error("URL decode error - {message}")]
    UrlDecodeError { message: String },
    #[error("standard I/O error - {e}")]
    StdIoError { e: std::io::Error },
    #[error("task error - {e}")]
    TaskError { e: JoinError },
    #[error("channel error - {e}")]
    ChannelError { e: async_channel::RecvError },
    #[error("CLI argument error - {message}")]
    CliArgumentError { message: String },
    #[error("CLI argument error ({e})")]
    ClapError { e: clap::Error },
    #[error("Program interrupted")]
    ProgramInterrupted,
    #[error("other error - {message}")]
    Other { message: String },
}

impl DlmError {
    pub fn other(message: String) -> Self {
        Self::Other { message }
    }
}

impl From<reqwest::Error> for DlmError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            Self::ConnectionTimeout
        } else if e.is_connect() {
            Self::ConnectError
        } else if e.is_body() {
            Self::ResponseBodyError
        } else {
            Self::other(e.to_string())
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
