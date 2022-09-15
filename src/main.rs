mod minecraft;
mod util;

use std::{
    io::{self, Write},
    path::PathBuf,
};

use chrono::{DateTime, Local};
use flate2::{Compression, GzBuilder};
use log::{error, LevelFilter};
use simplelog::{
    format_description, ColorChoice, CombinedLogger, TermLogger, TerminalMode, ThreadLogMode,
    WriteLogger,
};
use tokio::io::AsyncBufReadExt;
use util::{create_dir_if_not_exists, get_exe_parent_dir};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    if let Err(e) = init_logger() {
        eprintln!("failed to initialize logger: {:?}", e);
        std::process::exit(1);
    }
    let path = std::env::args().nth(1).expect("Missing path to server jar");
    let mut mc_server = minecraft::MinecraftServer::new(dunce::canonicalize(PathBuf::from(path))?);
    let mut stdin_reader = tokio::io::BufReader::new(tokio::io::stdin());
    let cargo_version = env!("CARGO_PKG_VERSION");
    println!("SSW Console v{}", cargo_version);
    // TODO: handle commands & errors properly without propagating them
    loop {
        let mut buf = Vec::new();
        stdin_reader.read_until(b'\n', &mut buf).await?;
        let msg = String::from_utf8_lossy(&buf).into_owned();
        let status = *mc_server.status().lock().unwrap();
        if status == minecraft::MCServerState::Running {
            if let Some(sender) = mc_server.get_server_sender() {
                if let Err(err) = sender.send(msg).await {
                    eprintln!("Error sending message to server: {}", err);
                }
            } else {
                eprintln!("Server is running but no sender is available");
            }
        } else {
            let command = msg.trim();
            match command {
                "start" => {
                    if status == minecraft::MCServerState::Stopped {
                        mc_server.run()?;
                    } else {
                        eprintln!("Server is already running");
                    }
                }
                "exit" => {
                    if status == minecraft::MCServerState::Running {
                        if let Some(sender) = mc_server.get_server_sender() {
                            if let Err(err) = sender.send("stop\n".to_string()).await {
                                eprintln!("Error sending message to server: {}", err);
                            }
                        }
                        mc_server.wait_for_exit().await?;
                    }
                    break;
                }
                "help" => {
                    println!("Available commands:");
                    println!("    start - start the server");
                    println!("    exit - exit ssw");
                    println!("    help - show this help message");
                }
                _ => {
                    eprintln!("Unknown command: {}", command);
                }
            }
        }
    }
    Ok(())
}

/// Zip up the previous logs and start a new log file.
/// This returns the path to the new log file.
///
/// returns: `io::Result<PathBuf>`
fn zip_logs() -> io::Result<PathBuf> {
    let log_path = get_exe_parent_dir().join("ssw-logs");
    create_dir_if_not_exists(&log_path)?;
    let latest_log = log_path.join("latest.log");
    if latest_log.exists() {
        // get the creation date of the file as a chrono DateTime, or use the current time if it fails
        let create_time = std::fs::metadata(&latest_log)?
            .created()
            .map_or_else(|_| Local::now(), |value| DateTime::<Local>::from(value));
        let package_name = env!("CARGO_PKG_NAME");
        let dated_name = create_time
            .format(&format!("{}-%Y-%m-%d-%H-%M-%S.log", package_name))
            .to_string();

        // this is where the actual zipping happens
        let archive_path = log_path.join(format!("{}.gz", dated_name));
        let file_handle = std::fs::File::create(archive_path)?;
        let log_data = std::fs::read(&latest_log)?;
        let mut gz = GzBuilder::new()
            .filename(dated_name)
            .write(file_handle, Compression::default());
        gz.write_all(&log_data)?;
        gz.finish()?;
        std::fs::remove_file(&latest_log)?;
    }
    Ok(latest_log)
}

fn init_logger() -> Result<(), Box<dyn std::error::Error>> {
    let config = simplelog::ConfigBuilder::new()
        .set_time_format_custom(format_description!("[[[hour]:[minute]:[second]]"))
        .set_thread_mode(ThreadLogMode::Both)
        .set_target_level(LevelFilter::Off)
        .set_thread_level(LevelFilter::Error)
        .build();

    let log_path = zip_logs()?;
    let log_file = std::fs::File::create(log_path)?;
    // TODO: command line arg to set log level
    CombinedLogger::init(vec![
        TermLogger::new(
            simplelog::LevelFilter::Info,
            config.clone(),
            TerminalMode::Mixed,
            ColorChoice::Auto,
        ),
        WriteLogger::new(LevelFilter::Info, config, log_file),
    ])?;
    Ok(())
}
