use thiserror::Error;

#[derive(Debug, Error)]
pub enum PlayerError {
    #[error("clip not found: {0}")]
    NotFound(String),
    #[error("decode failed: {0}")]
    DecodeFailed(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("io error: {0}")]
    Io(String),
}
