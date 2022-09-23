#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

mod minecraft;
mod proxy;
mod util;

use std::{
    io::{self, Write},
    path::PathBuf,
};

use crate::minecraft::MinecraftServer;
use chrono::{DateTime, Local};
use flate2::{Compression, GzBuilder};
use log::{debug, error, info, warn, LevelFilter};
use simplelog::{
    format_description, ColorChoice, CombinedLogger, TermLogger, TerminalMode, ThreadLogMode,
    WriteLogger,
};
use tokio::sync::mpsc::Receiver;
use tokio::{io::AsyncBufReadExt, select, sync::mpsc::Sender, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use util::{create_dir_if_not_exists, get_exe_parent_dir};

use crate::proxy::run_proxy;

enum Event {
    StdinMessage(String),
}

const EXIT_COMMAND: &str = "exit";

#[tokio::main]
async fn main() -> std::io::Result<()> {
    if let Err(e) = init_logger() {
        error!("failed to initialize logger: {:?}", e);
        std::process::exit(1);
    }
    // TODO: command line arg parser
    let path = std::env::args().nth(1).expect("Missing path to server jar");
    let mut mc_server = minecraft::MinecraftServer::new(dunce::canonicalize(PathBuf::from(path))?);
    let cargo_version = env!("CARGO_PKG_VERSION");
    println!("SSW Console v{}", cargo_version);
    let port = mc_server.ssw_config.ssw_port;
    let (proxy_handle, proxy_cancel_token) = start_proxy_task(port);
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<Event>(100);
    //? separate cancel token
    let stdin_handle = start_stdin_task(event_tx.clone(), proxy_cancel_token.clone());
    run_ssw_event_loop(&mut mc_server, proxy_cancel_token, &mut event_rx).await;
    stdin_handle.await??;
    proxy_handle.await??;
    Ok(())
}

/// Runs the main event loop for SSW. This loop handles all events that are sent to the event channel.
///
/// All errors are handled internally to allow for proper cleanup if one of the tasks fails.
///
/// # Arguments
///
/// * `mc_server`: the Minecraft server instance
/// * `proxy_cancel_token`: the cancel token for the proxy task
/// * `event_rx`: the event channel receiver
///
/// returns: `()`
async fn run_ssw_event_loop(
    mc_server: &mut MinecraftServer,
    proxy_cancel_token: CancellationToken,
    event_rx: &mut Receiver<Event>,
) {
    loop {
        let event = event_rx.recv().await;
        if event.is_none() {
            error!("Event channel prematurely closed!");
            break;
        }
        let event = event.unwrap();
        let current_server_status = *mc_server
            .status()
            .lock()
            .expect("Failed to lock on server status");
        match event {
            Event::StdinMessage(msg) => {
                let command = msg.trim();
                match command {
                    "start" => {
                        if current_server_status == minecraft::MCServerState::Stopped {
                            if let Err(e) = mc_server.run().await {
                                error!("Failed to start server: {:?}", e);
                            }
                        } else {
                            warn!("Server is already running");
                        }
                    }
                    EXIT_COMMAND => {
                        if current_server_status == minecraft::MCServerState::Running
                            || current_server_status == minecraft::MCServerState::Starting
                        {
                            info!("Server is currently running, stopping it first");
                            if let Err(e) = mc_server.stop().await {
                                error!("Failed to send stop command to server: {:?}", e);
                            }
                            if let Err(e) = mc_server.wait_for_exit().await {
                                error!("Failed to wait for server to exit: {}", e);
                            }
                        }
                        proxy_cancel_token.cancel();
                        break;
                    }
                    "help" if current_server_status != minecraft::MCServerState::Running => {
                        print_help();
                    }
                    _ => {
                        if current_server_status == minecraft::MCServerState::Running {
                            if let Err(e) = mc_server.send_command(command.to_string()).await {
                                error!("Failed to send command to server: {:?}", e);
                            }
                        } else {
                            error!("Unknown command: {}", command);
                        }
                    }
                }
            }
        }
    }
}

/// Prints the SSW help message to the console
fn print_help() {
    info!("Available commands:");
    info!("  start - start the server");
    info!("  {} - exit ssw", EXIT_COMMAND);
    info!("  help - show this help message");
}

/// Starts the proxy task and returns a handle to it along with its cancellation token
///
/// The proxy task itself returns an `io::Result<()>`.
///
/// # Arguments
///
/// * `port` - The port to listen on
///
/// returns: `(JoinHandle<io::Result<()>>, CancellationToken)`
fn start_proxy_task(port: u16) -> (JoinHandle<io::Result<()>>, CancellationToken) {
    let token = CancellationToken::new();
    let cloned_token = token.clone();
    let handle = tokio::spawn(async move {
        let inner_clone = cloned_token.clone();
        select! {
            r = run_proxy(port, inner_clone) => {
                r
            },
            _ = cloned_token.cancelled() => {
                Ok(())
            }
        }
    });
    (handle, token)
}

/// Start the task that reads from stdin and sends the messages through the given channel
///
/// # Arguments
///
/// * `tx` - The channel to send the messages through
/// * `cancel_token` - The cancellation token to use
fn start_stdin_task(
    tx: Sender<Event>,
    cancel_token: CancellationToken,
) -> JoinHandle<io::Result<()>> {
    let mut stdin_reader = tokio::io::BufReader::new(tokio::io::stdin());
    tokio::spawn(async move {
        loop {
            let mut buf = String::new();
            select! {
                n = stdin_reader.read_line(&mut buf) => {
                    let n = n?;
                    debug!("Read {} bytes from stdin", n);
                    if n == 0 {
                        warn!("Stdin closed, no more input will be accepted");
                        break;
                    }
                    let is_exit_command = buf.trim() == EXIT_COMMAND;
                    if let Err(e) = tx.send(Event::StdinMessage(buf)).await {
                        error!("Error sending message from stdin task: {}", e);
                    }
                    if is_exit_command {
                        break;
                    }
                },
                _ = cancel_token.cancelled() => {
                    break;
                }
            }
        }
        Ok::<(), io::Error>(())
    })
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
            .map_or_else(|_| Local::now(), DateTime::<Local>::from);
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

/// Initializes the logger
///
/// In debug mode, the logger will log DEBUG and above. In release, it will log INFO and above.
/// Both modes will log to the console and to a file.
///
/// returns: `Result<(), Box<dyn std::error::Error>>`
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
    let level_filter = if cfg!(debug_assertions) {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };
    CombinedLogger::init(vec![
        TermLogger::new(
            level_filter,
            config.clone(),
            TerminalMode::Mixed,
            ColorChoice::Auto,
        ),
        WriteLogger::new(level_filter, config, log_file),
    ])?;
    Ok(())
}
