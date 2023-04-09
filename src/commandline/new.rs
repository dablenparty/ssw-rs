use std::{io, path::PathBuf};

use clap::Args;

use crate::{ssw_error, util::async_create_dir_if_not_exists};

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
    todo!("Parse passed version and download server jar.");
}
