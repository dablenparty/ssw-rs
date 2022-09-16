use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use log::{debug, error, info};
use regex::Regex;
use serde::{de::Error, Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt},
    process::{ChildStdin, ChildStdout},
    select,
    sync::mpsc::{Receiver, Sender},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::util::{async_create_dir_if_not_exists, pipe_readable_to_stdout};

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

#[derive(Serialize, Deserialize, Debug)]
pub struct SswConfig {
    pub memory_in_gb: f32,
    pub restart_timeout: f32,
    pub shutdown_timeout: f32,
    pub ssw_port: u32,
    pub mc_version: Option<String>,
    pub required_java_version: String,
    pub extra_args: Vec<String>,
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

impl SswConfig {
    /// Attempt to load a config from the given path
    ///
    /// # Arguments
    ///
    /// * `config_path` - The path to the config file. If it does not exist, it will be created with default values.
    ///
    /// # Errors
    ///
    /// An error may occur when reading or writing the config file, as well as in the serialization/deserialization process.
    ///
    /// returns: `serde_json::Result<SswConfig>`
    ///
    pub async fn new(config_path: &Path) -> serde_json::Result<Self> {
        if config_path.exists() {
            let config_string = tokio::fs::read_to_string(config_path)
                .await
                .map_err(serde_json::Error::custom)?;
            // TODO: find a way to merge two bad configs
            //? proc macro for struct -> HashMap
            serde_json::from_str(&config_string)
        } else {
            let config = Self::default();
            async_create_dir_if_not_exists(
                &config_path
                    .parent()
                    .map_or_else(|| PathBuf::from("."), Path::to_path_buf),
            )
            .await
            .map_err(serde_json::Error::custom)?;
            let config_string = serde_json::to_string_pretty(&config)?;
            tokio::fs::write(config_path, config_string)
                .await
                .map_err(serde_json::Error::custom)?;
            Ok(config)
        }
    }
}

pub struct MinecraftServer {
    state: Arc<Mutex<MCServerState>>,
    exit_handler: Option<JoinHandle<()>>,
    server_stdin_sender: Option<Sender<String>>,
    jar_path: PathBuf,
    pub ssw_config: SswConfig,
}

impl MinecraftServer {
    pub fn new(jar_path: PathBuf) -> Self {
        Self {
            jar_path,
            state: Arc::new(Mutex::new(MCServerState::Stopped)),
            exit_handler: None,
            server_stdin_sender: None,
            ssw_config: SswConfig::default(),
        }
    }

    /// Run the Minecraft server process
    ///
    /// # Errors
    ///
    /// An error can occur when loading the config or when spawning the child process.
    pub async fn run(&mut self) -> io::Result<()> {
        // TODO: check java version
        info!("Checking Java version...");
        info!("Loading SSW config...");
        let config_path = self
            .jar_path
            .parent()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::Other,
                    "Could not get parent directory of jar file",
                )
            })?
            .join(".ssw")
            .join("ssw.json");
        self.ssw_config = SswConfig::new(&config_path).await.unwrap_or_else(|e| {
            error!("Failed to load SSW config: {}", e);
            SswConfig::default()
        });
        info!("SSW config loaded: {:?}", self.ssw_config);
        // TODO: load server.properties
        // TODO: patch Log4j
        let memory_in_mb = self.ssw_config.memory_in_gb * 1024.0;
        // truncation is intentional
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let memory_arg = format!("-Xmx{}M", memory_in_mb.abs() as u32);
        let proc_args = vec![
            "java",
            "-Xms512M",
            memory_arg.as_str(),
            "-jar",
            self.jar_path.to_str().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "Invalid unicode found in path")
            })?,
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
        let stdout_handle = tokio::spawn(async move {
            if let Err(err) = pipe_and_monitor_stdout(stdout, cloned_token, cloned_state).await {
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
        let (tx, rx) = tokio::sync::mpsc::channel::<String>(3);
        self.server_stdin_sender = Some(tx);
        let stdin_handle = tokio::spawn(async move {
            if let Err(err) = pipe_stdin(proc_stdin, rx, cloned_token).await {
                error!("Error reading from stdin: {}", err);
            }
        });
        pipe_handles.push((stdin_token, stdin_handle));

        let status_clone = self.state.clone();
        let exit_handler_handle = tokio::spawn(async move {
            match child.wait().await {
                Ok(status) => debug!("Server exited with status {}", status),
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

    /// Waits for the server to exit.
    ///
    /// This works by taking ownership of the internal exit handler handle and awaiting it.
    ///
    /// # Errors:
    ///
    /// An error is returned if one occurs waiting for the exit handler.
    pub async fn wait_for_exit(&mut self) -> io::Result<()> {
        if let Some(handle) = self.exit_handler.take() {
            handle.await?;
        }
        Ok(())
    }

    /// Gets a clone of the tokio mpsc channel sender for the server's stdin.
    pub fn get_server_sender(&self) -> Option<Sender<String>> {
        self.server_stdin_sender.clone()
    }

    /// Gets a clone of the `Arc<Mutex<MCServerState>>` for the server's state.
    pub fn status(&self) -> Arc<Mutex<MCServerState>> {
        self.state.clone()
    }
}

/// Pipes the given `ChildStdout` to this process's stdout and monitors the server state.
///
/// This is done by matching every line against various regexes to determine when the server is
/// ready vs stopping.
///
/// # Arguments
///
/// * `stdout` - The `ChildStdout` to pipe to this process's stdout.
/// * `cancellation_token` - The `CancellationToken` to use to cancel the pipe.
/// * `server_state` - The mutex lock used to update the server state.
///
/// # Errors
///
/// An error will be returned if one occurs flushing stdout or reading from the `ChildStdout`.
async fn pipe_and_monitor_stdout(
    stdout: ChildStdout,
    cancellation_token: CancellationToken,
    server_state: Arc<Mutex<MCServerState>>,
) -> io::Result<()> {
    let ready_line_regex = Regex::new(r#"^(\[.+\]:?)+ Done (\(\d+\.\d+s\))?!"#).unwrap();
    let stopping_server_line_regex = Regex::new(r#"^(\[.+\]:?)+ Stopping the server"#).unwrap();
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
                let mut current_state_lock = server_state.lock().unwrap();
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
    Ok(())
}

/// Pipes messages received by the given `rx` to the given `ChildStdin`.
///
/// # Arguments
///
/// * `stdin` - The `ChildStdin` to pipe to.
/// * `rx` - The `Receiver` to receive messages from.
/// * `cancellation_token` - The `CancellationToken` to use to cancel the pipe.
///
/// # Errors
///
/// An error will be returned if one occurs writing to the `ChildStdin`.
async fn pipe_stdin(
    stdin: ChildStdin,
    mut rx: Receiver<String>,
    cancellation_token: CancellationToken,
) -> io::Result<()> {
    let mut stdin_writer = tokio::io::BufWriter::new(stdin);
    loop {
        select! {
            msg = rx.recv() => {
                if let Some(mut msg) = msg {
                    if !msg.ends_with('\n') {
                        msg.push('\n');
                    }
                    stdin_writer.write_all(msg.as_bytes()).await?;
                    stdin_writer.flush().await?;
                }
            }

            _ = cancellation_token.cancelled() => {
                break;
            }
        }
    }
    Ok(())
}
