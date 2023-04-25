use std::{
    borrow::Cow,
    collections::HashMap,
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
    process,
    sync::{Arc, Mutex},
};

use chrono::Utc;
use lazy_static::lazy_static;
use log::{debug, error, info, warn};
use regex::Regex;
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt},
    process::{ChildStdin, ChildStdout},
    select,
    sync::{broadcast, mpsc},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use walkdir::WalkDir;
use zip::{write::FileOptions, CompressionMethod, ZipWriter};

use crate::{
    config::{convert_json_to_toml, SswConfig},
    minecraft::{manifest::VersionManifestV2, mc_version_data::MinecraftVersionData},
    ssw_error,
    util::{create_dir_if_not_exists, get_java_version, path_to_str},
};

use self::mc_version::try_read_version_from_jar;

pub mod log4j;
pub mod manifest;
pub mod mc_version;
pub mod mc_version_data;

/// Represents the state of a Minecraft server
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

/// The default port used by Minecraft servers
pub const DEFAULT_MC_PORT: u16 = 25565;

type MCServerProperties = HashMap<String, Value>;

/// Represents a Minecraft server
pub struct MinecraftServer<'a> {
    /// Thread-safe mutex lock on the server state as it needs to be accessed by multiple threads
    state: Arc<Mutex<MCServerState>>,
    /// A join handle to the exit handler task
    exit_handler: Option<JoinHandle<()>>,
    /// A sender used to send messages to the servers stdin
    server_stdin_sender: Option<mpsc::Sender<String>>,
    /// Broadcast channel pair for broadcasting server state changes
    server_status_broadcast_channels: (
        broadcast::Sender<MCServerState>,
        broadcast::Receiver<MCServerState>,
    ),
    /// Path to the Minecraft server jar
    jar_path: PathBuf,
    /// Deserialized server.properties file
    properties: Option<MCServerProperties>,
    /// The SSW server configuration
    pub ssw_config: Cow<'a, SswConfig<'a>>,
    pub show_output: bool,
}

