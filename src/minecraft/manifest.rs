use std::path::PathBuf;

use getset::Getters;
use log::debug;
use serde::Deserialize;

pub mod version;

const MANIFEST_V2_URL: &str = "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json";

#[derive(Debug, thiserror::Error)]
pub enum VersionManifestError {
    #[error("Failed to download version manifest: {0}")]
    DownloadError(#[from] reqwest::Error),
    #[error("Failed to parse version manifest: {0}")]
    ParseError(#[from] serde_json::Error),
    #[error("Failed to read or write version manifest: {0}")]
    ReadWriteError(#[from] std::io::Error),
}

/// The version manifest for the Minecraft launcher.
///
/// This is not a full representation of the version manifest, only the parts
/// that are relevant to this crate.
#[derive(Debug, Deserialize, Getters)]
#[get = "pub"]
pub struct VersionManifestV2 {
    versions: Vec<version::MinecraftVersion>,
}

impl VersionManifestV2 {
    /// Loads the version manifest from the default location specified by
    /// `get_manifest_location` and parses it into a `VersionManifestV2`.
    /// This will not download the version manifest if it does not exist,
    /// use `refresh_launcher_manifest` to do that.
    pub async fn load() -> Result<VersionManifestV2, VersionManifestError> {
        let manifest_location = get_manifest_location();
        let manifest_string = tokio::fs::read_to_string(manifest_location).await?;
        VersionManifestV2::try_from(manifest_string)
    }

    /// Downloads the version manifest from Mojang and saves it to the default
    /// location specified by `get_manifest_location`. This will overwrite any
    /// existing version manifest, hence the name "refresh".
    ///
    /// # Errors
    ///
    /// This function will return an error if the version manifest could not be
    /// downloaded, parsed, or written to disk.
    pub async fn refresh_launcher_manifest() -> Result<(), VersionManifestError> {
        let manifest_location = get_manifest_location();
        debug!("Downloading version manifest from {MANIFEST_V2_URL}");
        let manifest_bytes = reqwest::get(MANIFEST_V2_URL)
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        debug!(
            "Writing version manifest to {}",
            manifest_location.display()
        );
        tokio::fs::write(manifest_location, manifest_bytes).await?;
        Ok(())
    }
}

impl TryFrom<&str> for VersionManifestV2 {
    type Error = VersionManifestError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let manifest = serde_json::from_str::<VersionManifestV2>(value)?;
        Ok(manifest)
    }
}

impl TryFrom<String> for VersionManifestV2 {
    type Error = VersionManifestError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

/// Gets the location to the launcher manifest.
///
/// - Windows: `%APPDATA%/.minecraft/versions/version_manifest_v2.json`
/// - Mac:     `~/Library/Application Support/minecraft/versions/version_manifest_v2.json`
/// - Linux:   `~/.minecraft/versions/version_manifest_v2.json`
///
/// returns: `PathBuf`
fn get_manifest_location() -> PathBuf {
    const MANIFEST_NAME: &str = "version_manifest_v2.json";
    #[cfg(windows)]
    let manifest_parent = {
        // if APPDATA isn't set, there's a bigger problem
        let appdata = std::env::var("APPDATA").expect("APPDATA environment variable not set");
        let mut appdata_path = PathBuf::from(appdata);
        appdata_path.push(".minecraft");
        appdata_path
    };

    #[cfg(target_os = "macos")]
    let manifest_parent = {
        let mut home_path = dirs::home_dir().expect("Could not find home directory");
        home_path.push("Library");
        home_path.push("Application Support");
        home_path.push("minecraft");
        home_path
    };

    #[cfg(target_os = "linux")]
    let manifest_parent = {
        let mut home_path = dirs::home_dir().expect("Could not find home directory");
        home_path.push(".minecraft");
        home_path
    };

    manifest_parent.join("versions").join(MANIFEST_NAME)
}
