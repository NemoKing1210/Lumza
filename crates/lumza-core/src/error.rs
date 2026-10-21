use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("monitor not found: {0}")]
    NotFound(String),
    #[error("VCP code 0x{0:02X} is not supported by this monitor")]
    Unsupported(u8),
    #[error("monitor did not respond: {0}")]
    Timeout(String),
    #[error("win32 error: {0}")]
    Os(String),
    #[error("invalid input: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;
