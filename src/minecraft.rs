use std::{collections::HashMap, io, path::PathBuf, process::Stdio, str::FromStr, time::Duration};

use duration_string::DurationString;
use java_properties::PropertiesIter;
use log::{debug, error, info, warn};
use thiserror::Error;
use tokio::{
    io::AsyncWriteExt,
    process::{Child, Command},
    select,
    sync::mpsc::{self, Sender},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::config::SswConfig;

use self::{
    listener_task::begin_listener_task,
    restart_task::begin_restart_task,
    shutdown_task::begin_shutdown_task,
    state_task::{begin_state_task, ServerState},
};

mod listener_task;
mod manifest;
mod ping_task;
mod restart_task;
mod shutdown_task;
mod state_task;

fn pipe_stdin(process: &mut Child, token: CancellationToken) -> (JoinHandle<()>, Sender<String>) {
    let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::channel::<String>(3);
    let mut stdin = process.stdin.take().expect("process stdin is not piped");
    let handle = tokio::spawn(async move {
        loop {
            select! {
                o = stdin_rx.recv() => {
                    if let Some(mut msg) = o {
                        debug!("Sending message to process stdin: {msg}");
                        if !msg.ends_with('\n') {
                            msg.push('\n');
                        }
                        if let Err(e) = stdin.write_all(msg.as_bytes()).await {
                            error!("Error writing to process stdin: {e}");
                        }
                        stdin.flush().await.unwrap_or_else(|e| {
                            error!("Error flushing process stdin: {e}");
                        });
                    } else {
                        error!("Process stdin closed");
                        break;
                    }
                }
                _ = token.cancelled() => {
                    debug!("Process stdin pipe cancelled");
                    break;
                }
            }
        }
        stdin.shutdown().await.unwrap_or_else(|e| {
            error!("Error shutting down process stdin: {e}");
        });
    });
    (handle, stdin_tx)
}

#[derive(Debug, Error)]
pub enum MinecraftServerError {
    #[error("Failed to start Minecraft server: {0}")]
    StartFailed(#[from] io::Error),
    #[error("Config error: {0}")]
    SswConfigError(#[from] crate::config::SswConfigError),
    #[error("Failed to read server.properties: {0}")]
    ServerPropertiesError(#[from] java_properties::PropertiesError),
    #[error("Failed to update server state: {0}")]
    ServerStateError(#[from] mpsc::error::SendError<ServerState>),
}

pub struct MinecraftServer<'m> {
    jar_path: PathBuf,
    config: Option<SswConfig<'m>>,
}

impl MinecraftServer<'_> {
    pub fn new(jar_path: PathBuf) -> Self {
        let jar_path = dunce::canonicalize(&jar_path).unwrap_or(jar_path);
        Self {
            jar_path,
            config: None,
        }
    }

    /// Convenience method to get the port the server is running on
    /// This is equivalent to calling `get_property("server-port").unwrap_or(25565)`
    /// as `25565` is the default port for Minecraft servers.
    pub fn get_port(&self) -> u16 {
        self.get_property("server-port").unwrap_or(25565)
    }

    /// Convenience method to get the address the server is running on
    pub fn get_address(&self) -> String {
        let port = self.get_port();
        let address = self.get_property::<String>("server-ip").map_or_else(
            || "0.0.0.0".into(),
            |v| if v.is_empty() { "0.0.0.0".into() } else { v },
        );
        format!("{address}:{port}")
    }

    /// Get the value of a property from the server.properties file, if it exists
    /// If the file does not exist, or the property is not set, `None` is returned.
    ///
    /// # Arguments
    ///
    /// * `key` - The key of the property to get
    pub fn get_property<T>(&self, key: &str) -> Option<T>
    where
        T: FromStr,
    {
        let properties_path = self.jar_path.with_file_name("server.properties");
        std::fs::File::open(properties_path).ok().and_then(|f| {
            let properties_reader = std::io::BufReader::new(f);
            PropertiesIter::new(properties_reader)
                .into_iter()
                .find_map(|r| {
                    if let Ok(line) = r {
                        match line.consume_content() {
                            java_properties::LineContent::KVPair(k, v) if k == key => {
                                v.parse().ok()
                            }
                            _ => None,
                        }
                    } else {
                        None
                    }
                })
        })
    }

    /// Start the Minecraft server, returning a handle to the task and a channel to send messages to the server.
    /// The task will run until the server is stopped, and if SSW is configured to listen for connections, it will
    /// also do that.
    ///
    /// # Arguments
    ///
    /// * `restart_token` - A token for the restart task that is cancelled when the server is stopped.
    /// * `server_token` - A token for the server task that should only be cancelled in the event of a fatal error.
    async fn start(
        &mut self,
        server_sender: Sender<ServerTaskRequest>,
        status_sender: Sender<ServerState>,
        server_token: CancellationToken,
    ) -> Result<(JoinHandle<()>, mpsc::Sender<String>), MinecraftServerError> {
        const DEFAULT_MC_PORT: u16 = 25565;
        let config = {
            let config_path = self.jar_path.with_file_name("ssw-config.toml");
            let config = SswConfig::load(&config_path).await?;
            self.config = Some(config);
            self.config.as_ref().unwrap()
        };
        debug!("Loaded config: {config:#?}");
        if config.mc_version().is_none() {
            error!("The Minecraft version is not set in the config");
            return Err(crate::config::SswConfigError::MissingMinecraftVersion)?;
        }
        let java_executable = config.java_path();
        // TODO: check if the java version is valid for the server version
        // TODO: patch Log4Shell
        let port = self.get_port();
        info!("Starting Minecraft server on port {port}");
        let min_mem_arg = format!("-Xms{}M", config.min_memory_in_mb());
        let max_mem_arg = format!("-Xmx{}M", config.max_memory_in_mb());

        let mut process_args = vec![min_mem_arg.as_str(), max_mem_arg.as_str()];
        process_args.extend(
            config
                .extra_jvm_args()
                .iter()
                .map(std::string::String::as_str),
        );
        process_args.extend(vec!["-jar", self.jar_path.to_str().unwrap(), "nogui"]);

        let wd = self.jar_path.parent().unwrap();
        debug!("Starting process with args: {process_args:?}");
        let mut process = Command::new(java_executable)
            .current_dir(wd)
            .args(process_args)
            .stdin(Stdio::piped())
            .spawn()?;
        status_sender.send(ServerState::On).await?;
        let proc_stdin_token = CancellationToken::new();
        let (stdin_handle, stdin_sender) = pipe_stdin(&mut process, proc_stdin_token.child_token());

        let mut handles = vec![(stdin_handle, proc_stdin_token)];

        let restart_duration = Duration::from_secs_f32(*config.restart_after_hrs() * 3600.0);
        let shutdown_duration = Duration::from_secs_f32(*config.shutdown_after_mins() * 60.0);

        let exit_handle = tokio::spawn(async move {
            // sometimes modded servers will take a long time to shut down, so we'll wait a bit before killing it
            const WAIT_TO_KILL_DURATION: Duration = Duration::from_secs(10);
            if !restart_duration.is_zero() {
                let token = server_token.child_token();
                let restart_task =
                    begin_restart_task(restart_duration, server_sender.clone(), token.clone());
                handles.push((restart_task, token));
            }
            if !shutdown_duration.is_zero() {
                let token = server_token.child_token();
                let shutdown_task = begin_shutdown_task(
                    shutdown_duration,
                    format!("127.0.0.1:{port}"),
                    server_sender.clone(),
                    token.clone(),
                );
                handles.push((shutdown_task, token));
            }
            select! {
                r = process.wait() => {
                    if let Err(e) = r {
                        error!("Error waiting for server process: {e}");
                    }
                }
                _ = server_token.cancelled() => {
                    info!("Server task cancelled, killing server process in {}", DurationString::from(WAIT_TO_KILL_DURATION));
                    select! {
                        r = process.wait() => {
                            if let Err(e) = r {
                                error!("Error waiting for server process: {e}");
                            }
                        }
                        _ = tokio::time::sleep(WAIT_TO_KILL_DURATION) => {
                            if let Err(e) = process.kill().await {
                                error!("Error killing server process: {e}");
                            }
                        }
                    }
                }
            }
            if let Err(e) = status_sender.send(ServerState::Off).await {
                error!("Error sending server state: {e}");
            }
            for (handle, token) in handles {
                token.cancel();
                if let Err(e) = handle.await {
                    error!("Error waiting on child task: {e}");
                }
            }
        });

        Ok((exit_handle, stdin_sender))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ServerTaskRequest {
    Start,
    Stop,
    Restart,
    Kill,
    IsRunning,
    Command(String),
}

/// Begins the server task, which will start the server and handle all requests to it.
/// Returns a handle to the task, and a channel to send requests to the task.
/// The task will exit when the Kill request is sent.
///
/// # Arguments
///
/// * `jar_path` - The path to the server jar file
/// * `running_tx` - A channel to send a boolean representing the running status of the server
/// * `token` - A cancellation token that will be cancelled when the server task exits
#[allow(clippy::too_many_lines)]
pub fn begin_server_task(
    jar_path: PathBuf,
    running_tx: Sender<bool>,
    token: CancellationToken,
) -> (JoinHandle<()>, Sender<ServerTaskRequest>) {
    // this function is long, but it's mostly due to internal error handling. If something goes wrong, we want to
    // continue running the server task, but we also want to log the error and notify the user.
    let (server_task_tx, mut server_task_rx) = tokio::sync::mpsc::channel::<ServerTaskRequest>(5);
    let inner_tx = server_task_tx.clone();
    let task_handle = tokio::spawn(async move {
        let mut server = MinecraftServer::new(jar_path);
        let mut config = server.config.clone().unwrap_or_default();
        let mut server_senders = None;
        let mut handle_map = HashMap::<String, (JoinHandle<()>, CancellationToken)>::new();
        let state_token = token.child_token();
        let (state_handle, (state_tx, mut state_watch)) = begin_state_task(state_token.clone());
        handle_map.insert("state".into(), (state_handle, state_token));
        loop {
            let message = select! {
                r = server_task_rx.recv() => {
                    r.unwrap_or_else(|| {
                        warn!("Server task channel closed, killing server");
                        ServerTaskRequest::Kill
                    })
                }
                r = state_watch.changed() => {
                    if let Err(e) = r {
                        error!("Error receiving server state: {e}");
                        continue;
                    }
                    if *state_watch.borrow_and_update() == ServerState::Off && *config.auto_start() {
                        let listener_token = token.child_token();
                        let handle = begin_listener_task(server.get_address(), inner_tx.clone(), listener_token.clone());
                        handle_map.insert("listener".into(), (handle, listener_token));
                    } else {
                        config = server.config.clone().unwrap_or_default();
                    }
                    continue;
                }
                _ = token.cancelled() => {
                    debug!("Server message task cancelled");
                    for (id, (handle, child_token)) in handle_map.drain() {
                        child_token.cancel();
                        if let Err(e) = handle.await {
                            error!("Error waiting on child task '{id}': {e}");
                        }
                    }
                    break;
                }
            };
            let server_is_running = *state_watch.borrow() == ServerState::On;
            debug!("Received server task message: {message:?}");
            match message {
                ServerTaskRequest::Start => {
                    if server_is_running {
                        warn!("Server is already running");
                        continue;
                    }
                    if let Some((handle, token)) = handle_map.remove("listener").take() {
                        token.cancel();
                        if let Err(e) = handle.await {
                            error!("Error waiting for port listener to stop: {e}");
                        }
                    }
                    let server_token = token.child_token();
                    match server
                        .start(inner_tx.clone(), state_tx.clone(), server_token.clone())
                        .await
                    {
                        Ok((h, s)) => {
                            server_senders = Some(s);
                            handle_map.insert("server".into(), (h, server_token));
                        }
                        Err(e) => {
                            error!("Error starting server: {e}");
                            continue;
                        }
                    };
                }
                ServerTaskRequest::Kill | ServerTaskRequest::Stop => {
                    if !server_is_running {
                        warn!("Requested to stop server, but it is not running");
                        if message == ServerTaskRequest::Kill {
                            token.cancel();
                        }
                        continue;
                    }
                    if let Some(ref sender) = server_senders {
                        info!("Server is running, stopping it");
                        if let Err(e) = sender.send("stop".to_string()).await {
                            error!("Error sending stop command to server: {e}");
                        }
                    }
                    if let Some((handle, token)) = handle_map.remove("server").take() {
                        info!("Waiting for server to stop");
                        if let Err(e) = handle.await {
                            error!("Error waiting for server to stop: {e}");
                            token.cancel();
                        }
                    }
                    info!("Server stopped");
                }
                ServerTaskRequest::Restart => {
                    inner_tx.send(ServerTaskRequest::Stop).await.unwrap();
                    inner_tx.send(ServerTaskRequest::Start).await.unwrap();
                }
                ServerTaskRequest::Command(command) => {
                    if let Some(ref sender) = server_senders {
                        if let Err(e) = sender.send(command).await {
                            error!("Error sending command to server: {e}");
                        }
                    }
                }
                ServerTaskRequest::IsRunning => {
                    if let Err(e) = running_tx.send(server_is_running).await {
                        error!("Error sending running status to UI: {e}");
                    }
                }
            }
        }
    });
    (task_handle, server_task_tx)
}