impl<'a> MinecraftServer<'a> {
    /// Creates a new `MinecraftServer` struct.
    ///
    /// This will not start the server, only prepare it. This involves checking
    /// the jar file and loading the properties file if it exists.
    ///
    /// # Arguments
    ///
    /// * `jar_path` - The path to the server jar file
    pub async fn init(jar_path: PathBuf) -> MinecraftServer<'a> {
        let config_path = jar_path.with_file_name(".ssw").join("ssw.toml");
        let old_config_path = config_path.with_extension("json");
        let ssw_config = if old_config_path.exists() {
            info!("Found old SSW config, converting to new format");
            convert_json_to_toml::<SswConfig>(&old_config_path)
                .await
                .unwrap_or_else(|e| {
                    error!("Failed to convert SSW config, using default: {}", e);
                    SswConfig::default()
                })
        } else {
            SswConfig::from_path(&config_path)
                .await
                .unwrap_or_else(|e| {
                    error!("Failed to load SSW config, using default: {}", e);
                    SswConfig::default()
                })
        };
        let mut inst = Self {
            jar_path,
            state: Arc::new(Mutex::new(MCServerState::Stopped)),
            exit_handler: None,
            server_stdin_sender: None,
            server_status_broadcast_channels: broadcast::channel(1),
            ssw_config: Cow::Owned(ssw_config),
            properties: None,
            show_output: true,
        };
        inst.load_properties().await;
        inst
    }

    /// Stop the server if it is running
    ///
    /// # Errors
    ///
    /// An error may occur if sending the stop command to the server fails
    pub async fn stop(&self) -> Result<(), mpsc::error::SendError<String>> {
        self.send_command("stop".to_string()).await
    }

    /// Restarts the server
    /// This works by calling `stop`, `wait_for_exit`, then `start`
    ///
    /// # Errors
    ///
    /// An error may occur if sending the stop command to the server fails,
    /// if waiting for the server to exit fails, or if starting the server fails
    pub async fn restart(&mut self) -> ssw_error::Result<()> {
        self.stop().await.map_err(|e| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to send stop command: {}", e),
            )
        })?;
        self.wait_for_exit().await?;
        self.run().await?;
        Ok(())
    }

    /// Send a command to the server if it is running
    ///
    /// # Arguments
    ///
    /// * `command` - The command to send to the server
    ///
    /// # Errors
    ///
    /// An error may occur if sending the command to the server fails
    pub async fn send_command(
        &self,
        command: String,
    ) -> Result<(), mpsc::error::SendError<String>> {
        if let Some(ref sender) = self.server_stdin_sender {
            sender.send(command).await
        } else {
            let error_message = format!("Attempted to send command to stopped server: {}", command);
            Err(mpsc::error::SendError(error_message))
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
            debug!("Loaded server.properties");
        } else {
            warn!("Failed to load server.properties, using existing properties");
        }
    }

    /// Save the `server.properties` file for this server.
    ///
    /// # Errors
    ///
    /// An error may occur when writing the properties file.
    pub async fn save_properties(&self) -> io::Result<()> {
        let props_path = self.jar_path.with_file_name("server.properties");
        if self.properties.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Properties have not been loaded",
            ));
        }
        save_properties(&props_path, self.properties.as_ref().unwrap()).await
    }

    /// Saves this server's SSW config to the config file
    ///
    /// # Errors
    ///
    /// An error may occur when writing the config file or in the serialization process.
    pub async fn save_config(&self) -> ssw_error::Result<()> {
        let config_path = self.get_config_path();
        self.ssw_config.save(&config_path).await
    }

    /// Get a reference to a property from server.properties
    ///
    /// # Arguments
    ///
    /// * `key` - The key of the property to get
    pub fn get_property(&self, key: &str) -> Option<&Value> {
        self.properties.as_ref()?.get(key)
    }

    /// Set a server property if the properties have been loaded
    ///
    /// # Arguments
    ///
    /// * `key` - The key of the property to set
    /// * `value` - The value to set the property to
    pub fn set_property(&mut self, key: String, value: Value) {
        if let Some(ref mut props) = self.properties {
            props.insert(key, value);
        } else {
            warn!(
                "Attempting to set property '{}={}' before server.properties has been loaded",
                key, value
            );
        }
    }

    /// Loads the version of the Minecraft server and its required Java version.
    ///
    /// # Errors
    ///
    /// An error may occur if the server jar is not found or if reading the version manifest fails.
    ///
    /// # Panics
    ///
    /// This function will panic if the server jar somehow does not have a parent component.
    pub async fn load_version(&mut self) -> ssw_error::Result<()> {
        let mc_version_string = try_read_version_from_jar(
            self.jar_path()
                .parent()
                .expect("server jar is somehow the root directory"),
        )
        .unwrap_or_else(|e| {
            warn!("error occurred trying to read version from jar: {}", e);
            None
        });
        if let Some(mc_version_string) = mc_version_string {
            info!("Found Minecraft version in jar: {}", mc_version_string);
            let manifest = VersionManifestV2::load().await?;
            let mc_version = manifest.find_version(&mc_version_string).unwrap();
            let required_java_version = MinecraftVersionData::async_try_from(mc_version)
                .await
                .map_or_else(
                    |e| {
                        warn!("error occurred trying to get version data: {}", e);
                        17
                    },
                    |d| *d.java_version().major_version(),
                );
            let required_java_version = if required_java_version <= 8 {
                format!("1.{}", required_java_version)
            } else {
                format!("{}.0", required_java_version)
            };
            info!("Found required Java version: {}", required_java_version);
            let mut self_ssw_config = self.ssw_config.to_mut();
            self_ssw_config.mc_version = Some(mc_version_string);
            self_ssw_config.required_java_version = required_java_version;
            if let Err(e) = self.ssw_config.save(&self.get_config_path()).await {
                error!("failed to save SSW config: {:?}", e);
            }
        } else {
            warn!("Could not find Minecraft version in jar.");
            warn!("Please use the mc-version command to set the Minecraft version.");
        }
        Ok(())
    }

    /// Gets the server's default config path
    ///
    /// This will resolve to `{server_jar_path}/.ssw/ssw.json`
    pub fn get_config_path(&self) -> PathBuf {
        self.jar_path.with_file_name(".ssw").join("ssw.toml")
    }

    pub fn get_backup_folder(&self) -> PathBuf {
        self.jar_path.with_file_name(".ssw").join("backups")
    }

    /// Makes a backup of the server directory and saves it to the backup folder.
    /// This will not backup the `libraries`, `logs`, `versions`, or `.ssw` directories.
    /// The backup is a zip file.
    fn make_backup(&self) -> ssw_error::Result<()> {
        const EXCLUDE_GLOBS: &[&str] =
            &["**/.ssw/*", "**/logs/*", "**/libraries/*", "**/versions/*"];
        // get all files in the server directory
        let server_dir = self.jar_path.parent().unwrap();
        let files = WalkDir::new(server_dir)
            .into_iter()
            .filter_entry(|e| {
                !EXCLUDE_GLOBS.iter().any(|glob| {
                    let glob = glob::Pattern::new(glob).unwrap();
                    glob.matches_path(e.path())
                })
            })
            .filter_map(|e| {
                e.map_or_else(
                    |e| {
                        warn!("Failed to read entry: {}", e);
                        None
                    },
                    |e| Some(e.path().to_path_buf()),
                )
            })
            .collect::<Vec<_>>();

        let backup_folder = self.get_backup_folder();
        create_dir_if_not_exists(&backup_folder)?;
        // check if the backup folder has max backups
        let max_backups = self.ssw_config.max_backups;
        if max_backups > 0 {
            let mut backups = WalkDir::new(&backup_folder)
                .follow_links(true)
                .into_iter()
                .filter_map(|e| {
                    e.map_or_else(
                        |e| {
                            warn!("Failed to read entry: {}", e);
                            None
                        },
                        |e| Some(e.path().to_path_buf()),
                    )
                })
                .collect::<Vec<_>>();
            backups.sort_by(|a, b| {
                b.metadata()
                    .unwrap()
                    .modified()
                    .unwrap()
                    .cmp(&a.metadata().unwrap().modified().unwrap())
            });
            if backups.len() > max_backups {
                for backup in backups.iter().skip(max_backups) {
                    debug!("Removing old backup: {}", backup.display());
                    if let Err(e) = std::fs::remove_file(backup) {
                        warn!("Failed to remove old backup: {}", e);
                    }
                }
            }
        }

        let backup_name = format!("{}.zip", Utc::now().format("%Y-%m-%d_%H-%M-%S"));
        let backup_file = backup_folder.join(&backup_name);

        if let Err(e) = {
            let mut zip = ZipWriter::new(File::create(&backup_file)?);
            let options = FileOptions::default().compression_method(CompressionMethod::Stored);
            for file in files {
                let zip_path = file
                    .strip_prefix(server_dir)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                let file = server_dir.join(&file);
                if file.is_dir() {
                    zip.add_directory(zip_path, options)?;
                } else {
                    let mut file = File::open(file)?;
                    zip.start_file(zip_path, options)?;
                    io::copy(&mut file, &mut zip)?;
                }
            }
            zip.finish()?;
            Ok::<(), ssw_error::Error>(())
        } {
            error!("Failed to create backup: {}", e);
            // delete the backup file if it was created
            // this essentially undoes the failed backup
            if backup_file.exists() {
                std::fs::remove_file(&backup_file)?;
            }
        } else {
            info!("Created backup: {}", backup_name);
        }
        Ok(())
    }

    /// Run the Minecraft server process
    ///
    /// # Errors
    ///
    /// An error can occur when:
    /// - A Java executable cannot be found
    /// - The server jar cannot be found, read, or its path has an invalid format
    /// - Patching `Log4J` fails
    /// - The server process cannot be spawned
    pub async fn run(&mut self) -> ssw_error::Result<()> {
        let java_executable = self.get_java_executable().await?;
        self.check_java_version(&java_executable).await?;
        info!("Loading SSW config...");
        let config_path = self.get_config_path();
        let ssw_config = SswConfig::from_path(&config_path)
            .await
            .unwrap_or_else(|e| {
                error!("Failed to load SSW config, using default: {}", e);
                SswConfig::default()
            });
        self.ssw_config = Cow::Owned(ssw_config);
        info!("SSW config loaded: {:#?}", self.ssw_config);
        self.load_properties().await;
        if let Some(port) = self.get_property("server-port") {
            if !validate_port(self.ssw_config.ssw_port, port) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid Minecraft server port",
                )
                .into());
            }
        }
        self.patch_log4j().await?;
        if self.ssw_config.auto_backup {
            info!("Auto-backup enabled, backing up server...");
            if let Err(e) = self.make_backup() {
                error!("Failed to make backup, continuing: {}", e);
            } else {
                info!("Backup complete.");
            }
        }
        let memory_in_mb = self.ssw_config.memory_in_gb * 1024.0;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let memory_arg = format!("-Xmx{}M", memory_in_mb.abs() as u32);
        let proc_args = vec![
            path_to_str(&java_executable)?,
            "-Xms512M",
            memory_arg.as_str(),
            "-jar",
            path_to_str(&self.jar_path)?,
            "nogui",
        ];
        debug!("Running Minecraft server with args: {:?}", proc_args);
        debug!("Setting server state to Starting");
        {
            *self.state.lock().unwrap() = MCServerState::Starting;
        }
        if let Err(e) = self
            .server_status_broadcast_channels
            .0
            .send(MCServerState::Starting)
        {
            error!("Failed to send server status update: {:?}", e);
        }
        info!("Starting Minecraft server...");
        // use the jar path parent. otherwise, use the current directory. otherwise again, use "."
        let mut child = tokio::process::Command::new(proc_args[0])
            .current_dir(self.jar_path.parent().map_or_else(
                || std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                Path::to_path_buf,
            ))
            .args(&proc_args[1..])
            .stderr(process::Stdio::piped())
            .stdout(process::Stdio::piped())
            .stdin(process::Stdio::piped())
            .spawn()?;

        let pipe_handles = self.start_mc_output_pipes(&mut child);

        let status_clone = self.state.clone();
        let status_sender_clone = self.server_status_broadcast_channels.0.clone();
        let exit_handler_handle = tokio::spawn(exit_handler(
            child,
            pipe_handles,
            status_clone,
            status_sender_clone,
        ));
        self.exit_handler = Some(exit_handler_handle);

        Ok(())
    }

    fn start_mc_output_pipes(
        &mut self,
        child: &mut tokio::process::Child,
    ) -> Vec<(CancellationToken, JoinHandle<()>)> {
        // pipe stdout and stderr to stdout
        let stdout = child.stdout.take().unwrap();
        let stdout_token = CancellationToken::new();
        let cloned_token = stdout_token.clone();
        let cloned_state = self.state.clone();
        let cloned_state_sender = self.server_status_broadcast_channels.0.clone();
        let show_output = self.show_output;
        let stdout_handle = tokio::spawn(async move {
            if let Err(err) = pipe_and_monitor_stdout(
                stdout,
                cloned_token,
                cloned_state,
                cloned_state_sender,
                show_output,
            )
            .await
            {
                error!("Error reading from stdout: {}", err);
            }
        });
        let mut pipe_handles = vec![(stdout_token, stdout_handle)];
        if self.show_output {
            let stderr = child.stderr.take().unwrap();
            let stderr_token = CancellationToken::new();
            let cloned_token = stderr_token.clone();
            let stderr_handle = tokio::spawn(pipe_stderr(stderr, cloned_token));
            pipe_handles.push((stderr_token, stderr_handle));
        }
        // pipe stdin to child stdin
        let proc_stdin = child.stdin.take().unwrap();
        let stdin_token = CancellationToken::new();
        let cloned_token = stdin_token.clone();
        let (stdin_tx, stdin_rx) = tokio::sync::mpsc::channel::<String>(3);
        self.server_stdin_sender = Some(stdin_tx);
        let stdin_handle = tokio::spawn(async move {
            if let Err(err) = pipe_stdin(proc_stdin, stdin_rx, cloned_token).await {
                error!("Error reading from stdin: {}", err);
            }
        });
        pipe_handles.push((stdin_token, stdin_handle));
        pipe_handles
    }

    /// Checks that this servers configured Java version is valid
    ///
    /// # Arguments
    ///
    /// * `java_location` - The location of the Java executable
    ///
    /// # Errors
    ///
    /// An error can occur when loading the version from config or spawning the child process.
    async fn check_java_version(&mut self, java_location: &Path) -> ssw_error::Result<()> {
        info!("Checking Java version...");
        let java_version = get_java_version(java_location).await?;
        info!("Found Java version: {}", java_version);
        // compare version number in the order of major, minor, patch
        // unwrap is safe here because the regex will always match (unless Java changes their version format, which they haven't in a long time)
        for (ver, req) in java_version
            .split('.')
            .zip(self.ssw_config.required_java_version.split('.'))
            .map(|(a, b)| (a.parse::<u32>().unwrap(), b.parse::<u32>().unwrap()))
        {
            if ver == req {
                // skip to more specific version
                continue;
            }
            if ver > req {
                // version is good
                break;
            }
            // version is too low
            return Err(ssw_error::Error::BadJavaVersion(
                self.ssw_config.required_java_version.clone(),
                java_version,
            ));
        }
        Ok(())
    }

    /// Loads the Java executable path from the config. If the config is not set, it will try to
    /// find the Java executable in the PATH environment variable and then store it in the config
    /// for future use.
    ///
    /// # Errors
    ///
    /// An error can occur when loading the path from config or checking `PATH`.
    async fn get_java_executable(&mut self) -> io::Result<PathBuf> {
        let java_exec_store = self.get_config_path().with_file_name("java_executable");
        let java_location = if java_exec_store.exists() {
            // the java executable is stored in the config directory
            tokio::fs::read_to_string(java_exec_store)
                .await
                .map(|s| s.trim().to_string())
                .map(PathBuf::from)?
        } else {
            // try to find java in PATH
            let java_location = which::which("java").map_err(|e| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("Failed to find java executable in PATH: {}", e),
                )
            })?;
            // store the java executable path in the config directory for later use
            tokio::fs::write(java_exec_store, java_location.to_string_lossy().as_bytes()).await?;
            java_location
        };
        Ok(java_location)
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

    /// Returns a new broadcast receiver for server status updates.
    pub fn subscribe_to_state_changes(&self) -> broadcast::Receiver<MCServerState> {
        self.server_status_broadcast_channels.0.subscribe()
    }

    /// Gets a clone of the `Arc<Mutex<MCServerState>>` for the server's state.
    pub fn status(&self) -> Arc<Mutex<MCServerState>> {
        self.state.clone()
    }

    /// Gets a reference to the servers JAR path.
    pub fn jar_path(&self) -> &Path {
        self.jar_path.as_path()
    }
}

