use std::{io, path::PathBuf};

use clap::Args;
use log::{debug, info};

use crate::{
    minecraft::{
        log4j::patch_log4j, manifest::VersionManifestV2, mc_version_data::MinecraftVersionData,
        MinecraftServer,
    },
    ssw_error,
    util::async_create_dir_if_not_exists,
};

#[derive(Debug, Args)]
pub struct NewArgs {
    /// The path to the Minecraft server directory.
    #[arg(required = true)]
    server_dir: PathBuf,

    /// The Minecraft version to use.
    #[arg(short, long, value_parser)]
    mc_version: Option<String>,
}

/// Main function for the `new` subcommand. Makes a new Minecraft server.
///
/// # Arguments
///
/// * `args` - The command-line arguments.
///
/// # Errors
///
/// An error occurs if the specified directory is not empty, or if the server could not be created.
/// This includes errors that occur while creating the server dir, downloading the server JAR, or
/// patching Log4Shell.
pub async fn new_main(args: NewArgs) -> ssw_error::Result<()> {
    let server_dir = args.server_dir;
    info!("Creating new server in {}", server_dir.display());
    if server_dir.exists() && server_dir.read_dir()?.next().is_some() {
        return Err(
            io::Error::new(io::ErrorKind::Other, "the specified directory is not empty").into(),
        );
    } else {
        async_create_dir_if_not_exists(&server_dir).await?;
    }
    let server_dir = dunce::canonicalize(server_dir)?;
    let manifest = VersionManifestV2::load().await?;
    let version_str = args
        .mc_version
        .unwrap_or_else(|| manifest.latest().release().clone());
    let version = manifest
        .find_version(&version_str)
        .ok_or_else(|| ssw_error::Error::BadMinecraftVersion(version_str))?;

    info!("Downloading Minecraft server version {}", version.id);

    let version_data = MinecraftVersionData::async_try_from(version).await?;
    let server_info = version_data.downloads().server();
    let server_url = server_info.url();
    let server_size = server_info.size();
    debug!("Server URL: {}", server_url);
    debug!("Server size: {} bytes", server_size);

    let server_jar = server_dir.join("server.jar");
    let response_bytes = reqwest::get(server_url)
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    tokio::fs::write(&server_jar, response_bytes).await?;
    let mut mc_server = MinecraftServer::init(server_jar.clone()).await;
    mc_server.ssw_config.mc_version = Some(version.id.clone());
    patch_log4j(&mut mc_server).await?;
    info!("Done! Server created at {}", server_jar.display());
    Ok(())
}
