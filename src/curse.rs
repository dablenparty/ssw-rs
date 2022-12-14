use std::{io, path::Path};

use futures::{stream, StreamExt};
use log::{debug, info, warn};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::{ssw_error, util::async_create_dir_if_not_exists};

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
            .header("Accept", "application/json")
            .header("x-api-key", api_key)
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

    pub async fn install_to(&mut self, server_jar: &Path) -> ssw_error::Result<()> {
        lazy_static::lazy_static! {
            static ref ILLEGAL_CHARS: regex::Regex = regex::Regex::new(r#"[\\/:*?"<>|]"#).expect("Failed to compile ILLEGAL_CHARS regex");
        }
        let target_dir = if server_jar.is_dir() {
            server_jar.to_path_buf()
        } else {
            // unwrapping should be ok. if the server jar is a file, it should have a parent
            // otherwise, the user is doing something weird and deserves to have their program panic
            server_jar.parent().unwrap().to_path_buf()
        };
        debug!("modpack target directory: {}", target_dir.display());
        let num_cpus = num_cpus::get();
        let client = reqwest::Client::new();
        let api_key = std::env::var("CURSE_API_KEY")?;
        info!("Downloading {} mod files", self.manifest.files.len());
        stream::iter(&self.manifest.files)
            .map(|file| {
                let client = &client;
                let api_key = &api_key;
                async move {
                    let info = file.get_info(client, api_key).await?;
                    Ok::<_, ssw_error::Error>(info)
                }
            })
            .buffer_unordered(num_cpus * 2)
            //? TODO: do this without cloning
            .filter_map(|parsed| async move {
                parsed.map_or_else(
                    |e| {
                        warn!("Failed to get file info: {}", e);
                        None
                    },
                    |p| Some(p["data"].clone()),
                )
            })
            .map(|data| {
                let download_url = data["downloadUrl"].to_string().replace('"', "");
                let file_name = data["fileName"].to_string();
                let file_name = ILLEGAL_CHARS.replace_all(&file_name, "").to_string();
                let parent_folder = if file_name.ends_with("zip") {
                    "resourcepacks"
                } else {
                    "mods"
                };
                let client = &client;
                let target_parent = target_dir.join(parent_folder);
                async move {
                    async_create_dir_if_not_exists(&target_parent).await?;
                    let target_path = target_parent.join(file_name);
                    debug!(
                        "downloading {} to {}",
                        data["displayName"],
                        target_path.display()
                    );
                    if download_url == "null" {
                        return Err(crate::ssw_error::Error::IoError(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("downloadUrl for {} is null", data["displayName"]),
                        )));
                    }
                    if target_path.exists() {
                        debug!("{} already exists, skipping", target_path.display());
                        return Ok(());
                    }
                    let mut file_handle = tokio::fs::File::create(&target_path).await?;
                    let dl_response = client.get(&download_url).send().await?.error_for_status()?;
                    let content = dl_response.bytes().await?;
                    tokio::io::copy(&mut content.to_vec().as_slice(), &mut file_handle).await?;
                    Ok::<_, ssw_error::Error>(())
                }
            })
            .buffer_unordered(num_cpus)
            .for_each(|r| async {
                if let Err(e) = r {
                    warn!("{}", e);
                }
            })
            .await;
        info!("extracting overrides");
        for i in 0..self.archive.len() {
            let mut file = self
                .archive
                .by_index(i)
                .map_err(|e| ssw_error::Error::IoError(e.into()))?;
            let enclosed_name = file.enclosed_name().unwrap();
            let stripped_name = enclosed_name.strip_prefix(&self.manifest.overrides);
            if stripped_name.is_err() || file.is_dir() {
                continue;
            }
            let stripped_name = stripped_name.unwrap();
            let target_path = target_dir.join(stripped_name);
            if target_path.exists() {
                debug!("{} already exists, skipping", target_path.display());
                continue;
            }
            debug!("extracting override to: {}", target_path.display());
            async_create_dir_if_not_exists(target_path.parent().unwrap()).await?;
            let mut target_file = std::fs::File::create(target_path)?;
            io::copy(&mut file, &mut target_file)?;
        }
        Ok(())
    }
}
