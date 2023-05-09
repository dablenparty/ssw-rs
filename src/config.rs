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
    mc_version: Option<String>,
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
    /// Attempts to load the config from the given path. If the file does not
    /// exist, a default config is created and saved to the given path. This
    /// function will also attempt to read the Minecraft version from the
    /// jar files in the parent folder of the given path.
    ///
    /// # Arguments
    ///
    /// * `path` - The path to the config file
    ///
    /// # Errors
    ///
    /// This function will return an error if the config file cannot be read or
    /// parsed, or if the Minecraft version cannot be read from the jar files.
    ///
    /// # Panics
    ///
    /// This function will panic if the parent folder of the given path does
    /// not exist.
    pub async fn load(path: &Path) -> Result<SswConfig<'s>, SswConfigError> {
        if path.exists() {
            Self::try_from(path)
        } else {
            let mut config = Self::default();
            let parent = path.parent().unwrap();
            let mc_version = try_read_version_from_folder(parent).await?;
            config.set_mc_version(mc_version);
            // save config
            let toml_string = toml::to_string_pretty(&config)?;
            tokio::fs::write(path, toml_string).await?;
            Ok(config)
        }
    }
}

/// Tries to read the version string from the given folder.
///
/// All Minecraft versions `1.14` and later have a `version.json` file in the
/// server jar file. This function will attempt to read the version string from
/// that file. If the file is not found, or the version string cannot be read,
/// `None` is returned.
///
/// This function uses the sync `zip` crate because the `async_zip` crate has
/// issues with reading some types of zip files.
pub async fn try_read_version_from_folder(server_folder: &Path) -> std::io::Result<Option<String>> {
    // get all jar files, ignoring errors (e.g., if a file is not a jar)
    let jar_files = {
        let mut dir_reader = tokio::fs::read_dir(server_folder).await?;
        let mut jar_files = Vec::new();
        while let Ok(Some(entry)) = dir_reader.next_entry().await {
            let path = entry.path();
            if let Some(ext) = path.extension() {
                if ext == "jar" {
                    jar_files.push(path);
                }
            }
        }
        jar_files
    };
    for jar in jar_files {
        let jar_handle = std::fs::File::open(&jar)?;
        let mut jar_reader = zip::ZipArchive::new(jar_handle)?;
        match jar_reader.by_name("version.json") {
            Ok(version_json) => {
                let parsed: serde_json::Value = serde_json::from_reader(version_json)?;
                return Ok(parsed["id"].as_str().map(String::from));
            }
            Err(e) => {
                warn!("failed to read version.json from jar file {jar:?}: {e:?}",);
                continue;
            }
        };
    }
    Ok(None)
}
