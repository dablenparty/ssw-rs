use log::{debug, info};

use crate::{minecraft::MinecraftServer, ssw_error, util::download_file_with_progress};

use super::manifest::VersionManifestV2;

/// Patch the Log4j vulnerability in a Minecraft server.
/// More information can be found on the [Minecraft website](https://help.minecraft.net/hc/en-us/articles/4416199399693-Security-Vulnerability-in-Minecraft-Java-Edition).
///
/// # Arguments
///
/// * `mc_server` - The Minecraft server to patch.
///
/// # Errors
///
/// An error will be returned if one of the following happens:
/// - The version manifest cannot be loaded
/// - The server version is not specified
/// - The patch file fails to download or write
/// - The server config fails to save
pub async fn patch_log4j(mc_server: &mut MinecraftServer<'_>) -> ssw_error::Result<()> {
    let manifest = VersionManifestV2::load().await?;
    let server_version_id = mc_server
        .ssw_config
        .mc_version
        .clone()
        .ok_or(ssw_error::Error::MissingMinecraftVersion)?;
    // unwrap is safe because the version id is validated when it is assigned
    let server_version = manifest.find_version(&server_version_id).unwrap();
    let (arg, url) = if server_version >= manifest.find_version("1.18.1").unwrap() {
        // no patch needed (too new)
        (None, None)
    } else if server_version >= manifest.find_version("1.17").unwrap() {
        (Some("-Dlog4j2.formatMsgNoLookups=true"), None)
    } else if manifest.find_version("1.12").unwrap() <= server_version
        && server_version <= manifest.find_version("1.16.5").unwrap()
    {
        (Some("-Dlog4j.configurationFile=log4j2_112-116.xml"), Some("https://launcher.mojang.com/v1/objects/02937d122c86ce73319ef9975b58896fc1b491d1/log4j2_112-116.xml"))
    } else if manifest.find_version("1.7").unwrap() <= server_version
        && server_version <= manifest.find_version("1.11.2").unwrap()
    {
        (Some("-Dlog4j.configurationFile=log4j2_17-111.xml"), Some("https://launcher.mojang.com/v1/objects/4bb89a97a66f350bc9f73b3ca8509632682aea2e/log4j2_17-111.xml"))
    } else {
        // no patch needed (too old)
        (None, None)
    };

    let should_save = arg.is_some() && url.is_some();

    if let Some(url) = url {
        let file_name = url.split('/').last().unwrap();
        let log4j_config_path = mc_server.jar_path().with_file_name(file_name);
        if !log4j_config_path.exists() {
            download_file_with_progress(url, &log4j_config_path).await?;
        }
    }

    let config = &mut mc_server.ssw_config;

    if let Some(arg) = arg {
        let arg = arg.to_string();
        if !config.jvm_args.contains(&arg) {
            config.to_mut().jvm_args.to_mut().push(arg);
        }
    }

    if should_save {
        debug!("Saving server config");
        mc_server.save_config().await?;
        info!("Patched log4j for {}", server_version_id);
    } else {
        info!("No Log4J patch needed for this version of Minecraft");
    }

    Ok(())
}
