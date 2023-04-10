use std::{io, path::PathBuf};

use clap::Args;

use crate::{
    minecraft::manifest::VersionManifestV2, ssw_error, util::async_create_dir_if_not_exists,
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

pub async fn new_main(args: NewArgs) -> ssw_error::Result<()> {
    let server_dir = args.server_dir;
    if server_dir.exists() {
        if server_dir.read_dir()?.next().is_some() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "the specified directory is not empty",
            )
            .into());
        }
    } else {
        async_create_dir_if_not_exists(&server_dir).await?;
    }
    let manifest = VersionManifestV2::load().await?;
    let version_str = args
        .mc_version
        .unwrap_or_else(|| manifest.latest().release().clone());
    let version = manifest
        .find_version(&version_str)
        .ok_or_else(|| ssw_error::Error::BadMinecraftVersion(version_str))?;

    todo!("download server jar.");
    todo!("patch log4j (this will require larger refactorings)");
}
