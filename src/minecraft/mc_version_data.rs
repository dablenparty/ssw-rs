use getset::Getters;
use serde::Deserialize;

use crate::ssw_error;

use super::mc_version::MinecraftVersion;

#[derive(Debug, Deserialize, Getters)]
#[get = "pub"]
pub struct VersionDownloadInfo {
    url: String,
    size: u64,
}

#[derive(Debug, Deserialize, Getters)]
#[get = "pub"]
pub struct VersionDownloadOptions {
    server: VersionDownloadInfo,
}

#[derive(Debug, Deserialize, Getters)]
#[serde(rename_all = "camelCase")]
#[get = "pub"]
pub struct VersionJavaInfo {
    major_version: u8,
}

#[derive(Debug, Deserialize, Getters)]
#[serde(rename_all = "camelCase")]
#[get = "pub"]
pub struct MinecraftVersionData {
    // My instinct is to uncomment this, but this isn't a library and I don't use this field anywhere.
    // id: String,
    downloads: VersionDownloadOptions,
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
