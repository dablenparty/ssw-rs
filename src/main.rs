use std::path::PathBuf;

use log::{error, info, warn, LevelFilter};
use minecraft::{begin_server_task, ServerTaskRequest};
use tokio::{io::AsyncBufReadExt, select, sync::mpsc::Sender, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::logging::init_logging;

mod logging;
mod minecraft;
mod config;

#[tokio::main]
async fn main() {
    if let Err(e) = init_logging(LevelFilter::Debug) {
        eprintln!("Failed to initialize logging: {}", e);
        eprintln!("Debug info: {:?}", e);
        std::process::exit(1);
    }
    // TODO: clap args
    let jar_path = PathBuf::from("mc-server/server.jar");
    let server_token = CancellationToken::new();
    let (server_handle, server_sender) = begin_server_task(jar_path, server_token.clone());
    let stdin_token = CancellationToken::new();
    let stdin_handle = begin_stdin_task(server_sender, stdin_token);
    stdin_handle.await.unwrap_or_else(|e| {
        error!("Error waiting on stdin task: {e}");
    });
    let handles = vec![(server_handle, server_token)];
    for (handle, token) in handles {
        token.cancel();
        if let Err(e) = handle.await {
            error!("Error waiting on child task: {e}");
        }
    }
}

fn begin_stdin_task(
    server_sender: Sender<ServerTaskRequest>,
    token: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let stdin = tokio::io::stdin();
        let mut stdin_reader = tokio::io::BufReader::new(stdin).lines();
        loop {
            let line = select! {
                r = stdin_reader.next_line() => {
                    match r.unwrap_or_else(|e| {
                        error!("Error reading from stdin: {e}");
                        None
                    }) {
                        Some(line) => line,
                        None => {
                            warn!("Stdin closed or errored");
                            break;
                        }
                    }
                }
                _ = token.cancelled() => {
                    info!("Stdin task cancelled");
                    break;
                }
            };

            match line.as_str() {
                "start" => {
                    if let Err(e) = server_sender.send(ServerTaskRequest::Start).await {
                        error!("Error sending start request: {e}");
                    }
                }
                "exit" => {
                    // kill server
                    if let Err(e) = server_sender.send(ServerTaskRequest::Kill).await {
                        error!("Error sending kill request: {e}");
                    }
                    info!("Exiting");
                    token.cancel();
                    break;
                }
                _ => {
                    // TODO: check status and only send command if server is running
                    if let Err(e) = server_sender.send(ServerTaskRequest::Command(line)).await {
                        error!("Error sending command to server: {e}");
                    }
                }
            }
        }
        info!("Finished reading from stdin");
    })
}
