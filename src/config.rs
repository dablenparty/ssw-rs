use std::{borrow::Cow, path::Path};

use getset::{Getters, MutGetters, Setters};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SswConfigError {
    #[error("Failed to parse config: {0}")]
    ParseError(#[from] toml::de::Error),
    #[error("Failed to read config: {0}")]
    IoError(#[from] std::io::Error),
}

#[derive(Debug, Clone, Deserialize, Serialize, Getters, MutGetters, Setters)]
#[getset(get = "pub", get_mut = "pub", set = "pub")]
pub struct SswConfig<'s> {
    /// Extra JVM arguments to pass to the server
    extra_jvm_args: Cow<'s, Vec<String>>,
    /// The amount of memory to allocate to the server in MB
    memory_in_mb: usize,
    /// The version of Minecraft to run. This is used to determine how to patch Log4Shell.
    mc_version: Option<Cow<'s, str>>,
    /// The number of hours to wait before restarting the server (set to 0 to disable)
    restart_after_hrs: f32,
    /// The number of minutes to wait before shutting down the server (set to 0 to disable)
    shutdown_after_mins: f32,
}

impl Default for SswConfig<'_> {
    fn default() -> Self {
        Self {
            extra_jvm_args: Cow::Owned(vec![]),
            memory_in_mb: 1024,
            mc_version: None,
            restart_after_hrs: 12.0,
            shutdown_after_mins: 5.0,
        }
    }
}

impl<'s> SswConfig<'s> {
    /// Attempts to asynchronously load a config from the given path.
    ///
    /// # Arguments
    ///
    /// * `path` - The path-like to load the config from
    ///
    /// # Errors
    ///
    /// This function will return an error if the file cannot be read or if the file cannot be parsed.
    pub async fn try_from_path<P: AsRef<Path>>(path: P) -> Result<SswConfig<'s>, SswConfigError> {
        let path = path.as_ref();
        let config = tokio::fs::read_to_string(path).await?;
        let config = toml::from_str(&config)?;
        Ok(config)
    }
}
