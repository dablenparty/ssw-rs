use std::{error, fmt, io, num};

/// An error wrapper type used across the entire crate, usually where multiple error types are
/// returned.
#[allow(clippy::enum_variant_names)]
#[derive(Debug)]
pub enum Error {
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

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::IoError(e) => write!(f, "IO error: {}", e),
            Error::ReqwestError(e) => write!(f, "Reqwest error: {}", e),
            Error::LoggingError(e) => write!(f, "Logging error: {}", e),
            Error::BadJavaVersion(required, actual) => write!(
                f,
                "Java version '{}' is less than the required '{}'",
                actual, required
            ),
            Error::MissingMinecraftVersion => write!(f, "Minecraft version not specified"),
            Error::ParseIntError(e) => write!(f, "Parse error: {}", e),
            Error::JsonError(e) => write!(f, "JSON error: {}", e),
        }
    }
}

impl error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::IoError(e)
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::ReqwestError(e)
    }
}

impl From<log::SetLoggerError> for Error {
    fn from(e: log::SetLoggerError) -> Self {
        Error::LoggingError(e)
    }
}

impl From<num::ParseIntError> for Error {
    fn from(e: num::ParseIntError) -> Self {
        Error::ParseIntError(e)
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::JsonError(e)
    }
}
