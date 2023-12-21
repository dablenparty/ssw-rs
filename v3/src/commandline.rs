use std::path::PathBuf;

use clap::{Parser, ValueHint};

#[derive(Debug, Parser)]
pub struct CommandLineArgs {
    /// The server launch file. This is most often a .jar file, but can also be a script
    /// (e.g. .bat, .sh, ps1, etc.)
    #[arg(required = true, value_hint = ValueHint::FilePath, value_parser = parse_launcher_path)]
    server_launcher: PathBuf,
}

fn parse_launcher_path(s: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(s);
    match p.try_exists() {
        Ok(true) if p.is_file() => Ok(p),
        Ok(false) => Err(format!("Path does not exist: {s}")),
        Err(e) => Err(format!("Error verifying path: {e}")),
        _ => Err(format!("Path is not a file: {s}")),
    }
}
