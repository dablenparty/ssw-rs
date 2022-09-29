use std::{error, fmt, io, num};

/// An error wrapper type used across the entire crate, usually where multiple error types are
/// returned.
#[derive(Debug)]
pub enum SswError {
    /// Raised when the second argument, the actual version string, is lower than the first argument,
    /// the minimum required version string.
    BadJavaVersion(String, String),
    IoError(io::Error),
    JsonError(serde_json::Error),
    LoggingError(log::SetLoggerError),
    MissingMinecraftVersion,
    ParseIntError(num::ParseIntError),
    ReqwestError(reqwest::Error),
}

pub type Result<T> = std::result::Result<T, SswError>;

impl fmt::Display for SswError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SswError::IoError(e) => write!(f, "IO error: {}", e),
            SswError::ReqwestError(e) => write!(f, "Reqwest error: {}", e),
            SswError::LoggingError(e) => write!(f, "Logging error: {}", e),
            SswError::BadJavaVersion(required, actual) => write!(
                f,
                "Java version '{}' is less than the required '{}'",
                actual, required
            ),
            SswError::MissingMinecraftVersion => write!(f, "Minecraft version not specified"),
            SswError::ParseIntError(e) => write!(f, "Parse error: {}", e),
            SswError::JsonError(e) => write!(f, "JSON error: {}", e),
        }
    }
}

impl error::Error for SswError {}

impl From<io::Error> for SswError {
    fn from(e: io::Error) -> Self {
        SswError::IoError(e)
    }
}

impl From<reqwest::Error> for SswError {
    fn from(e: reqwest::Error) -> Self {
        SswError::ReqwestError(e)
    }
}

impl From<log::SetLoggerError> for SswError {
    fn from(e: log::SetLoggerError) -> Self {
        SswError::LoggingError(e)
    }
}

impl From<num::ParseIntError> for SswError {
    fn from(e: num::ParseIntError) -> Self {
        SswError::ParseIntError(e)
    }
}

impl From<serde_json::Error> for SswError {
    fn from(e: serde_json::Error) -> Self {
        SswError::JsonError(e)
    }
}
