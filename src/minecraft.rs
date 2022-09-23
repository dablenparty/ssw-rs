use std::{
    collections::HashMap,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use log::{debug, error, info, warn};
use regex::Regex;
use serde::{de::Error, Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt},
    process::{ChildStdin, ChildStdout},
    select,
    sync::mpsc::{error::SendError, Receiver, Sender},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::util::async_create_dir_if_not_exists;

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
    pub ssw_port: u16,
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
            ssw_port: 25566,
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
            info!("Loading existing SSW config");
            let config_string = tokio::fs::read_to_string(config_path)
                .await
                .map_err(serde_json::Error::custom)?;
            // TODO: find a way to merge two bad configs
            //? proc macro for struct -> HashMap
            serde_json::from_str(&config_string)
        } else {
            info!(
                "No SSW config found, creating default config at {}",
                config_path.display()
            );
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

pub const DEFAULT_MC_PORT: u16 = 25565;
type MCServerProperties = HashMap<String, Value>;

pub struct MinecraftServer {
    state: Arc<Mutex<MCServerState>>,
    exit_handler: Option<JoinHandle<()>>,
    server_stdin_sender: Option<Sender<String>>,
    jar_path: PathBuf,
    properties: Option<MCServerProperties>,
    pub ssw_config: SswConfig,
}

impl MinecraftServer {
    /// Creates a new `MinecraftServer` struct.
    ///
    /// This will not start the server, only prepare it. This involves checking
    /// the jar file and loading the properties file if it exists.
    ///
    /// # Arguments
    ///
    /// * `jar_path` - The path to the server jar file
    pub async fn new(jar_path: PathBuf) -> Self {
        let config_path = jar_path.with_file_name(".ssw").join("ssw.json");
        let mut inst = Self {
            jar_path,
            state: Arc::new(Mutex::new(MCServerState::Stopped)),
            exit_handler: None,
            server_stdin_sender: None,
            ssw_config: SswConfig::new(&config_path).await.unwrap_or_default(),
            properties: None,
        };
        inst.load_properties().await;
        inst
    }

    /// Stop the server if it is running
    pub async fn stop(&self) -> Result<(), SendError<String>> {
        self.send_command("stop".to_string()).await
    }

    /// Send a command to the server if it is running
    ///
    /// # Arguments
    ///
    /// * `command` - The command to send to the server
    pub async fn send_command(&self, command: String) -> Result<(), SendError<String>> {
        if let Some(ref sender) = self.server_stdin_sender {
            sender.send(command).await
        } else {
            Ok(())
        }
    }

    /// Load the `server.properties` file for this server.
    ///
    /// If the properties have been previously loaded and fail to load again, the previous properties will be kept.
    pub async fn load_properties(&mut self) {
        // load the properties file if it exists or assign None
        let new_props = load_properties(&self.jar_path.with_file_name("server.properties"))
            .await
            .map_or_else(
                |e| {
                    warn!("Failed to load server.properties: {}", e);
                    None
                },
                Some,
            );
        if new_props.is_some() || self.properties.is_none() {
            self.properties = new_props;
        } else {
            warn!("Failed to load server.properties, using existing properties");
        }
    }

    /// Get a reference to a property from server.properties
    ///
    /// # Arguments
    ///
    /// * `key` - The key of the property to get
    pub fn get_property(&self, key: &str) -> Option<&Value> {
        self.properties.as_ref()?.get(key)
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
            error!("Using default SSW config");
            SswConfig::default()
        });
        info!("SSW config loaded: {:?}", self.ssw_config);
        self.load_properties().await;
        debug!("Validating ports");
        if let Some(port) = self.get_property("server-port") {
            let mut return_err = false;
            let _: u16 = port.as_u64().map_or_else(
                || {
                    debug!("server.properties server-port does not exist");
                    DEFAULT_MC_PORT
                },
                |v| {
                    v.try_into().unwrap_or_else(|_| {
                        warn!("Invalid Minecraft server port: {}", port);
                        warn!("Valid ports are in the range 0-65535");
                        if self.ssw_config.ssw_port == DEFAULT_MC_PORT {
                            error!("The SSW port is also the default Minecraft port");
                            error!("Because there was an error with the server port, there will be a feedback loop if anyone tries to connect");
                            error!("Please change the server port in server.properties to fix this");
                        }
                        return_err = true;
                        0
                    })
                },
            );
            if return_err {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid Minecraft server port",
                ));
            }
        }
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
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid unicode found in JAR path",
                )
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

        let mut stderr = child.stderr.take().unwrap();
        let stderr_token = CancellationToken::new();
        let cloned_token = stderr_token.clone();
        let stderr_handle = tokio::spawn(async move {
            let mut stdout = tokio::io::stdout();
            select! {
                n = tokio::io::copy(&mut stderr, &mut stdout) => {
                    if let Err(err) = n {
                        error!("Error reading from stderr: {}", err);
                    } else {
                        debug!("Finished reading from stderr");
                    }
                }
                _ = cloned_token.cancelled() => {
                    debug!("stderr pipe cancelled");
                }
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
            exit_handler(child, pipe_handles, status_clone).await;
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

    /// Gets a clone of the `Arc<Mutex<MCServerState>>` for the server's state.
    pub fn status(&self) -> Arc<Mutex<MCServerState>> {
        self.state.clone()
    }
}

/// Load the properties file from the given path.
///
/// Properties files are expected to be in the format `key=value`
/// with comments starting with `#`.
///
/// # Arguments:
///
/// * `path` - The path to the properties file.
///
/// # Errors:
///
/// An error is returned if the file could not be read. One is _not_ returned if the file is not
/// in the incorrect format. Instead, the properties are returned with only the properties that
/// were successfully parsed.
async fn load_properties(path: &Path) -> io::Result<MCServerProperties> {
    let properties_string = tokio::fs::read_to_string(path).await?;
    let new_props = properties_string
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let mut split = line.splitn(2, '=');
            let key = split.next()?;
            let value = split
                .next()
                .map_or_else(|| Ok(Value::Null), serde_json::from_str)
                .unwrap_or(Value::Null);
            Some((key.to_string(), value))
        })
        .collect::<MCServerProperties>();
    Ok(new_props)
}

/// Waits for the server process to exit, cancels all pipes, and set the server state to `Stopped`.
/// This function should be spawned as a task.
///
/// # Arguments
///
/// * `server_child_proc` - The child process to wait for
/// * `pipe_handles` - A vector of tuples containing a `CancellationToken` and a `JoinHandle` for each pipe
/// * `status` - The `Arc<Mutex<MCServerState>>` to set to `Stopped` when the server exits
async fn exit_handler(
    mut server_child_proc: tokio::process::Child,
    pipe_handles: Vec<(CancellationToken, JoinHandle<()>)>,
    status_clone: Arc<Mutex<MCServerState>>,
) {
    match server_child_proc.wait().await {
        Ok(status) => debug!("Server exited with status {}", status),
        Err(err) => error!("Error waiting for child process: {}", err),
    }
    // wait for all pipes to finish after cancelling them
    for result in futures::future::join_all(pipe_handles.into_iter().map(|(token, handle)| {
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
    let ready_line_regex = Regex::new(r#"^(\[.+]:?)+ Done (\(\d+\.\d+s\))?!"#).unwrap();
    let stopping_server_line_regex = Regex::new(r#"^(\[.+]:?)+ Stopping the server"#).unwrap();
    // doing this manually is slower than using tokio::io::copy, but it allows us to monitor the
    // output and update the server state
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
                debug!("stdout pipe cancelled");
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
