use std::{path::PathBuf, str::FromStr};

use clap::{Parser, ValueHint};
use log::LevelFilter;

fn parse_path(s: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(s);
    match p.try_exists() {
        Ok(true) if p.is_file() => Ok(p),
        Ok(false) => Err(format!("Path does not exist: {s}")),
        Err(e) => Err(format!("Error checking path: {e}")),
        _ => Err(format!("Path is not a file: {s}")),
    }
}

fn parse_log_level(s: &str) -> Result<LevelFilter, String> {
    if let Ok(l) = LevelFilter::from_str(s) {
        Ok(l)
    } else {
        let mut options: Vec<_> = LevelFilter::iter().collect();
        options.sort();
        Err(format!(
            "Invalid log level: {s}. Valid options are: {options:?}",
        ))
    }
}

/// Simple Server Wrapper (SSW) is a server wrapper for Minecraft servers, allowing for easy
/// automation of some simple server management features.
#[derive(Debug, Parser)]
pub struct CommandLineArgs {
    /// The path to the server jar
    #[arg(required = true, value_parser = parse_path, value_hint = ValueHint::FilePath)]
    pub server_jar_path: PathBuf,

    /// The log level to use for the terminal (file will always be debug or trace)
    #[arg(long, short, default_value = "info", value_parser = parse_log_level)]
    pub log_level: LevelFilter,

    /// Refresh the version manifest when SSW starts
    #[arg(long, short)]
    pub refresh_manifest: bool,
}
