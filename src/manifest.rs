use std::{error, io, path::PathBuf};

use log::debug;
use serde::Deserialize;

use crate::mc_version::MinecraftVersion;

const MANIFEST_V2_LINK: &str = "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json";

#[derive(Debug, Deserialize)]
struct Latest {
    release: String,
    snapshot: String,
}

#[derive(Deserialize)]
struct VersionManifestV2 {
    latest: Latest,
    versions: Vec<MinecraftVersion>,
}

/// Gets the location to the launcher manifest.
///
/// - Windows: `%APPDATA%/.minecraft/versions/version_manifest_v2.json`
/// - macOSX:  `~/Library/Application Support/minecraft/versions/version_manifest_v2.json`
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

    // TODO: mac and linux

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
