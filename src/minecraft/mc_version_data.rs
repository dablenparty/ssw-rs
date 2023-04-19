use getset::Getters;
use serde::Deserialize;

use crate::ssw_error;

use super::mc_version::MinecraftVersion;

/// The download info for a specific JAR related to a Minecraft version,
/// such as the server or client JAR.
#[derive(Debug, Deserialize, Getters)]
#[get = "pub"]
pub struct VersionDownloadInfo {
    /// The URL to download the server JAR from.
    url: String,
}

/// The download options for a Minecraft version.
#[derive(Debug, Deserialize, Getters)]
#[get = "pub"]
pub struct VersionDownloadOptions {
    /// The download info for the server JAR.
    server: VersionDownloadInfo,
}

/// Info about the Java version required to run a Minecraft version.
#[derive(Debug, Deserialize, Getters)]
#[serde(rename_all = "camelCase")]
#[get = "pub"]
pub struct VersionJavaInfo {
    /// The major version of Java required to run the server (e.g. 8, 17).
    major_version: u8,
}

/// The data for a Minecraft version.
#[derive(Debug, Deserialize, Getters)]
#[serde(rename_all = "camelCase")]
#[get = "pub"]
pub struct MinecraftVersionData {
    // My instinct is to uncomment this, but this isn't a library and I don't use this field anywhere.
    // id: String,
    /// The download options for this version.
    downloads: VersionDownloadOptions,
    /// The Java version required to run this version of Minecraft.
    java_version: VersionJavaInfo,
}

impl MinecraftVersionData {
    /// Tries to convert a `MinecraftVersion` into a `MinecraftVersionData` by making an API call.
    /// Normally, this would be implemented as a `TryFrom` trait, but traits can't be async.
    pub async fn async_try_from(mc_version: &MinecraftVersion) -> ssw_error::Result<Self> {
        let response: Self = reqwest::get(&mc_version.url).await?.json().await?;
        Ok(response)
    }
}
