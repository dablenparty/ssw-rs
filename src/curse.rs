use std::{io, path::Path};

use futures::{stream, StreamExt};
use log::{debug, info};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::ssw_error;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CurseFile {
    #[serde(rename = "fileID")]
    file_id: u32,
    #[serde(rename = "projectID")]
    project_id: u32,
    required: bool,
}

impl CurseFile {
    async fn get_info(
        &self,
        client: &Client,
        api_key: &str,
    ) -> ssw_error::Result<serde_json::Value> {
        const BASE_CURSE_URL: &str = "https://api.curseforge.com";
        let endpoint = format!("/v1/mods/{}/files/{}", self.project_id, self.file_id);
        let url = format!("{}{}", BASE_CURSE_URL, endpoint);
        debug!("Requesting {}", url);
        let response = client
            .get(&url)
            .header("X-API-Key", api_key)
            .header("Accept", "application/json")
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?;
        Ok(response)
    }
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

pub struct CurseModpack {
    archive: zip::ZipArchive<std::fs::File>,
    manifest: CurseManifest,
}

impl CurseModpack {
    pub fn load(path: &str) -> io::Result<Self> {
        let file = std::fs::File::open(path)?;
        let mut archive = zip::ZipArchive::new(file)?;
        let manifest = archive.by_name("manifest.json")?;
        let manifest: CurseManifest = serde_json::from_reader(manifest)?;
        Ok(Self { archive, manifest })
    }

    pub async fn install_to(&mut self, server_jar: &Path) -> io::Result<()> {
        lazy_static::lazy_static! {
            static ref ILLEGAL_CHARS: regex::Regex = regex::Regex::new(r#"[\\/:*?"<>|]"#).expect("Failed to compile ILLEGAL_CHARS regex");
        }
        let target_dir = if server_jar.is_dir() {
            server_jar.to_path_buf()
        } else {
            server_jar
                .parent()
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Server jar path is not in a folder",
                    )
                })?
                .to_path_buf()
        };
        debug!("modpack target directory: {}", target_dir.display());
        let num_cpus = num_cpus::get();
        let num_physical_cpus = num_cpus::get_physical();
        let client = reqwest::Client::new();
        // this variable should be validated when the program starts
        let api_key = std::env::var("CURSE_API_KEY").unwrap();
        stream::iter(&self.manifest.files)
            .map(|file| {
                let client = &client;
                let api_key = &api_key;
                async move {
                    let info = file.get_info(client, api_key).await?;
                    Ok::<_, crate::ssw_error::Error>(info)
                }
            })
            .buffer_unordered(num_cpus * 2)
            //? TODO: do this without cloning
            .filter_map(|parsed| async move { parsed.ok().map(|p| p["data"].clone()) })
            .map(|data| {
                let download_url = data["downloadUrl"].to_string().replace('"', "");
                let file_name = data["fileName"].to_string();
                let file_name = ILLEGAL_CHARS.replace_all(&file_name, "").to_string();
                let parent_folder = if file_name.ends_with("zip") {
                    "resourcepacks"
                } else {
                    "mods"
                };
                let target_parent = target_dir.join(parent_folder);
                if !target_parent.exists() {
                    std::fs::create_dir_all(&target_parent).ok();
                }
                let target_path = target_parent.join(file_name);
                let client = &client;
                info!(
                    "downloading {} to {}",
                    data["displayName"],
                    target_path.display()
                );
                async move {
                    if target_path.exists() {
                        info!("{} already exists, skipping", target_path.display());
                        return Ok(());
                    }
                    let mut file_handle = tokio::fs::File::create(&target_path).await?;
                    let dl_response = client.get(&download_url).send().await?.error_for_status()?;
                    let content = dl_response.text().await?;
                    tokio::io::copy(&mut content.as_bytes(), &mut file_handle).await?;
                    Ok::<_, crate::ssw_error::Error>(())
                }
            })
            .buffer_unordered(num_physical_cpus * 2)
            .for_each(|_| async {})
            .await;
        for i in 0..self.archive.len() {
            let mut file = self.archive.by_index(i)?;
            let enclosed_name = file.enclosed_name().unwrap();
            if file.is_dir() || !enclosed_name.starts_with(&self.manifest.overrides) {
                continue;
            }
            let stripped_name = enclosed_name
                .strip_prefix(&self.manifest.overrides)
                .unwrap();
            let target_path = target_dir.join(stripped_name);
            if target_path.exists() {
                info!("{} already exists, skipping", target_path.display());
                continue;
            }
            info!("extracting override to: {}", target_path.display());
            let target_dir = target_path.parent().unwrap();
            if !target_dir.exists() {
                std::fs::create_dir_all(target_dir)?;
            }
            let mut target_file = std::fs::File::create(target_path)?;
            io::copy(&mut file, &mut target_file)?;
        }
        Ok(())
    }
}
