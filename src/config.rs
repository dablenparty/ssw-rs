use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

use getset::{Getters, MutGetters, Setters};
use log::warn;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SswConfigError {
    #[error("Failed to parse config: {0}")]
    ParseError(#[from] toml::de::Error),
    #[error("Failed to serialize config: {0}")]
    SerializeError(#[from] toml::ser::Error),
    #[error("Failed to read config: {0}")]
    IoError(#[from] std::io::Error),
    #[error("mc_version is not set in config")]
    MissingMinecraftVersion,
}

#[derive(Debug, Clone, Deserialize, Serialize, Getters, MutGetters, Setters)]
#[getset(get = "pub", get_mut = "pub", set = "pub")]
pub struct SswConfig<'s> {
    /// Extra JVM arguments to pass to the server
    extra_jvm_args: Cow<'s, Vec<String>>,
    /// The minimum amount of memory to allocate to the server in MB
    min_memory_in_mb: usize,
    /// The maximum amount of memory to allocate to the server in MB
    max_memory_in_mb: usize,
    /// The version of Minecraft to run. This is used to determine how to patch Log4Shell.
    mc_version: Option<Cow<'s, str>>,
    /// The number of hours to wait before restarting the server (set to 0 to disable)
    restart_after_hrs: f32,
    /// The number of minutes to wait before shutting down the server (set to 0 to disable)
    shutdown_after_mins: f32,
    /// Whether to automatically start the server when it is stopped and the listener receives a connection
    auto_start: bool,
    /// The path to the Java executable to use
    java_path: String,
}

impl Default for SswConfig<'_> {
    fn default() -> Self {
        Self {
            extra_jvm_args: Cow::Owned(vec![
                "-XX:+UnlockExperimentalVMOptions".into(),
                "-XX:+UseG1GC".into(),
                "-XX:G1NewSizePercent=20".into(),
                "-XX:G1ReservePercent=20".into(),
                "-XX:MaxGCPauseMillis=50".into(),
                "-XX:G1HeapRegionSize=32M".into(),
            ]),
            min_memory_in_mb: 256,
            max_memory_in_mb: 1024,
            mc_version: None,
            restart_after_hrs: 12.0,
            shutdown_after_mins: 5.0,
            auto_start: true,
            java_path: which::which("java").map_or_else(
                |e| {
                    warn!("Failed to find java in PATH: {e}");
                    "java".into()
                },
                |p| p.to_string_lossy().into_owned(),
            ),
        }
    }
}

impl TryFrom<PathBuf> for SswConfig<'_> {
    type Error = SswConfigError;

    fn try_from(path: PathBuf) -> Result<Self, Self::Error> {
        Self::try_from(path.as_path())
    }
}

impl TryFrom<&Path> for SswConfig<'_> {
    type Error = SswConfigError;

    fn try_from(path: &Path) -> Result<Self, Self::Error> {
        let config = std::fs::read_to_string(path)?;
        let config = toml::from_str(&config)?;
        Ok(config)
    }
}

impl<'s> SswConfig<'s> {
    pub async fn save(&self, path: &Path) -> Result<(), SswConfigError> {
        let config = toml::to_string_pretty(self)?;
        tokio::fs::write(path, config).await?;
        Ok(())
    }
}
