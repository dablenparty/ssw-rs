#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

mod manifest;
mod mc_version;
mod minecraft;
mod proxy;
mod util;

use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

use crate::{
    manifest::refresh_manifest,
    minecraft::{MinecraftServer, DEFAULT_MC_PORT},
};
use chrono::{DateTime, Local};
use clap::Parser;
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

#[derive(Debug)]
pub enum Event {
    StdinMessage(String),
    McPortRequest,
}

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct CommandLineArgs {
    #[clap(forbid_empty_values = true)]
    /// The path to the Minecraft server jar file.
    server_jar: PathBuf,

    #[clap(short, long, value_parser, default_value_t = LevelFilter::Info)]
    /// The log level to use.
    log_level: LevelFilter,

    #[clap(short, long, takes_value = false)]
    /// Whether to refresh the Minecraft server manifest.
    refresh_manifest: bool,
}

const EXIT_COMMAND: &str = "exit";

#[tokio::main]
async fn main() -> io::Result<()> {
    let args = CommandLineArgs::parse();
    if let Err(e) = init_logger(args.log_level) {
        error!("failed to initialize logger: {:?}", e);
        std::process::exit(1);
    }
    debug!("Parsed command line arguments: {:?}", args);
    let cargo_version = env!("CARGO_PKG_VERSION");
    println!("SSW Console v{}", cargo_version);
    if args.refresh_manifest {
        info!("Refreshing Minecraft server manifest...");
        if let Err(e) = refresh_manifest().await {
            error!("failed to refresh manifest: {:?}", e);
        } else {
            info!("Successfully refreshed Minecraft server manifest.");
        }
    }
    let mut mc_server = MinecraftServer::new(dunce::canonicalize(args.server_jar)?).await;
    if mc_server.ssw_config.mc_version.is_none() || args.refresh_manifest {
        let mc_version_string = try_read_version_from_jar(
            mc_server
                .jar_path()
                .parent()
                .expect("server jar is somehow the root directory"),
        )
        .unwrap_or_else(|e| {
            warn!("error occurred trying to read version from jar: {}", e);
            None
        });
        if let Some(mc_version_string) = mc_version_string {
            info!("Found Minecraft version in jar: {}", mc_version_string);
            mc_server.ssw_config.mc_version = Some(mc_version_string);
        } else {
            // TODO: prompt user for version (or add a command to do so, telling user to use it)
            warn!("Could not find Minecraft version in jar.");
        }
        if let Err(e) = mc_server
            .ssw_config
            .save(&mc_server.get_config_path())
            .await
        {
            error!("failed to save SSW config: {:?}", e);
        }
    }
    let port = mc_server.ssw_config.ssw_port;
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<Event>(100);
    let (proxy_handle, proxy_cancel_token, proxy_tx) = start_proxy_task(port, event_tx.clone());
    //? separate cancel token
    let stdin_handle = start_stdin_task(event_tx.clone(), proxy_cancel_token.clone());
    run_ssw_event_loop(&mut mc_server, proxy_cancel_token, &mut event_rx, proxy_tx).await;
    stdin_handle.await?;
    proxy_handle.await?;
    Ok(())
}

/// Tries to read the Minecraft version from every jar file found in a given directory.
///
/// Newer versions of Minecraft store the version in a `version.json` file packaged in with the server jar.
/// This function tries to read the version from that file.
///
/// The version ID string (e.g., `1.19`) will be returned if it is found, or `None` if it is not.
///
/// # Arguments
///
/// * `server_folder` - The directory containing the server jar.
///
/// # Errors
///
/// An error will be returned if the directory or a jar file cannot be read.
fn try_read_version_from_jar(server_folder: &Path) -> io::Result<Option<String>> {
    // get all jar files, ignoring errors (e.g., if a file is not a jar)
    let jar_files = server_folder
        .read_dir()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            if entry.path().extension()?.to_str()? == "jar" {
                Some(entry.path())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    for jar in jar_files {
        let jar_handle = std::fs::File::open(&jar)?;
        let mut jar_reader = zip::ZipArchive::new(jar_handle)?;
        match jar_reader.by_name("version.json") {
            Ok(version_json) => {
                let parsed: serde_json::Value = serde_json::from_reader(version_json)?;
                return Ok(parsed["id"].as_str().map(String::from));
            }
            Err(e) => {
                warn!(
                    "failed to read version.json from jar file {:?}: {:?}",
                    jar, e
                );
                continue;
            }
        };
    }
    Ok(None)
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
    proxy_tx: Sender<u16>,
) {
    loop {
        let event = event_rx.recv().await;
        if event.is_none() {
            error!("Event channel prematurely closed!");
            break;
        }
        let event = event.unwrap();
        debug!("Received event: {:?}", event);
        let current_server_status = *mc_server
            .status()
            .lock()
            .expect("Failed to lock on server status");
        match event {
            Event::StdinMessage(msg) => {
                let command_with_args: Vec<&str> = msg.split_whitespace().collect();
                let command = command_with_args[0];
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
                    "mc-port" => get_or_set_mc_port(&command_with_args, mc_server).await,
                    _ => {
                        if current_server_status == minecraft::MCServerState::Stopped {
                            error!("Unknown command: {}", command);
                        } else {
                            // put it all back together and send it to the server
                            let command = command_with_args.join(" ");
                            if let Err(e) = mc_server.send_command(command.to_string()).await {
                                error!("Failed to send command to server: {:?}", e);
                            }
                        }
                    }
                }
            }
            Event::McPortRequest => {
                // this really shouldn't be a problem, but just in case
                let port: u16 = mc_server
                    .get_property("server-port")
                    .map_or(DEFAULT_MC_PORT.into(), |v| {
                        v.as_u64().unwrap_or_else(|| DEFAULT_MC_PORT.into())
                    })
                    .try_into()
                    .unwrap_or_else(|_| {
                        error!("The server port is not a valid TCP port. Valid ports are in the range 0-65535");
                        DEFAULT_MC_PORT
                    });
                if let Err(e) = proxy_tx.send(port).await {
                    error!("Failed to send port to proxy: {:?}", e);
                }
            }
        }
    }
}

