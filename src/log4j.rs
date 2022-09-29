use std::io;

use log::{debug, info};

use crate::{manifest::load_versions, mc_version::MinecraftVersion, minecraft::MinecraftServer};

/// Helper function to get a Minecraft version from a version string.
/// It is very important to note that this function assumes the version string is valid.
/// This decision was made as this function is always used in a context where the version string
/// is known to be valid.
///
/// # Arguments
///
/// * `versions` - The list of versions to search through.
/// * `id` - The version string to search for.
fn get_version_by_id<'a>(versions: &'a [MinecraftVersion], id: &str) -> &'a MinecraftVersion {
    versions.iter().find(|v| v.id == id).unwrap()
}

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
pub async fn patch_log4j(mc_server: &mut MinecraftServer) -> io::Result<()> {
    let versions = load_versions().await?;
    let server_version_id = mc_server.ssw_config.mc_version.as_ref().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Minecraft version not specified in server config",
        )
    })?;
    // unwrap is safe because the version id is validated when it is assigned
    let server_version = versions
        .iter()
        .find(|version| version.id == *server_version_id)
        .unwrap();
    let (arg, url) = if server_version >= get_version_by_id(&versions, "1.18.1") {
        // no patch needed
        (None, None)
    } else if server_version >= get_version_by_id(&versions, "1.17") {
        (Some("-Dlog4j2.formatMsgNoLookups=true"), None)
    } else if get_version_by_id(&versions, "1.12") <= server_version
        && server_version <= get_version_by_id(&versions, "1.16.5")
    {
        (Some("-Dlog4j.configurationFile=log4j2_112-116.xml"), Some("https://launcher.mojang.com/v1/objects/02937d122c86ce73319ef9975b58896fc1b491d1/log4j2_112-116.xml"))
    } else if get_version_by_id(&versions, "1.7") <= server_version
        && server_version <= get_version_by_id(&versions, "1.11.2")
    {
        (Some("-Dlog4j.configurationFile=log4j2_17-111.xml"), Some("https://launcher.mojang.com/v1/objects/4bb89a97a66f350bc9f73b3ca8509632682aea2e/log4j2_17-111.xml"))
    } else {
        (None, None)
    };

    let should_save = arg.is_some() && url.is_some();

    if let Some(url) = url {
        let file_name = url.split('/').last().unwrap();
        let log4j_config_path = mc_server.jar_path().with_file_name(file_name);
        // TODO: SSW Error Type
        if !log4j_config_path.exists() {
            // DO NOT UNWRAP THIS
            let text = reqwest::get(url).await.unwrap().text().await.unwrap();
            tokio::fs::write(&log4j_config_path, text).await?;
        }
    }

    if let Some(arg) = arg {
        let arg = arg.to_string();
        if !mc_server.ssw_config.extra_args.contains(&arg) {
            mc_server.ssw_config.extra_args.push(arg);
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
