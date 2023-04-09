use clap::{Parser, Subcommand};
use getset::CopyGetters;
use log::LevelFilter;

use crate::ssw_error;

use self::start::{start_main, StartArgs};

pub mod start;

#[repr(u8)]
#[derive(Debug, Subcommand)]
pub enum CommandLineCommands {
    /// Starts a Minecraft server.
    Start(StartArgs) = 0,
}

impl CommandLineCommands {
    pub async fn run(self) -> ssw_error::Result<()> {
        match self {
            CommandLineCommands::Start(args) => start_main(args).await?,
        }
        Ok(())
    }
}

/// Simple Server Wrapper (SSW) is a simple wrapper for Minecraft servers, allowing for easy
/// automation of some simple server management features.
#[derive(Parser, Debug, CopyGetters)]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
#[get_copy = "pub"]
pub struct CommandLineArgs {
    /// The SSW sub command to run.
    #[command(subcommand)]
    #[getset(skip)]
    subcommand: CommandLineCommands,

    /// The log level to use.
    #[arg(short, long, value_parser, default_value_t = LevelFilter::Info)]
    log_level: LevelFilter,

    /// Refreshes the Minecraft version manifest.
    #[arg(short, long)]
    refresh_manifest: bool,
}

impl CommandLineArgs {
    pub fn subcommand(self) -> CommandLineCommands {
        self.subcommand
    }
}
