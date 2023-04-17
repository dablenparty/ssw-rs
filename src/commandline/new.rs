use std::{io, path::PathBuf};

use clap::Args;
use log::{debug, info};

use crate::{
    minecraft::{
        log4j::patch_log4j, manifest::VersionManifestV2, mc_version_data::MinecraftVersionData,
        MinecraftServer,
    },
    ssw_error,
    util::{async_create_dir_if_not_exists, download_file_with_progress},
};

#[derive(Debug, Args)]
pub struct NewArgs {
    /// The path to the Minecraft server directory.
    #[arg(required = true)]
    server_dir: PathBuf,

    /// The Minecraft version to use. If not specified, the latest release version is used.
    #[arg(short, long, value_parser)]
    mc_version: Option<String>,

    /// Whether to use the latest snapshot version instead of the latest release version.
    /// This option is ignored if `mc-version` is specified.
    #[arg(short, long)]
    snapshot: bool,
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
/// patching `Log4Shell`.
pub async fn new_main(args: NewArgs) -> ssw_error::Result<()> {
    let server_dir = args.server_dir;
    info!("Creating new server in {}", server_dir.display());
    if server_dir.exists() && server_dir.read_dir()?.next().is_some() {
        return Err(
            io::Error::new(io::ErrorKind::Other, "the specified directory is not empty").into(),
        );
    }
    async_create_dir_if_not_exists(&server_dir).await?;

    let server_dir = dunce::canonicalize(server_dir)?;
    let manifest = VersionManifestV2::load().await?;
    let use_snapshot = args.snapshot;
    let version_str = args.mc_version.unwrap_or_else(|| {
        let latest_data = manifest.latest();
        if use_snapshot {
            latest_data.snapshot().clone()
        } else {
            latest_data.release().clone()
        }
    });
    let version = manifest
        .find_version(&version_str)
        .ok_or_else(|| ssw_error::Error::BadMinecraftVersion(version_str))?;
    let download_message = format!("Downloading Minecraft server version {}", version.id);
    info!("{}", download_message);

    let version_data = MinecraftVersionData::async_try_from(version).await?;
    let server_info = version_data.downloads().server();
    let server_url = server_info.url();
    debug!("Server URL: {}", server_url);
    let server_jar = server_dir.join("server.jar");

    download_file_with_progress(server_url, &server_jar).await?;

    let mut mc_server = MinecraftServer::init(server_jar.clone()).await;
    mc_server.ssw_config.to_mut().mc_version = Some(version.id.clone());
    mc_server.save_config().await?;
    patch_log4j(&mut mc_server).await?;
    info!("Done! Server created at {}", server_jar.display());
    Ok(())
}
