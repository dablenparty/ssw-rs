use std::{io, path::Path};

use chrono::{DateTime, Utc};
use log::warn;
use serde::Deserialize;

/// A Minecraft version type
#[repr(u8)]
#[derive(Debug, Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum MCVersionType {
    Release = 0,
    Snapshot = 1,
    OldBeta = 2,
    OldAlpha = 3,
}

/// A Minecraft version
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MinecraftVersion {
    /// The ID of the version (e.g., `1.16.1`)
    pub id: String,
    /// The type of the version
    pub r#type: MCVersionType,
    /// A URL to the version's JSON file
    pub url: String,
    /// A time field, not sure what this is for
    pub time: DateTime<Utc>,
    /// The time the version was released
    pub release_time: DateTime<Utc>,
    /// SHA1 hash of the version's JSON file
    pub sha1: String,
    /// The compliance level of the version. From what I can tell, this is used
    /// to determine if the version is compatible with player safety features
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

/// Tries to read the Minecraft version from every jar file found in a given directory.
///
/// Newer versions of Minecraft (`1.14` and later) store the version in a `version.json` file packaged in with the server jar.
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
