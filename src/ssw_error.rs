use thiserror::Error;

#[derive(Debug, Error)]
pub enum SswError {
    #[error("SswConfigError: {0}")]
    SswConfigError(#[from] crate::config::SswConfigError),
    #[error("IoError: {0}")]
    IoError(#[from] std::io::Error),
}
