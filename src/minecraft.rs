use std::{io, path::PathBuf, process::Stdio};

use crossbeam::channel::Sender;
use getset::Getters;
use log::{debug, error, info};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    select,
    task::JoinHandle,
};

fn pipe_stdin(process: &mut Child) -> (JoinHandle<()>, Sender<String>) {
    let (stdin_tx, stdin_rx) = crossbeam::channel::unbounded::<String>();
    let mut stdin = process.stdin.take().expect("process stdin is not piped");
    // TODO: cancellation token
    let handle = tokio::spawn(async move {
        loop {
            match stdin_rx.recv() {
                Ok(msg) => {
                    if let Err(e) = stdin.write_all(msg.as_bytes()).await {
                        error!("Error writing to process stdin: {e}");
                    }
                }
                Err(e) => {
                    error!("Process stdin closed: {e}");
                    break;
                }
            };
        }
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
        let jar_path = jar_path.canonicalize().unwrap_or(jar_path);
        Self { jar_path }
    }

    fn start(&mut self) -> io::Result<(Child, MinecraftServerSenders)> {
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
        info!("Starting Minecraft server on port {}", port);
        // TODO: load these from config
        let min_memory_in_mb = 256;
        let max_memory_in_mb = 1024;
        let min_mem_arg = format!("-Xms{}M", min_memory_in_mb);
        let max_mem_arg = format!("-Xmx{}M", max_memory_in_mb);

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
            // .stderr(Stdio::piped())
            .stdin(Stdio::piped())
            // .stdout(Stdio::piped())
            .spawn()?;
        // TODO: start stdin pipe
        let (stdin_handle, stdin_sender) = pipe_stdin(&mut process);

        let senders = MinecraftServerSenders {
            stdin: stdin_sender,
        };

        Ok((process, senders))
    }
}

pub enum ServerTaskRequest {
    Start,
    Stop,
    Restart,
    Kill,
    Status,
    Command(String),
}

pub fn begin_server_task(jar_path: PathBuf) -> (JoinHandle<()>, Sender<ServerTaskRequest>) {
    let (server_task_tx, server_task_rx) = crossbeam::channel::unbounded::<ServerTaskRequest>();
    let task_handle = tokio::spawn(async move {
        let mut server = MinecraftServer::new(jar_path);
        let mut server_handle = None;
        let mut server_senders = None;
        loop {
            let message = server_task_rx.recv().unwrap_or_else(|e| {
                error!("Error receiving message: {e}");
                ServerTaskRequest::Kill
            });
            match message {
                ServerTaskRequest::Start => {
                    if server_handle.is_some() {
                        error!("Server is already running");
                        continue;
                    }
                    info!("Starting server");
                    (server_handle, server_senders) = match server.start() {
                        Ok((mut child, senders)) => {
                            let handle = tokio::spawn(async move {
                                if let Err(e) = child.wait().await {
                                    error!("Error waiting for server process: {e}");
                                }
                                // TODO: set state to stopped
                            });
                            (Some(handle), Some(senders))
                        }
                        Err(e) => {
                            error!("Error starting server: {e}");
                            continue;
                        }
                    };
                }
                ServerTaskRequest::Stop => {
                    todo!("Stopping server");
                }
                ServerTaskRequest::Restart => {
                    todo!("Restarting server");
                }
                ServerTaskRequest::Kill => {
                    todo!("Killing server");
                }
                ServerTaskRequest::Status => {
                    todo!("Getting server status");
                }
                ServerTaskRequest::Command(command) => {
                    if let Some(ref senders) = server_senders {
                        debug!("Sending command to server: {command}");
                        let sender = senders.stdin();
                        if let Err(e) = sender.send(command) {
                            error!("Error sending command to server: {e}");
                        }
                    }
                }
            }
        }
    });
    (task_handle, server_task_tx)
}
