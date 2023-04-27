use std::{io, path::PathBuf, process::Stdio};

use getset::Getters;
use log::{debug, error, info, warn};
use tokio::{
    io::AsyncWriteExt,
    process::{Child, Command},
    select,
    sync::mpsc::Sender,
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

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

pub struct MinecraftServer {
    jar_path: PathBuf,
}

impl MinecraftServer {
    pub fn new(jar_path: PathBuf) -> Self {
        let jar_path = dunce::canonicalize(&jar_path).unwrap_or(jar_path);
        Self { jar_path }
    }

    fn start(&mut self) -> io::Result<(JoinHandle<()>, MinecraftServerSenders)> {
        const DEFAULT_MINECRAFT_PORT: u16 = 25565;
        debug!("Jar path: {}", self.jar_path.display());
        // TODO: get java executable
        // this will be a PathBuf or &Path
        let java_executable = "java";
        // TODO: check if the java version is valid for the server version
        // TODO: load config
        // TODO: load server.properties and set the port from there
        let port = DEFAULT_MINECRAFT_PORT;
        // TODO: patch Log4Shell
        info!("Starting Minecraft server on port {port}");
        // TODO: load these from config
        let min_memory_in_mb = 256;
        let max_memory_in_mb = 1024;
        let min_mem_arg = format!("-Xms{min_memory_in_mb}M");
        let max_mem_arg = format!("-Xmx{max_memory_in_mb}M");

        let process_args = vec![
            min_mem_arg.as_str(),
            max_mem_arg.as_str(),
            "-jar",
            self.jar_path.to_str().unwrap(),
            "nogui",
        ];
        let wd = self.jar_path.parent().unwrap();
        // TODO: extend process_args with extra args from config
        debug!("Starting process with args: {:?}", process_args);
        // TODO: set state to starting
        let mut process = Command::new(java_executable)
            .current_dir(wd)
            .args(process_args)
            .stderr(Stdio::piped())
            .stdin(Stdio::piped())
            // .stdout(Stdio::piped())
            .spawn()?;
        let proc_stdin_token = CancellationToken::new();
        let (stdin_handle, stdin_sender) = pipe_stdin(&mut process, proc_stdin_token.clone());

        let stderr_handle = {
            let mut stderr = process.stderr.take().expect("process stderr is not piped");
            tokio::spawn(async move {
                // copy stderr to stdout
                if let Err(e) = tokio::io::copy(&mut stderr, &mut tokio::io::stdout()).await {
                    error!("Error copying process stderr to stdout: {e}");
                };
            })
        };

        // TODO: pipe AND MONITOR stdout with regexes

        let handles = vec![
            (stdin_handle, Some(proc_stdin_token)),
            (stderr_handle, None),
        ];

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
            // TODO: set state to stopped
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
                    (server_exit_handle, server_senders) = match server.start() {
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
