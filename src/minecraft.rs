use std::{io, path::PathBuf, process::Stdio, str::FromStr, time::Duration};

use getset::Getters;
use java_properties::PropertiesIter;
use log::{debug, error, info, warn};
use thiserror::Error;
use tokio::{
    io::AsyncWriteExt,
    process::{Child, Command},
    select,
    sync::mpsc::Sender,
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::config::SswConfig;

use self::{restart_task::begin_restart_task, shutdown_task::begin_shutdown_task};

mod ping_task;
mod restart_task;
mod shutdown_task;

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

#[derive(Getters)]
#[get = "pub"]
struct MinecraftServerSenders {
    stdin: Sender<String>,
    // state
}

#[derive(Debug, Error)]
pub enum MinecraftServerError {
    #[error("Failed to start Minecraft server: {0}")]
    StartFailed(#[from] io::Error),
    #[error("Config error: {0}")]
    SswConfigError(#[from] crate::config::SswConfigError),
    #[error("Failed to read server.properties: {0}")]
    ServerPropertiesError(#[from] java_properties::PropertiesError),
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
        server_token: CancellationToken,
    ) -> Result<(JoinHandle<()>, MinecraftServerSenders), MinecraftServerError> {
        const DEFAULT_MC_PORT: u16 = 25565;
        debug!("Jar path: {}", self.jar_path.display());
        // TODO: get java executable
        // this will be a PathBuf or &Path
        let java_executable = "java";
        let config = {
            let config_path = self.jar_path.with_file_name("ssw-config.toml");
            let config = if self.config.is_some() || config_path.exists() {
                SswConfig::try_from(config_path).unwrap_or_else(|e| {
                    error!("Error loading config: {e}");
                    SswConfig::default()
                })
            } else {
                let config = SswConfig::default();
                config.save(&config_path).await?;
                config
            };
            self.config = Some(config);
            self.config.as_ref().unwrap()
        };
        debug!("Loaded config: {config:#?}");
        // TODO: try to read the minecraft version from the jar manifest
        if config.mc_version().is_none() {
            error!("The Minecraft version is not set in the config");
            return Err(crate::config::SswConfigError::MissingMinecraftVersion)?;
        }
        // TODO: check if the java version is valid for the server version
        // TODO: patch Log4Shell
        let port: u16 = self.get_port();
        info!("Starting Minecraft server on port {port}");
        let min_memory_in_mb = *config.min_memory_in_mb();
        let max_memory_in_mb = *config.max_memory_in_mb();
        let min_mem_arg = format!("-Xms{min_memory_in_mb}M");
        let max_mem_arg = format!("-Xmx{max_memory_in_mb}M");

        let mut process_args = vec![min_mem_arg.as_str(), max_mem_arg.as_str()];
        process_args.extend(
            config
                .extra_jvm_args()
                .iter()
                .map(std::string::String::as_str),
        );
        process_args.extend(vec!["-jar", self.jar_path.to_str().unwrap(), "nogui"]);

        let wd = self.jar_path.parent().unwrap();
        debug!("Starting process with args: {:?}", process_args);
        let mut process = Command::new(java_executable)
            .current_dir(wd)
            .args(process_args)
            .stdin(Stdio::piped())
            .spawn()?;
        let proc_stdin_token = CancellationToken::new();
        let (stdin_handle, stdin_sender) = pipe_stdin(&mut process, proc_stdin_token.child_token());

        let mut handles = vec![(stdin_handle, proc_stdin_token)];

        let senders = MinecraftServerSenders {
            stdin: stdin_sender,
        };

        let restart_duration = Duration::from_secs_f32(*config.restart_after_hrs() * 3600.0);
        let shutdown_duration = Duration::from_secs_f32(*config.shutdown_after_mins() * 60.0);

        let exit_handle = tokio::spawn(async move {
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
                    info!("Server task cancelled, killing server process");
                    if let Err(e) = process.kill().await {
                        error!("Error killing server process: {e}");
                    }
                }
            }
            for (handle, token) in handles {
                token.cancel();
                if let Err(e) = handle.await {
                    error!("Error waiting on child task: {e}");
                }
            }
        });

        Ok((exit_handle, senders))
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
pub fn begin_server_task(
    jar_path: PathBuf,
    running_tx: Sender<bool>,
    token: CancellationToken,
) -> (JoinHandle<()>, Sender<ServerTaskRequest>) {
    let (server_task_tx, mut server_task_rx) = tokio::sync::mpsc::channel::<ServerTaskRequest>(5);
    let inner_tx = server_task_tx.clone();
    let task_handle = tokio::spawn(async move {
        let mut server = MinecraftServer::new(jar_path);
        let mut server_exit_handle: Option<JoinHandle<()>> = None;
        let mut server_senders = None;
        loop {
            let message = select! {
                r = server_task_rx.recv() => {
                    r.unwrap_or_else(|| {
                        warn!("Server task channel closed, killing server");
                        ServerTaskRequest::Kill
                    })
                }
                _ = token.cancelled() => {
                    debug!("Server message task cancelled");
                    break;
                }
            };
            let server_is_running = !server_exit_handle
                .as_ref()
                .map_or(true, tokio::task::JoinHandle::is_finished);
            debug!("Received server task message: {message:?}");
            match message {
                ServerTaskRequest::Start => {
                    if server_is_running {
                        warn!("Server is already running");
                        continue;
                    }
                    (server_exit_handle, server_senders) =
                        match server.start(inner_tx.clone(), token.child_token()).await {
                            Ok((h, s)) => (Some(h), Some(s)),
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
                    if let Some(ref senders) = server_senders {
                        info!("Server is running, stopping it");
                        let sender = senders.stdin();
                        if let Err(e) = sender.send("stop".to_string()).await {
                            error!("Error sending stop command to server: {e}");
                        }
                    }
                    if let Some(handle) = server_exit_handle.take() {
                        info!("Waiting for server to stop");
                        if let Err(e) = handle.await {
                            error!("Error waiting for server to stop: {e}");
                        }
                    }
                    info!("Server stopped");
                }
                ServerTaskRequest::Restart => {
                    inner_tx.send(ServerTaskRequest::Stop).await.unwrap();
                    inner_tx.send(ServerTaskRequest::Start).await.unwrap();
                }
                ServerTaskRequest::Command(command) => {
                    if let Some(ref senders) = server_senders {
                        let sender = senders.stdin();
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
