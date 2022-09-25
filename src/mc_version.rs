use chrono::{DateTime, Utc};
use serde::Deserialize;

#[repr(u8)]
#[derive(Debug, Deserialize)]
pub enum MCVersionType {
    Release = 0,
    Snapshot = 1,
    OldBeta = 2,
    OldAlpha = 3,
}

#[derive(Debug, Deserialize)]
pub struct MinecraftVersion {
    pub id: String,
    pub r#type: MCVersionType,
    pub url: String,
    pub time: DateTime<Utc>,
    pub release_time: DateTime<Utc>,
    pub sha1: String,
    pub compliance_level: u8,
}
