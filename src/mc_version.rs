use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

#[repr(u8)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MCVersionType {
    Release = 0,
    Snapshot = 1,
    OldBeta = 2,
    OldAlpha = 3,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MinecraftVersion {
    pub id: String,
    pub r#type: MCVersionType,
    pub url: String,
    pub time: DateTime<Utc>,
    pub release_time: DateTime<Utc>,
    pub sha1: String,
    pub compliance_level: u8,
}

/// Gets the required version of Java for the given Minecraft version.
///
/// `MinecraftVersion` structs have a `url` field that points to an API with more information about the version.
///
/// # Arguments
///
/// * `mc_version`: the Minecraft version to get the required Java version forƒ
pub async fn get_required_java_version(mc_version: &MinecraftVersion) -> reqwest::Result<String> {
    let response: Value = reqwest::get(&mc_version.url).await?.json().await?;
    let major_version = response["javaVersion"]["majorVersion"]
        .as_u64()
        .unwrap_or(17);
    if major_version <= 8 {
        Ok(format!("1.{}", major_version))
    } else {
        Ok(format!("{}.0", major_version))
    }
}
