use std::path::PathBuf;

use clap::Args;

use crate::ssw_error;

#[derive(Debug, Args)]
pub struct NewArgs {
    /// The path to the Minecraft server directory.
    #[arg(required = true)]
    server_dir: PathBuf,

    /// The Minecraft version to use.
    #[arg(short, long, value_parser)]
    mc_version: Option<String>,
}

pub async fn new_main(_args: NewArgs) -> ssw_error::Result<()> {
    unimplemented!("`ssw new` is not yet implemented.");
}
