#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::uninlined_format_args)]

mod log4j;
mod manifest;
mod mc_version;
mod minecraft;
mod proxy;
mod ssw_error;
mod util;

use std::{
    io::{self, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::{
    manifest::refresh_manifest,
    minecraft::{MinecraftServer, DEFAULT_MC_PORT},
};
use chrono::{DateTime, Local};
use clap::Parser;
use duration_string::DurationString;
use flate2::{Compression, GzBuilder};
use log::{debug, error, info, warn, LevelFilter};
use manifest::load_versions;
use minecraft::{MCServerState, SswConfig};
use proxy::ProxyConfig;
use simplelog::{
    format_description, ColorChoice, CombinedLogger, TermLogger, TerminalMode, ThreadLogMode,
    WriteLogger,
};
use tokio::sync::{broadcast, mpsc::Receiver};
use tokio::{io::AsyncBufReadExt, select, sync::mpsc::Sender, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use util::{create_dir_if_not_exists, get_exe_parent_dir};

use crate::proxy::run_proxy;

/// Represents an event that can be sent to the main thread.
#[derive(Debug)]
pub enum SswEvent {
    /// Sent when a string is read from `stdin`.
    StdinMessage(String),
    /// Sent when the proxy thread needs to know the Minecraft port
    McPortRequest,
    /// Sent when another thread forces a shutdown of the server, with a reason.
    ForceShutdown(String),
    /// Sent when another thread forces a restart of the server, with a reason.
    ForceRestart(String),
    /// Sent when another thread forces a startup of the server, with a reason.
    ForceStartup(String),
}

/// Simple Server Wrapper (SSW) is a simple wrapper for Minecraft servers, allowing for easy
/// automation of some simple server management features.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct CommandLineArgs {
    /// The path to the Minecraft server JAR file.
    #[arg(required = true)]
    server_jar: PathBuf,

    /// The log level to use.
    #[arg(short, long, value_parser, default_value_t = LevelFilter::Info)]
    log_level: LevelFilter,

    /// Refreshes the Minecraft version manifest.
    #[arg(short, long)]
    refresh_manifest: bool,

    /// Binds the proxy to the specific address.
    #[arg(short, long, value_parser, default_value_t = String::from("0.0.0.0"))]
    proxy_ip: String,

    /// Disables output from the Minecraft server. This only affects the output that is sent to the
    /// console, not the log file. That is managed by the server itself, and not SSW.
    #[arg(short, long)]
    no_mc_output: bool,
}

const EXIT_COMMAND: &str = "exit";

#[tokio::main]
async fn main() -> io::Result<()> {
    let args = CommandLineArgs::parse();
    if let Err(e) = init_logger(args.log_level) {
        error!("failed to initialize logger: {:?}", e);
        std::process::exit(1);
    }
    debug!("Parsed args: {:#?}", args);
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
    mc_server.show_output = !args.no_mc_output;
    if mc_server.ssw_config.mc_version.is_none() || args.refresh_manifest {
        if let Err(e) = mc_server.load_version().await {
            error!("failed to load version: {}", e);
        }
    }
    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<SswEvent>(100);
    let (proxy_handle, proxy_cancel_token, proxy_tx) = start_proxy_task(
        mc_server.ssw_config.clone(),
        args.proxy_ip,
        mc_server.status(),
        mc_server.subscribe_to_state_changes(),
        event_tx.clone(),
    );
    //? separate cancel token
    let stdin_handle = start_stdin_task(event_tx.clone(), proxy_cancel_token.clone());
    run_ssw_event_loop(
        &mut mc_server,
        proxy_cancel_token,
        (event_tx, event_rx),
        proxy_tx,
    )
    .await;
    stdin_handle.await?;
    proxy_handle.await?;
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
/// * `event_channels`: the event channels
/// * `proxy_tx`: the proxy task's port channel
///
/// returns: `()`
async fn run_ssw_event_loop(
    mc_server: &mut MinecraftServer,
    proxy_cancel_token: CancellationToken,
    event_channels: (Sender<SswEvent>, Receiver<SswEvent>),
    proxy_tx: Sender<u16>,
) {
    let (event_tx, mut event_rx) = event_channels;
    let restart_handle = {
        let state_rx = mc_server.subscribe_to_state_changes();
        let restart_cancel_token = proxy_cancel_token.clone();
        // restart timeout is stored in hours
        let restart_timeout =
            Duration::from_secs_f64(mc_server.ssw_config.restart_timeout * 3600.0);
        let event_tx = event_tx.clone();
        tokio::spawn(handle_restart_task(
            restart_timeout,
            event_tx,
            state_rx,
            restart_cancel_token,
        ))
    };
    loop {
        let event = event_rx.recv().await;
        if event.is_none() {
            error!("Event channel prematurely closed!");
            break;
        }
        let event = event.unwrap();
        let current_server_status = { *mc_server.status().lock().unwrap() };
        match event {
            SswEvent::StdinMessage(msg) => {
                let command_with_args: Vec<&str> = msg.split_whitespace().collect();
                let command = command_with_args[0];
                match command {
                    "start" => {
                        handle_start_command(current_server_status, mc_server).await;
                    }
                    EXIT_COMMAND => {
                        if current_server_status == minecraft::MCServerState::Running
                            || current_server_status == minecraft::MCServerState::Starting
                        {
                            info!("Server is currently running, stopping it first");
                            gracefully_stop_server(mc_server).await;
                        }
                        proxy_cancel_token.cancel();
                        break;
                    }
                    "help" if current_server_status != minecraft::MCServerState::Running => {
                        print_help();
                    }
                    "mc-port" => {
                        if let Err(e) = get_or_set_mc_port(&command_with_args, mc_server).await {
                            error!("Failed to get or set Minecraft port: {}", e);
                        }
                    }
                    "mc-version" => {
                        if let Err(e) = get_or_set_mc_version(&command_with_args, mc_server).await {
                            error!("Failed to get or set Minecraft version: {}", e);
                        }
                    }
                    "ssw-port" => {
                        if let Err(e) = get_or_set_ssw_port(&command_with_args, mc_server).await {
                            error!("Failed to get or set SSW port: {}", e);
                        }
                    }
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
            SswEvent::McPortRequest => {
                // this really shouldn't be a problem, but just in case
                send_port_to_proxy(mc_server, &proxy_tx).await;
            }
            SswEvent::ForceShutdown(reason) => {
                info!("Force shutdown requested: {}", reason);
                gracefully_stop_server(mc_server).await;
            }
            SswEvent::ForceRestart(reason) => {
                info!("Force restart requested: {}", reason);
                if let Err(e) = mc_server.restart().await {
                    error!("Failed to restart server: {}", e);
                }
            }
            SswEvent::ForceStartup(reason) => {
                info!("Force startup requested: {}", reason);
                handle_start_command(current_server_status, mc_server).await;
            }
        }
    }
    if let Err(e) = restart_handle.await {
        error!("Failed to wait on restart task: {}", e);
    }
}

/// Handles the start command. If the server is already running, nothing happens.
///
/// # Arguments
///
/// * `current_server_status`: the current server status
/// * `mc_server`: the Minecraft server instance
async fn handle_start_command(
    current_server_status: MCServerState,
    mc_server: &mut MinecraftServer,
) {
    if current_server_status == minecraft::MCServerState::Stopped {
        if let Err(e) = mc_server.run().await {
            error!("Failed to start server: {}", e);
            if let ssw_error::Error::MissingMinecraftVersion = e {
                error!("Use the 'mc-version' command to set the Minecraft version of the server.");
            }
        }
    } else {
        warn!("Server is already running");
    }
}

/// Wrapper function to monitor the server state and handle the restart timeout accordingly.
///
/// # Arguments
///
/// * `restart_timeout`: the timeout after which the server should be restarted
/// * `event_tx`: the event channel sender
/// * `state_rx`: the state channel receiver
/// * `cancel_token`: the cancel token for the task
async fn handle_restart_task(
    restart_timeout: Duration,
    event_tx: Sender<SswEvent>,
    mut state_rx: broadcast::Receiver<MCServerState>,
    cancel_token: CancellationToken,
) {
    let mut task_handle: Option<JoinHandle<()>> = None;
    loop {
        select! {
            state = state_rx.recv() => {
                if let Err(e) = state {
                    error!("failed to receive state change: {:?}", e);
                    continue;
                }
                match state.unwrap() {
                    MCServerState::Starting => {},
                    MCServerState::Running => {
                        if let Some(handle) = task_handle.take() {
                            warn!("Server restarted before scheduled restart task could run.");
                            handle.abort();
                        }
                        if !restart_timeout.is_zero() {
                            task_handle = Some(tokio::spawn(restart_task(restart_timeout, event_tx.clone())));
                        }
                    },
                    MCServerState::Stopping | MCServerState::Stopped => {
                        if let Some(handle) = task_handle {
                            handle.abort();
                        }
                        task_handle = None;
                    },
                };
            }
            _ = cancel_token.cancelled() => {
                debug!("Restart task cancelled.");
                if let Some(handle) = task_handle {
                    handle.abort();
                }
                break;
            }
        }
    }
}

/// Waits for a specified duration before sending a restart event to the event channel.
/// This will notify the server every hour, then every 15 minutes, then 5 minutes, then
/// 1 minute, finally ending with a 15 second countdown.
///
/// This function is intended to be run as a task.
///
/// # Arguments
///
/// * `wait_for` - The duration to wait before sending the restart event.
/// * `event_tx` - The event channel to send events to.
async fn restart_task(wait_for: Duration, event_tx: Sender<SswEvent>) {
    const MESSAGE_PREFIX: &str = "/me is restarting in";
    const DURATION_SPLITS: [usize; 6] = [3600, 900, 300, 60, 15, 1];
    let mut time_left = wait_for;
    for split in &DURATION_SPLITS {
        let split_duration = Duration::from_secs(*split as u64);
        let mut ticker = tokio::time::interval(split_duration);
        ticker.tick().await; // immediately tick to start
        while time_left > split_duration {
            let msg = format!("{} {}", MESSAGE_PREFIX, DurationString::from(time_left));
            if let Err(e) = event_tx.send(SswEvent::StdinMessage(msg)).await {
                error!("Failed to send restart notification: {}", e);
            }
            ticker.tick().await;
            time_left -= split_duration;
        }
    }
    // last tick
    let msg = format!("{} {}", MESSAGE_PREFIX, DurationString::from(time_left));
    if let Err(e) = event_tx.send(SswEvent::StdinMessage(msg)).await {
        error!("Failed to send restart notification: {}", e);
    }
    if let Err(e) = event_tx
        .send(SswEvent::ForceRestart("Periodic restart".to_string()))
        .await
    {
        error!("Failed to send stop command: {}", e);
    }
}

/// Helper function to log errors when attempting to stop the server.
///
/// # Arguments
///
/// * `mc_server` - The Minecraft server to stop.
async fn gracefully_stop_server(mc_server: &mut MinecraftServer) {
    if let Err(e) = mc_server.stop().await {
        error!("Failed to send stop command to server: {:?}", e);
    }
    if let Err(e) = mc_server.wait_for_exit().await {
        error!("Failed to wait for server to exit: {}", e);
    }
}

/// Sends the Minecraft server's port to the proxy.
///
/// # Arguments
///
/// * `mc_server` - The Minecraft server to get the port from.
/// * `proxy_tx` - The proxy's event sender.
async fn send_port_to_proxy(mc_server: &mut MinecraftServer, proxy_tx: &Sender<u16>) {
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

/// Gets or sets the SSW proxy port.
/// The current implementation requires the whole application to be restarted to apply the change.
///
/// # Arguments
///
/// * `command_with_args`: the command and its arguments
/// * `mc_server`: the Minecraft server instance
///
/// returns: `ssw_error::Result<()>`
async fn get_or_set_ssw_port(
    command_with_args: &[&str],
    mc_server: &mut MinecraftServer,
) -> ssw_error::Result<()> {
    if command_with_args.len() == 1 {
        info!("Current SSW port: {}", mc_server.ssw_config.ssw_port);
    } else {
        let port = command_with_args[1].parse::<u16>()?;
        mc_server.ssw_config.ssw_port = port;
        mc_server.save_config().await?;
        info!(
            "Successfully set SSW port to {}. Please restart the application to apply the changes.",
            port
        );
    }
    Ok(())
}

/// Gets or sets the Minecraft server version based on the command line arguments.
///
/// # Arguments
///
/// * `command_with_args`: the command line arguments
/// * `mc_server`: a reference to the Minecraft server instance
///
/// # Errors
///
/// An error will be returned if the given version is invalid or an IO error occurs.
async fn get_or_set_mc_version(
    command_with_args: &[&str],
    mc_server: &mut MinecraftServer,
) -> io::Result<()> {
    if command_with_args.len() == 1 {
        println!(
            "Minecraft version: {}",
            mc_server
                .ssw_config
                .mc_version
                .as_ref()
                .unwrap_or(&"Unknown".to_string())
        );
    } else {
        let version = command_with_args[1];
        let all_versions = load_versions().await?;
        let _ = all_versions
            .iter()
            .find(|v| v.id == version)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Invalid Minecraft version: {}", version),
                )
            })?;
        mc_server.ssw_config.mc_version = Some(version.to_string());
        mc_server
            .ssw_config
            .save(&mc_server.get_config_path())
            .await?;
        info!("Successfully set Minecraft version.");
    }
    Ok(())
}

/// Gets or sets the Minecraft server port, depending on the command arguments.
///
/// # Arguments
///
/// * `command_with_args`: the command and its arguments
/// * `mc_server`: the Minecraft server instance
///
/// returns: `()`
async fn get_or_set_mc_port(
    command_with_args: &[&str],
    mc_server: &mut MinecraftServer,
) -> ssw_error::Result<()> {
    if let Some(arg) = command_with_args.get(1) {
        let _: u16 = arg.parse()?;
        mc_server.set_property("server-port".to_string(), serde_json::from_str(arg)?);
        if let Err(e) = mc_server.save_properties().await {
            if e.kind() == io::ErrorKind::NotFound {
                warn!("Failed to save properties: {}", e);
                warn!("server.properties not found, attempting to load it");
                mc_server.load_properties().await;
            } else {
                return Err(e.into());
            }
        } else {
            info!("Successfully set Minecraft port");
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
    Ok(())
}

/// Prints the SSW help message to the console
fn print_help() {
    // padding before the command name
    const COMMAND_PADDING: usize = 2;
    // TODO: extract commands to command map
    let commands = vec![
        ("help", "prints this help message"),
        ("start", "starts the Minecraft server"),
        (EXIT_COMMAND, "stops the Minecraft server and exits SSW"),
        ("mc-port [port]", "show or set the Minecraft server port"),
        (
            "mc-version [version]",
            "manually show or set the Minecraft server version",
        ),
        ("ssw-port [port]", "show or set the SSW port"),
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
/// * `ssw_config`: the SSW configuration
/// * `server_state`: a thread-safe reference to the server state
/// * `server_state_rx`: the server state receiver
/// * `event_tx`: an event sender for the main thread
///
/// returns: `(JoinHandle<()>, CancellationToken, Sender<u16>)`
fn start_proxy_task(
    ssw_config: SswConfig,
    proxy_ip: String,
    server_state: Arc<Mutex<MCServerState>>,
    server_state_rx: broadcast::Receiver<MCServerState>,
    event_tx: Sender<SswEvent>,
) -> (JoinHandle<()>, CancellationToken, Sender<u16>) {
    let token = CancellationToken::new();
    let cloned_token = token.clone();
    let (proxy_tx, proxy_rx) = tokio::sync::mpsc::channel::<u16>(100);
    let port = ssw_config.ssw_port;
    let proxy_config = ProxyConfig {
        ssw_config,
        server_state,
        ssw_event_tx: event_tx,
        server_port_rx: proxy_rx,
        server_state_rx,
        ip: proxy_ip,
    };
    let handle = tokio::spawn(async move {
        let inner_clone = cloned_token.clone();
        select! {
            r = run_proxy(proxy_config, inner_clone) => {
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
fn start_stdin_task(tx: Sender<SswEvent>, cancel_token: CancellationToken) -> JoinHandle<()> {
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
                    if let Err(e) = tx.send(SswEvent::StdinMessage(buf)).await {
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
/// returns: `ssw_error::Result<()>`
fn init_logger(level_filter: LevelFilter) -> ssw_error::Result<()> {
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
