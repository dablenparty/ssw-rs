use std::{io, path::Path};

use chrono::{DateTime, Utc};
use log::warn;
use serde::Deserialize;
use serde_json::Value;

#[repr(u8)]
#[derive(Debug, Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum MCVersionType {
    Release = 0,
    Snapshot = 1,
    OldBeta = 2,
    OldAlpha = 3,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
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

impl PartialOrd for MinecraftVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MinecraftVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.release_time.cmp(&other.release_time)
    }
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

/// Tries to read the Minecraft version from every jar file found in a given directory.
///
/// Newer versions of Minecraft store the version in a `version.json` file packaged in with the server jar.
/// This function tries to read the version from that file.
///
/// The version ID string (e.g., `1.19`) will be returned if it is found, or `None` if it is not.
///
/// # Arguments
///
/// * `server_folder` - The directory containing the server jar.
///
/// # Errors
///
/// An error will be returned if the directory or a jar file cannot be read.
pub fn try_read_version_from_jar(server_folder: &Path) -> io::Result<Option<String>> {
    // get all jar files, ignoring errors (e.g., if a file is not a jar)
    let jar_files = server_folder
        .read_dir()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            if entry.path().extension()?.to_str()? == "jar" {
                Some(entry.path())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    for jar in jar_files {
        let jar_handle = std::fs::File::open(&jar)?;
        let mut jar_reader = zip::ZipArchive::new(jar_handle)?;
        match jar_reader.by_name("version.json") {
            Ok(version_json) => {
                let parsed: serde_json::Value = serde_json::from_reader(version_json)?;
                return Ok(parsed["id"].as_str().map(String::from));
            }
            Err(e) => {
                warn!(
                    "failed to read version.json from jar file {:?}: {:?}",
                    jar, e
                );
                continue;
            }
        };
    }
    Ok(None)
}
