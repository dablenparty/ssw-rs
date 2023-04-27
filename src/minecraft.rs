use std::{io, path::PathBuf, process::Stdio};

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

mod restart_task;

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
                    info!("Process stdin pipe cancelled");
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

    /// Get the port the server is running on from the server.properties file
    /// If the file does not exist, or the port is not set, the default Minecraft port `25565`
    /// is returned.
    pub fn get_port(&self) -> u16 {
        const DEFAULT_MINECRAFT_PORT: u16 = 25565;
        let properties_path = self.jar_path.with_file_name("server.properties");
        std::fs::File::open(properties_path).map_or(DEFAULT_MINECRAFT_PORT, |f| {
            let properties_reader = std::io::BufReader::new(f);
            PropertiesIter::new(properties_reader)
                .into_iter()
                .find_map(|r| {
                    if let Ok(line) = r {
                        match line.consume_content() {
                            java_properties::LineContent::KVPair(k, v) if k == "server-port" => {
                                v.parse().ok()
                            }
                            _ => None,
                        }
                    } else {
                        None
                    }
                })
                .unwrap_or(DEFAULT_MINECRAFT_PORT)
        })
    }

    async fn start(
        &mut self,
    ) -> Result<(JoinHandle<()>, MinecraftServerSenders), MinecraftServerError> {
        debug!("Jar path: {}", self.jar_path.display());
        // TODO: get java executable
        // this will be a PathBuf or &Path
        let java_executable = "java";
        // TODO: load config
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
        info!("Starting Minecraft server on port {}", self.get_port());
        let min_memory_in_mb = *config.min_memory_in_mb();
        let max_memory_in_mb = *config.max_memory_in_mb();
        let min_mem_arg = format!("-Xms{min_memory_in_mb}M");
        let max_mem_arg = format!("-Xmx{max_memory_in_mb}M");

        let mut process_args = vec![min_mem_arg.as_str(), max_mem_arg.as_str()];
        process_args.extend(config.extra_jvm_args().iter().map(|s| s.as_str()));
        process_args.extend(vec!["-jar", self.jar_path.to_str().unwrap(), "nogui"]);

        let wd = self.jar_path.parent().unwrap();
        debug!("Starting process with args: {:?}", process_args);
        let mut process = Command::new(java_executable)
            .current_dir(wd)
            .args(process_args)
            .stdin(Stdio::piped())
            .spawn()?;
        let proc_stdin_token = CancellationToken::new();
        let (stdin_handle, stdin_sender) = pipe_stdin(&mut process, proc_stdin_token.clone());

        let handles = vec![(stdin_handle, Some(proc_stdin_token))];

        let senders = MinecraftServerSenders {
            stdin: stdin_sender,
        };

        let exit_handle = tokio::spawn(async move {
            if let Err(e) = process.wait().await {
                error!("Error waiting for server process: {e}");
            }
            for (handle, token) in handles {
                if let Some(token) = token {
                    token.cancel();
                }
                if let Err(e) = handle.await {
                    error!("Error waiting on child task: {e}");
                }
            }
        });

        Ok((exit_handle, senders))
    }
}

#[derive(Debug)]
pub enum ServerTaskRequest {
    Start,
    Stop,
    Restart,
    Kill,
    Command(String),
}

pub fn begin_server_task(
    jar_path: PathBuf,
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
                    info!("Server message task cancelled");
                    break;
                }
            };
            let server_is_running = !server_exit_handle
                .as_ref()
                .map_or(true, |f| f.is_finished());
            match message {
                ServerTaskRequest::Start => {
                    if server_is_running {
                        error!("Server is already running");
                        continue;
                    }
                    info!("Starting server");
                    (server_exit_handle, server_senders) = match server.start().await {
                        Ok((handle, senders)) => (Some(handle), Some(senders)),
                        Err(e) => {
                            error!("Error starting server: {e}");
                            continue;
                        }
                    };
                }
                ServerTaskRequest::Stop => {
                    if !server_is_running {
                        warn!("Requested to stop server, but it is not running");
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
                        info!(
                            "Waiting for server to stop (if this takes too long, kill it manually)"
                        );
                        if let Err(e) = handle.await {
                            error!("Error waiting for server to stop: {e}");
                        }
                    }
                }
                ServerTaskRequest::Restart => {
                    info!("Restarting server");
                    inner_tx.send(ServerTaskRequest::Stop).await.unwrap();
                    inner_tx.send(ServerTaskRequest::Start).await.unwrap();
                }
                ServerTaskRequest::Kill => {
                    info!("Killing server task");
                    if !server_is_running {
                        debug!("Minecraft server not running, skipping stop command");
                        token.cancel();
                        break;
                    }
                    // send stop command
                    if let Some(ref senders) = server_senders {
                        info!("Server is still running, stopping it");
                        let sender = senders.stdin();
                        if let Err(e) = sender.send("stop".to_string()).await {
                            error!("Error sending stop command to server: {e}");
                        }
                    }
                    if let Some(handle) = server_exit_handle.take() {
                        info!(
                            "Waiting for server to stop (if this takes too long, kill it manually)"
                        );
                        if let Err(e) = handle.await {
                            error!("Error waiting for server to stop: {e}");
                        }
                    }
                }
                ServerTaskRequest::Command(command) => {
                    if let Some(ref senders) = server_senders {
                        if command.trim_end() == "stop" {
                            inner_tx
                                .send(ServerTaskRequest::Stop)
                                .await
                                .unwrap_or_else(|e| {
                                    error!("Error sending stop command to server task: {e}");
                                });
                            continue;
                        }
                        debug!("Sending command to server: {command}");
                        let sender = senders.stdin();
                        if let Err(e) = sender.send(command).await {
                            error!("Error sending command to server: {e}");
                        }
                    }
                }
            }
        }
    });
    (task_handle, server_task_tx)
}
