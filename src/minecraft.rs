use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use log::{error, info};
use regex::Regex;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt},
    select,
    sync::mpsc::Sender,
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::util::pipe_readable_to_stdout;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MCServerState {
    Stopped = 1,
    Starting = 2,
    Running = 3,
    Stopping = 4,
}

impl Default for MCServerState {
    fn default() -> Self {
        MCServerState::Stopped
    }
}

pub struct SswConfig {
    memory_in_gb: f32,
    restart_timeout: f32,
    shutdown_timeout: f32,
    ssw_port: u32,
    mc_version: Option<String>,
    required_java_version: String,
    extra_args: Vec<String>,
}

impl Default for SswConfig {
    fn default() -> Self {
        Self {
            memory_in_gb: 1.0,
            restart_timeout: 12.0,
            shutdown_timeout: 5.0,
            ssw_port: 25565,
            mc_version: None,
            required_java_version: "17.0".to_string(),
            extra_args: Vec::new(),
        }
    }
}

pub struct MinecraftServer {
    state: Arc<Mutex<MCServerState>>,
    jar_path: PathBuf,
    exit_handler: Option<JoinHandle<()>>,
    server_stdin_sender: Option<Sender<String>>,
}

impl MinecraftServer {
    pub fn new(jar_path: PathBuf) -> Self {
        Self {
            jar_path,
            state: Arc::new(Mutex::new(MCServerState::Stopped)),
            exit_handler: None,
            server_stdin_sender: None,
        }
    }

    pub fn run(&mut self) -> io::Result<()> {
        // TODO: check java version
        info!("Checking Java version...");
        // TODO: load config and server.properties
        // TODO: patch Log4j
        let proc_args = vec![
            "java",
            "-Xms1G",
            "-Xmx1G",
            "-jar",
            self.jar_path
                .to_str()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid path"))?,
            "nogui",
        ];
        {
            *self.state.lock().unwrap() = MCServerState::Starting;
        }
        let mut child = tokio::process::Command::new(proc_args[0])
            .current_dir(self.jar_path.parent().map_or_else(
                || std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                Path::to_path_buf,
            ))
            .args(&proc_args[1..])
            .stderr(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped())
            .spawn()?;
        // pipe stdout and stderr to stdout
        let stdout = child.stdout.take().unwrap();
        let stdout_token = CancellationToken::new();
        let cloned_token = stdout_token.clone();
        let cloned_state = self.state.clone();
        let ready_line_regex = Regex::new(r#"^(\[.+\]:?)+ Done (\(\d+\.\d+s\))?!"#).unwrap();
        let stopping_server_line_regex = Regex::new(r#"^(\[.+\]:?)+ Stopping the server"#).unwrap();
        let stdout_handle =
            tokio::spawn(async move {
                if let Err(err) = async {
                let cancellation_token = cloned_token;
                let buf = &mut String::new();
                let mut reader = tokio::io::BufReader::new(stdout);
                loop {
                    select! {
                        n = reader.read_line(buf) => {
                            if n? == 0 {
                                break;
                            }
                            print!("{}", buf);
                            std::io::stdout().flush()?;
                            let mut current_state_lock = cloned_state.lock().unwrap();
                            match *current_state_lock {
                                MCServerState::Stopped => error!("Reading IO after server stopped"),
                                MCServerState::Starting => {
                                    if ready_line_regex.is_match(buf) {
                                        *current_state_lock = MCServerState::Running;
                                    }
                                },
                                MCServerState::Running => {
                                    if stopping_server_line_regex.is_match(buf) {
                                        *current_state_lock = MCServerState::Stopping;
                                    }
                                },
                                MCServerState::Stopping => {},
                            }
                            buf.clear();
                        }
                        _ = cancellation_token.cancelled() => {
                            break;
                        }
                    }
                }
                Ok::<(), io::Error>(())
            }.await {
                error!("Error reading from stdout: {}", err);
            }
            });
        let mut pipe_handles = vec![(stdout_token, stdout_handle)];

        let stderr = child.stderr.take().unwrap();
        let stderr_token = CancellationToken::new();
        let cloned_token = stderr_token.clone();
        let stderr_handle = tokio::spawn(async move {
            if let Err(err) = pipe_readable_to_stdout(stderr, cloned_token).await {
                error!("Error reading from stderr: {}", err);
            }
        });
        pipe_handles.push((stderr_token, stderr_handle));

        // pipe stdin to child stdin
        let proc_stdin = child.stdin.take().unwrap();
        let stdin_token = CancellationToken::new();
        let cloned_token = stdin_token.clone();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(3);
        self.server_stdin_sender = Some(tx);
        let stdin_handle = tokio::spawn(async move {
            // TODO: extract to a function, this looks confusing
            if let Err(err) = async {
                let mut proc_stdin = tokio::io::BufWriter::new(proc_stdin);
                loop {
                    select! {
                        msg = rx.recv() => {
                            if let Some(mut msg) = msg {
                                if !msg.ends_with('\n') {
                                    msg.push('\n');
                                }
                                proc_stdin.write_all(msg.as_bytes()).await?;
                                proc_stdin.flush().await?;
                            }
                        }

                        _ = cloned_token.cancelled() => {
                            break;
                        }
                    }
                }
                Ok::<(), io::Error>(())
            }
            .await
            {
                error!("Error reading from stdin: {}", err);
            }
        });
        pipe_handles.push((stdin_token, stdin_handle));

        let status_clone = self.state.clone();
        let exit_handler_handle = tokio::spawn(async move {
            match child.wait().await {
                Ok(status) => info!("Server exited with status {}", status),
                Err(err) => error!("Error waiting for child process: {}", err),
            }
            // wait for all pipes to finish after cancelling them
            for result in
                futures::future::join_all(pipe_handles.into_iter().map(|(token, handle)| {
                    token.cancel();
                    handle
                }))
                .await
            {
                if let Err(err) = result {
                    error!("Error waiting for pipe: {}", err);
                }
            }
            *status_clone.lock().unwrap() = MCServerState::Stopped;
        });
        self.exit_handler = Some(exit_handler_handle);

        Ok(())
    }

    pub async fn wait_for_exit(&mut self) -> io::Result<()> {
        if let Some(handle) = self.exit_handler.take() {
            handle.await?;
        }
        Ok(())
    }

    pub fn get_server_sender(&self) -> Option<Sender<String>> {
        self.server_stdin_sender.clone()
    }

    pub fn status(&self) -> Arc<Mutex<MCServerState>> {
        self.state.clone()
    }
}
