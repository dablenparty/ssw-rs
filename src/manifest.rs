use std::{error, io, path::PathBuf};

use log::debug;
use serde::Deserialize;

use crate::mc_version::MinecraftVersion;

const MANIFEST_V2_LINK: &str = "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json";

/// The manifest of all Minecraft versions.
///
/// This struct is not a complete representation of the manifest, but only the parts that are needed.
#[derive(Deserialize)]
struct VersionManifestV2 {
    versions: Vec<MinecraftVersion>,
}

/// Gets the location to the launcher manifest.
///
/// - Windows: `%APPDATA%/.minecraft/versions/version_manifest_v2.json`
/// - Mac:  `~/Library/Application Support/minecraft/versions/version_manifest_v2.json`
/// - Linux:   `~/.minecraft/versions/version_manifest_v2.json`
fn get_manifest_location() -> PathBuf {
    const MANIFEST_NAME: &str = "version_manifest_v2.json";
    #[cfg(windows)]
    let manifest_parent = {
        let appdata = env!("APPDATA");
        let mut appdata_path = PathBuf::from(appdata);
        appdata_path.push(".minecraft");
        appdata_path.push("versions");
        appdata_path
    };

    #[cfg(target_os = "macos")]
    let manifest_parent = {
        let mut home_path = dirs::home_dir().expect("Could not find home directory");
        home_path.push("Library");
        home_path.push("Application Support");
        home_path.push("minecraft");
        home_path.push("versions");
        home_path
    };

    #[cfg(target_os = "linux")]
    let manifest_parent = {
        let mut home_path = dirs::home_dir().expect("Could not find home directory");
        home_path.push(".minecraft");
        home_path.push("versions");
        home_path
    };

    manifest_parent.join(MANIFEST_NAME)
}

/// Refreshes the launcher manifest by downloading the latest version of it from [Mojang](https://launchermeta.mojang.com/mc/game/version_manifest_v2.json).
pub async fn refresh_manifest() -> Result<(), Box<dyn error::Error>> {
    debug!(
        "Downloading Minecraft version manifest from {}",
        MANIFEST_V2_LINK
    );
    let manifest = reqwest::get(MANIFEST_V2_LINK).await?.text().await?;
    let manifest_location = get_manifest_location();
    debug!("Saving new manifest to {}", manifest_location.display());
    tokio::fs::write(manifest_location, manifest).await?;
    Ok(())
}

/// Loads the launcher manifest from the local file system.
pub async fn load_versions() -> io::Result<Vec<MinecraftVersion>> {
    let manifest_location = get_manifest_location();
    debug!(
        "Loading version manifest from {}",
        manifest_location.display()
    );
    let manifest = tokio::fs::read_to_string(manifest_location).await?;
    let manifest: VersionManifestV2 = serde_json::from_str(&manifest)?;
    Ok(manifest.versions)
}
