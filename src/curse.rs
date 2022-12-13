use std::{io, path::Path};

use futures::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CurseFile {
    #[serde(rename = "fileID")]
    file_id: u32,
    #[serde(rename = "projectID")]
    project_id: u32,
    required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CurseManifest {
    files: Vec<CurseFile>,
    manifest_type: String,
    manifest_version: u8,
    name: String,
    overrides: String,
    version: String,
}

struct CurseModpack {
    archive: zip::ZipArchive<std::fs::File>,
    manifest: CurseManifest,
}

impl CurseModpack {
    fn new(path: &str) -> io::Result<Self> {
        let file = std::fs::File::open(path)?;
        let mut archive = zip::ZipArchive::new(file)?;
        let manifest = archive.by_name("manifest.json")?;
        let manifest: CurseManifest = serde_json::from_reader(manifest)?;
        Ok(Self { archive, manifest })
    }
}
