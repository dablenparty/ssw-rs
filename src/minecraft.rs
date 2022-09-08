use std::{
    io,
    path::{Path, PathBuf},
};

use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt},
    process::Child,
    select,
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

pub struct MinecraftServer {
    state: MCServerState,
    jar_path: PathBuf,
    pipe_handles: Vec<(CancellationToken, JoinHandle<()>)>,
}

impl MinecraftServer {
    pub fn new(jar_path: PathBuf) -> Self {
        Self {
            jar_path,
            state: MCServerState::Stopped,
            pipe_handles: Vec::new(),
        }
    }

    pub fn run(&mut self) -> io::Result<Child> {
        // TODO: check java version
        println!("Checking Java version...");
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
        // TODO: set this to starting, use ready event to set to running
        self.state = MCServerState::Running;
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
        let stdout_handle = tokio::spawn(async move {
            if let Err(err) = pipe_readable_to_stdout(stdout, cloned_token).await {
                eprintln!("Error reading from stdout: {}", err);
            }
        });
        self.pipe_handles.push((stdout_token, stdout_handle));

        let stderr = child.stderr.take().unwrap();
        let stderr_token = CancellationToken::new();
        let cloned_token = stderr_token.clone();
        let stderr_handle = tokio::spawn(async move {
            if let Err(err) = pipe_readable_to_stdout(stderr, cloned_token).await {
                eprintln!("Error reading from stderr: {}", err);
            }
        });
        self.pipe_handles.push((stderr_token, stderr_handle));

        // pipe stdin to child stdin
        let proc_stdin = child.stdin.take().unwrap();
        let stdin_token = CancellationToken::new();
        let cloned_token = stdin_token.clone();
        let stdin_handle = tokio::spawn(async move {
            // TODO: extract to a function, this looks confusing
            if let Err(err) = async {
                let buf = &mut String::new();
                let mut stdin = tokio::io::BufReader::new(tokio::io::stdin());
                let mut proc_stdin = tokio::io::BufWriter::new(proc_stdin);
                loop {
                    select! {
                        n = stdin.read_line(buf) => {
                            if n? == 0 || buf.trim() == "stop" {
                                break;
                            }
                            proc_stdin.write_all(buf.as_bytes()).await?;
                            proc_stdin.flush().await?;
                            buf.clear();
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
                eprintln!("Error reading from stdin: {}", err);
            }
        });
        self.pipe_handles.push((stdin_token, stdin_handle));

        Ok(child)
    }
}