/// Pipes the stderr of the child process to the stdout
/// of the current process.
///
/// # Arguments
///
/// * `stderr` - The stderr of the child process
/// * `cancel_token` - The cancellation token for the pipe
async fn pipe_stderr(mut stderr: tokio::process::ChildStderr, cancel_token: CancellationToken) {
    let mut stdout = tokio::io::stdout();
    select! {
        n = tokio::io::copy(&mut stderr, &mut stdout) => {
            if let Err(err) = n {
                error!("Error reading from stderr: {}", err);
            } else {
                debug!("Finished reading from stderr");
            }
        }
        _ = cancel_token.cancelled() => {
            debug!("stderr pipe cancelled");
        }
    }
}

/// Validate that the Minecraft server port is a valid `u16` and the SSW port is not the same as the Minecraft server port.
///
/// # Arguments:
///
/// * `ssw_port` - The SSW port.
/// * `server_port_value` - The Minecraft server port.
///
/// returns: `bool`
fn validate_port(ssw_port: u16, server_port_value: &Value) -> bool {
    let mut port_is_valid = true;
    let server_port: u16 = server_port_value.as_u64().map_or_else(
        || {
            debug!("server.properties server-port does not exist");
            DEFAULT_MC_PORT
        },
        |v| {
            v.try_into().unwrap_or_else(|_| {
                warn!("Invalid Minecraft server port: {}", server_port_value);
                warn!("Valid ports are in the range 0-65535");
                if ssw_port == DEFAULT_MC_PORT {
                    error!("The SSW port ({}) is the same as the default Minecraft port", ssw_port);
                    error!("Because there was an error with the server port, there will be a feedback loop if anyone tries to connect");
                    error!("Please change the server port in server.properties to fix this");
                }
                port_is_valid = false;
                DEFAULT_MC_PORT
            })
        },
    );
    if ssw_port == server_port {
        error!(
            "The SSW port ({}) is the same as the Minecraft server port ({})",
            ssw_port, server_port
        );
        error!("Change one of the ports to fix this");
        port_is_valid = false;
    }
    port_is_valid
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
async fn load_properties(path: &Path) -> ssw_error::Result<MCServerProperties> {
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
                .map(|s| s.split('#').next().unwrap_or(""))
                .map_or_else(|| Ok(Value::Null), serde_json::from_str)
                .unwrap_or_default();
            Some((key.to_string(), value))
        })
        .collect::<MCServerProperties>();
    Ok(new_props)
}