/// Gets or sets the Minecraft server port, depending on the command arguments.
///
/// # Arguments
///
/// * `command_with_args`: the command and its arguments
/// * `mc_server`: the Minecraft server instance
///
/// returns: `()`
async fn get_or_set_mc_port(command_with_args: &[&str], mc_server: &mut MinecraftServer) {
    if let Some(arg) = command_with_args.get(1) {
        let _: u16 = match arg.parse() {
            Ok(port) => port,
            Err(e) => {
                error!("Invalid Minecraft port: {}", e);
                return;
            }
        };
        mc_server.set_property(
            "server-port".to_string(),
            serde_json::from_str(arg).unwrap(),
        );
        if let Err(e) = mc_server.save_properties().await {
            warn!("Failed to save properties: {}", e);
            if e.kind() == io::ErrorKind::NotFound {
                warn!("server.properties not found, attempting to load it");
                mc_server.load_properties().await;
            }
        } else {
            info!("Minecraft port now set to {}", arg);
        }
    } else {
        // this just reports the port that the server is using, it doesn't check validity
        let port = mc_server
            .get_property("server-port")
            .map_or(DEFAULT_MC_PORT.into(), |v| {
                v.as_u64().unwrap_or_else(|| DEFAULT_MC_PORT.into())
            });
        info!("Current MC port: {}", port);
    }
}

/// Prints the SSW help message to the console
fn print_help() {
    // padding before the command name
    const COMMAND_PADDING: usize = 2;
    let commands = vec![
        ("help", "prints this help message"),
        ("start", "starts the Minecraft server"),
        (EXIT_COMMAND, "stops the Minecraft server and exits SSW"),
        ("mc-port", "show or set the Minecraft server port"),
    ];
    // safe to unwrap as this iterator should never be empty
    let max_command_len = commands
        .iter()
        .map(|(command, _)| command.len())
        .max()
        .unwrap();
    info!("Available SSW commands:");
    for command in commands {
        let padding = " ".repeat(COMMAND_PADDING);
        let help_string = format!(
            "{}{:width$}{}",
            padding,
            command.0,
            command.1,
            width = max_command_len + 2
        );
        info!("{}", help_string);
    }
}

/// Starts the proxy task and returns a handle to it along with its cancellation token
///
/// The proxy task itself returns an `io::Result<()>`.
///
/// # Arguments
///
/// * `port` - The port to listen on
/// * `event_tx` - The event channel sender
///
/// returns: `(JoinHandle<()>, CancellationToken)`
fn start_proxy_task(
    port: u16,
    event_tx: Sender<Event>,
) -> (JoinHandle<()>, CancellationToken, Sender<u16>) {
    let token = CancellationToken::new();
    let cloned_token = token.clone();
    let (proxy_tx, proxy_rx) = tokio::sync::mpsc::channel::<u16>(100);
    let handle = tokio::spawn(async move {
        let inner_clone = cloned_token.clone();
        select! {
            r = run_proxy(port, inner_clone, event_tx, proxy_rx) => {
                if let Err(e) = r {
                    if e.kind() == io::ErrorKind::AddrInUse {
                        error!("Failed to start proxy: port {} is already in use", port);
                        error!("Make sure that no other server is running on this port");
                    } else {
                        error!("Failed to start proxy: {:?}", e);
                    }
                }
            },
            _ = cloned_token.cancelled() => {
                debug!("Proxy task cancelled");
            }
        }
    });
    (handle, token, proxy_tx)
}

/// Start the task that reads from stdin and sends the messages through the given channel
///
/// # Arguments
///
/// * `tx` - The channel to send the messages through
/// * `cancel_token` - The cancellation token to use
///
/// returns: `JoinHandle<()>`
fn start_stdin_task(tx: Sender<Event>, cancel_token: CancellationToken) -> JoinHandle<()> {
    let mut stdin_reader = tokio::io::BufReader::new(tokio::io::stdin());
    tokio::spawn(async move {
        loop {
            let mut buf = String::new();
            select! {
                n = stdin_reader.read_line(&mut buf) => {
                    let n = match n {
                        Ok(n) => n,
                        Err(e) => {
                            error!("Failed to read from stdin: {:?}", e);
                            break;
                        }
                    };
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
/// # Arguments
///
/// * `level_filter` - The log level filter to use
///
/// returns: `Result<(), Box<dyn std::error::Error>>`
fn init_logger(level_filter: LevelFilter) -> Result<(), Box<dyn std::error::Error>> {
    let config = simplelog::ConfigBuilder::new()
        .set_time_format_custom(format_description!("[[[hour]:[minute]:[second]]"))
        .set_thread_mode(ThreadLogMode::Both)
        .set_target_level(LevelFilter::Off)
        .set_thread_level(LevelFilter::Error)
        .build();

    let log_path = zip_logs()?;
    let log_file = std::fs::File::create(log_path)?;
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
