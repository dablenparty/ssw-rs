use std::path::{Path, PathBuf};

use log::info;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::{ssw_error, util::async_create_dir_if_not_exists};

// TODO: auto-restart after crash
/// The SSW server configuration
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct SswConfig {
    /// How much memory to allocate to the server in gigabytes
    pub memory_in_gb: f64,
    /// How long to wait (in hours) before restarting the server
    pub restart_timeout: f64,
    /// How long to wait (in minutes) with no players before shutting
    /// down the server
    pub shutdown_timeout: f64,
    /// The port to use for the SSW proxy
    pub ssw_port: u16,
    /// The version string for the associated Minecraft server
    pub mc_version: Option<String>,
    /// The required Java version string for the associated Minecraft server
    pub required_java_version: String,
    /// Extra arguments to pass to the JVM when starting the server
    pub jvm_args: Vec<String>,
    /// Whether to automatically backup the server on startup
    pub auto_backup: bool,
    /// The maximum number of backups to keep
    pub max_backups: usize,
}

impl Default for SswConfig {
    fn default() -> Self {
        Self {
            memory_in_gb: 1.0,
            restart_timeout: 12.0,
            shutdown_timeout: 5.0,
            ssw_port: 25566,
            mc_version: None,
            required_java_version: "17.0".to_string(),
            jvm_args: Vec::new(),
            auto_backup: true,
            max_backups: 5,
        }
    }
}

impl SswConfig {
    /// Attempt to load a config from the given path
    ///
    /// # Arguments
    ///
    /// * `config_path` - The path to the config file. If it does not exist, it will be created with default values.
    ///
    /// # Errors
    ///
    /// An error may occur when reading or writing the config file, as well as in the serialization/deserialization process.
    ///
    /// returns: `serde_json::Result<SswConfig>`
    pub async fn from_path(config_path: &Path) -> ssw_error::Result<Self> {
        if config_path.exists() {
            info!("Found existing SSW config");
            let config_string = tokio::fs::read_to_string(config_path).await?;
            toml::from_str(&config_string).map_err(ssw_error::Error::from)
        } else {
            info!(
                "No SSW config found, creating default config at {}",
                config_path.display()
            );
            let config = Self::default();
            async_create_dir_if_not_exists(
                &config_path
                    .parent()
                    .map_or_else(|| PathBuf::from("."), Path::to_path_buf),
            )
            .await?;
            let config_string = toml::to_string_pretty(&config)?;
            tokio::fs::write(config_path, config_string).await?;
            Ok(config)
        }
    }

    /// Save the config to the given path
    ///
    /// # Arguments
    ///
    /// * `config_path` - The path to the config file. If it does not exist, it will be created with default values.
    ///
    /// # Errors
    ///
    /// An error may occur when writing the config file, as well as in the serialization process.
    pub async fn save(&self, config_path: &Path) -> ssw_error::Result<()> {
        let config_string = toml::to_string_pretty(&self)?;
        tokio::fs::write(config_path, config_string)
            .await
            .map_err(ssw_error::Error::from)
    }
}

/// Converts a JSON config file to a TOML config file, returning the deserialized config for
/// convenience. This is a temporary function to help with the transition from JSON to TOML.
///
/// ***IMPORTANT**: This function will delete the JSON file after conversion.*
///
/// # Arguments
///
/// * `json_path` - The path to the JSON config file
///
/// # Errors
///
/// An error may occur when reading or writing the config file, as well as in the serialization/deserialization process.
pub async fn convert_json_to_toml<T: Serialize + DeserializeOwned>(
    json_path: &Path,
) -> ssw_error::Result<T> {
    let json_string = tokio::fs::read_to_string(json_path).await?;
    let value: T = serde_json::from_str(&json_string)?;
    let toml_string = toml::to_string_pretty(&value)?;
    let toml_path = json_path.with_extension("toml");
    tokio::fs::write(toml_path.clone(), toml_string).await?;
    tokio::fs::remove_file(json_path).await?;
    Ok(value)
}
