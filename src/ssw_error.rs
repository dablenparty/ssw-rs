use std::{env, error, fmt, io, num};

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
    TomlDeserializeError(toml::de::Error),
    TomlSerializeError(toml::ser::Error),
    EnvVarError(env::VarError),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BadJavaVersion(required, actual) => write!(
                f,
                "Java version '{}' is less than the required '{}'",
                actual, required
            ),
            Error::EnvVarError(e) => write!(f, "Environment variable error: {}", e),
            Error::IoError(e) => write!(f, "IO error: {}", e),
            Error::JsonError(e) => write!(f, "JSON error: {}", e),
            Error::LoggingError(e) => write!(f, "Logging error: {}", e),
            Error::MissingMinecraftVersion => write!(f, "Minecraft version not specified"),
            Error::ParseIntError(e) => write!(f, "Parse error: {}", e),
            Error::ReqwestError(e) => write!(f, "Reqwest error: {}", e),
            Error::TomlDeserializeError(e) => write!(f, "TOML deserialization error: {}", e),
            Error::TomlSerializeError(e) => write!(f, "TOML serialization error: {}", e),
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

impl From<toml::de::Error> for Error {
    fn from(e: toml::de::Error) -> Self {
        Error::TomlDeserializeError(e)
    }
}

impl From<toml::ser::Error> for Error {
    fn from(e: toml::ser::Error) -> Self {
        Error::TomlSerializeError(e)
    }
}

impl From<env::VarError> for Error {
    fn from(e: env::VarError) -> Self {
        Error::EnvVarError(e)
    }
}
