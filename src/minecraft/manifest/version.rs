use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq, Eq, Clone, Copy)]
pub enum MinecraftVersionType {
    /// A release version
    Release,
    /// A snapshot version
    Snapshot,
    /// An old beta version
    OldBeta,
    /// An old alpha version
    OldAlpha,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MinecraftVersion {
    /// The ID of the version (e.g., `1.16.1`)
    id: String,
    /// The type of the version
    r#type: MinecraftVersionType,
    /// A URL to the version's JSON file
    url: String,
    /// A time field, not sure what this is for
    time: DateTime<Utc>,
    /// The time the version was released
    release_time: DateTime<Utc>,
    /// SHA1 hash of the version's JSON file
    sha1: String,
    /// The compliance level of the version. From what I can tell, this is used
    /// to determine if the version is compatible with player safety features
    compliance_level: u8,
}

// Ordering of versions is based on release time since the ID's can't
// be sorted if you include anything other than release versions

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
