use log::{debug, info};

use super::{
    manifest::{VersionManifestError, VersionManifestV2},
    MinecraftServer,
};

#[derive(Debug, thiserror::Error)]
#[allow(clippy::enum_variant_names)]
pub enum Log4ShellPatchError {
    #[error("Version manifest error: {0}")]
    VersionManifestError(#[from] VersionManifestError),
    #[error("SSW config error: {0}")]
    SswConfigError(#[from] crate::config::SswConfigError),
    #[error("Failed to download Log4j config: {0}")]
    DownloadError(#[from] reqwest::Error),
    #[error("Failed to write Log4j config: {0}")]
    IoError(#[from] std::io::Error),
}

impl MinecraftServer<'_> {
    /// Attempts to patch [`Log4Shell`](https://www.dynatrace.com/news/blog/what-is-log4shell/#:~:text=Log4Shell%20is%20a%20software%20vulnerability,certain%20versions%20of%20Log4j%202.),
    /// if needed.
    /// This function follows the instructions from
    /// [this page](https://help.minecraft.net/hc/en-us/articles/4416199399693#:~:text=We%20have%20identified%20a%20vulnerability,%E2%80%93%20including%20Minecraft%3A%20Java%20Edition.).
    pub async fn patch_log4shell(&mut self) -> Result<(), Log4ShellPatchError> {
        let manifest = VersionManifestV2::load().await?;
        let server_version_id = self
            .config
            .mc_version()
            .as_ref()
            .ok_or(crate::config::SswConfigError::MissingMinecraftVersion)?;
        let server_version = manifest.find_by_id(server_version_id)?;
        // there might be a better way to do this than this ugly if/else chain, but I don't know it
        // since finding the version in the manifest is O(n), I don't want to do it more than
        // necessary, hence them all being searched in-line
        let (arg, url) = if server_version >= manifest.find_by_id("1.18.1").unwrap() {
            // no patch needed (too new)
            (None, None)
        } else if server_version >= manifest.find_by_id("1.17").unwrap() {
            (Some("-Dlog4j2.formatMsgNoLookups=true"), None)
        } else if manifest.find_by_id("1.12").unwrap() <= server_version
            && server_version <= manifest.find_by_id("1.16.5").unwrap()
        {
            (Some("-Dlog4j.configurationFile=log4j2_112-116.xml"), Some("https://launcher.mojang.com/v1/objects/02937d122c86ce73319ef9975b58896fc1b491d1/log4j2_112-116.xml"))
        } else if manifest.find_by_id("1.7").unwrap() <= server_version
            && server_version <= manifest.find_by_id("1.11.2").unwrap()
        {
            (Some("-Dlog4j.configurationFile=log4j2_17-111.xml"), Some("https://launcher.mojang.com/v1/objects/4bb89a97a66f350bc9f73b3ca8509632682aea2e/log4j2_17-111.xml"))
        } else {
            // no patch needed (too old)
            (None, None)
        };

        if let Some(url) = url {
            let file_name = url.split('/').last().unwrap();
            let log4j_config_path = self.launcher.path.with_file_name(file_name);
            if !log4j_config_path.exists() {
                info!("Downloading Log4Shell patch from {url}");
                let log4j_config_bytes = reqwest::get(url).await?.bytes().await?;
                tokio::fs::write(log4j_config_path, log4j_config_bytes).await?;
            }
        }

        let config = &mut self.config;

        if let Some(arg) = arg {
            let arg = arg.to_string();
            if !config.extra_jvm_args().contains(&arg) {
                debug!("Added Log4Shell patch to config");
                config.extra_jvm_args_mut().to_mut().push(arg);
                let config_path = self.launcher.path.with_file_name("ssw-config.toml");
                config.save(&config_path).await?;
            }
        } else {
            // the arg is always Some if a patch is needed, otherwise this branch executes
            info!("No Log4Shell patch needed for this version of Minecraft");
        }

        Ok(())
    }
}
