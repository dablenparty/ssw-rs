use std::{
    io,
    path::{Path, PathBuf},
};

use tokio::process::Child;

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
}

impl MinecraftServer {
    pub fn new(jar_path: PathBuf) -> Self {
        Self {
            jar_path,
            state: MCServerState::Stopped,
        }
    }

    pub fn run(&mut self) -> io::Result<Child> {
        // TODO: check java version
        println!("Checking Java version...");
        // TODO: load config and server.properties
        // TODO: patch Log4j
        // TODO: use ok_or to propagate error
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
        let child = tokio::process::Command::new(proc_args[0])
            .current_dir(self.jar_path.parent().map_or_else(
                || std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                Path::to_path_buf,
            ))
            .args(&proc_args[1..])
            .spawn()?;
        //? TODO: pipe stdout and stderr to terminal
        Ok(child)
    }
}
