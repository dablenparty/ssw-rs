use std::path::PathBuf;

use log::{error, info, LevelFilter};
use minecraft::{begin_server_task, ServerTaskRequest};
use tokio::{io::AsyncBufReadExt, sync::mpsc::Sender, task::JoinHandle};

use crate::logging::init_logging;

mod logging;
mod minecraft;

#[tokio::main]
async fn main() {
    if let Err(e) = init_logging(LevelFilter::Debug) {
        eprintln!("Failed to initialize logging: {}", e);
        eprintln!("Debug info: {:?}", e);
        std::process::exit(1);
    }
    // TODO: clap args
    let jar_path = PathBuf::from("mc-server/server.jar");
    let (server_handle, server_sender) = begin_server_task(jar_path);
    let stdin_handle = begin_stdin_task(server_sender);
    let handles = vec![server_handle, stdin_handle];
    for handle in handles {
        if let Err(e) = handle.await {
            error!("Error waiting on child task: {e}");
        }
    }
}

fn begin_stdin_task(server_sender: Sender<ServerTaskRequest>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let stdin = tokio::io::stdin();
        let mut stdin_reader = tokio::io::BufReader::new(stdin).lines();
        loop {
            let line = match stdin_reader.next_line().await.unwrap() {
                Some(l) => l,
                None => {
                    error!("Error reading from stdin, most likely closed");
                    break;
                }
            };

            match line.as_str() {
                "start" => {
                    if let Err(e) = server_sender.send(ServerTaskRequest::Start).await {
                        error!("Error sending start request: {e}");
                    }
                }
                _ => {
                    if let Err(e) = server_sender.send(ServerTaskRequest::Command(line)).await {
                        error!("Error sending command to server: {e}");
                    }
                }
            }
        }
        info!("Finished reading from stdin");
    })
}
