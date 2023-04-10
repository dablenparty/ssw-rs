use getset::Getters;
use serde::Deserialize;

#[derive(Debug, Deserialize, Getters)]
#[get = "pub"]
pub struct VersionDownloadInfo {
    url: String,
    size: u64,
}

#[derive(Debug, Deserialize, Getters)]
#[get = "pub"]
pub struct VersionDownloadOptions {
    client: VersionDownloadInfo,
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
    id: String,
    downloads: VersionDownloadOptions,
    java_version: VersionJavaInfo,
}