/// Save the properties to the given path.
/// Properties files are expected to be in the format `key=value`.
/// Any value corresponding to `null` will be saved as an empty string.
///
/// # Arguments:
///
/// * `path` - The path to the properties file.
/// * `properties` - The properties to save.
///
/// # Errors:
///
/// An error is returned if the file could not be written.
async fn save_properties(path: &Path, properties: &MCServerProperties) -> io::Result<()> {
    let props_string = properties
        .iter()
        .map(|(k, v)| {
            let vs = if v.is_null() {
                String::new()
            } else {
                v.to_string()
            };
            format!("{}={}", k, vs)
        })
        .collect::<Vec<String>>()
        .join("\n");
    tokio::fs::write(path, props_string).await?;
    Ok(())
}

/// Waits for the server process to exit, cancels all pipes, and set the server state to `Stopped`.
/// This function should be spawned as a task.
///
/// # Arguments
///
/// * `server_child_proc` - The child process to wait for
/// * `pipe_handles` - A vector of tuples containing a `CancellationToken` and a `JoinHandle` for each pipe
/// * `status` - The `Arc<Mutex<MCServerState>>` to set to `Stopped` when the server exits
/// * `status_tx` - The `Sender` for the server status broadcast channel
async fn exit_handler(
    mut server_child_proc: tokio::process::Child,
    pipe_handles: Vec<(CancellationToken, JoinHandle<()>)>,
    status: Arc<Mutex<MCServerState>>,
    status_tx: broadcast::Sender<MCServerState>,
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
    // this one goes to INFO so that the user knows the server has stopped
    // without having to enable debug logging
    info!("Setting server state to Stopped");
    *status.lock().unwrap() = MCServerState::Stopped;
    if let Err(err) = status_tx.send(MCServerState::Stopped) {
        error!("Error sending server state: {}", err);
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
/// * `server_state_tx` - The `Sender` for the server status broadcast channel
///
/// # Errors
///
/// An error will be returned if one occurs flushing stdout or reading from the `ChildStdout`.
async fn pipe_and_monitor_stdout(
    stdout: ChildStdout,
    cancellation_token: CancellationToken,
    server_state: Arc<Mutex<MCServerState>>,
    server_state_tx: broadcast::Sender<MCServerState>,
    show_output: bool,
) -> io::Result<()> {
    lazy_static! {
        static ref READY_REGEX: Regex =
            Regex::new(r#"^(\[.+]:?)+ Done (\(\d+\.\d+s\))?!"#).unwrap();
        static ref STOPPING_REGEX: Regex =
            Regex::new(r#"^(\[.+]:?)+ Stopping the server"#).unwrap();
    }
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
                if show_output {
                    print!("{}", buf);
                    std::io::stdout().flush()?;
                }
                let mut current_state_lock = server_state.lock().unwrap();
                match *current_state_lock {
                    MCServerState::Stopped => error!("Reading IO after server stopped"),
                    MCServerState::Starting => {
                        if READY_REGEX.is_match(buf) {
                            debug!("Setting server state to Running");
                            *current_state_lock = MCServerState::Running;
                            if let Err(err) = server_state_tx.send(MCServerState::Running) {
                                error!("Error sending server state: {}", err);
                            }
                        }
                    },
                    MCServerState::Running => {
                        if STOPPING_REGEX.is_match(buf) {
                            debug!("Setting server state to Stopping");
                            *current_state_lock = MCServerState::Stopping;
                            if let Err(err) = server_state_tx.send(MCServerState::Stopping) {
                                error!("Error sending server state: {}", err);
                            }
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
    mut rx: mpsc::Receiver<String>,
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
