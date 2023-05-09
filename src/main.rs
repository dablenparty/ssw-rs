#![warn(clippy::all, clippy::pedantic)]
// the same thing has different names on windows and unix for some reason
#![allow(clippy::module_name_repetitions)]

use std::path::PathBuf;

use log::{debug, error, info, warn, LevelFilter};
use minecraft::{begin_server_task, ServerTaskRequest};
use tokio::{io::AsyncBufReadExt, select, sync::mpsc::Sender, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::logging::init_logging;

mod config;
mod logging;
mod minecraft;

#[tokio::main]
async fn main() {
    if let Err(e) = init_logging(LevelFilter::Debug) {
        eprintln!("Failed to initialize logging: {e}");
        eprintln!("Debug info: {e:?}");
        std::process::exit(1);
    }
    // TODO: clap args
    let jar_path = PathBuf::from("mc-server/server.jar");
    let server_token = CancellationToken::new();
    let (running_tx, running_rx) = tokio::sync::mpsc::channel(1);
    let (server_handle, server_sender) =
        begin_server_task(jar_path, running_tx, server_token.clone());
    let stdin_token = CancellationToken::new();
    let stdin_handle = begin_stdin_task(server_sender, running_rx, stdin_token);
    let ssw_version = env!("CARGO_PKG_VERSION");
    println!("SSW Console v{ssw_version}");
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
    mut running_rx: tokio::sync::mpsc::Receiver<bool>,
    token: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let stdin = tokio::io::stdin();
        let mut stdin_reader = tokio::io::BufReader::new(stdin).lines();
        loop {
            let line = select! {
                r = stdin_reader.next_line() => {
                    if let Some(line) = r.unwrap_or_else(|e| {
                        error!("Error reading from stdin: {e}");
                        None
                    }) { line }
                        else {
                            warn!("Stdin closed or errored");
                            break;
                        }

                }
                _ = token.cancelled() => {
                    debug!("Stdin task cancelled");
                    break;
                }
            };

            match line.as_str() {
                "start" => {
                    if let Err(e) = server_sender.send(ServerTaskRequest::Start).await {
                        error!("Error sending start request: {e}");
                    }
                }
                "stop" => {
                    if let Err(e) = server_sender.send(ServerTaskRequest::Stop).await {
                        error!("Error sending stop request: {e}");
                    }
                }
                "exit" => {
                    // kill server
                    if let Err(e) = server_sender.send(ServerTaskRequest::Kill).await {
                        error!("Error sending kill request: {e}");
                    }
                    info!("Exiting, expect some errors as the server is killed");
                    break;
                }
                _ => {
                    server_sender
                        .send(ServerTaskRequest::IsRunning)
                        .await
                        .unwrap_or_else(|e| {
                            error!("Error sending is_running request: {e}");
                        });
                    if !running_rx.recv().await.unwrap_or(false) {
                        warn!("Unknown command: {line}");
                        continue;
                    }
                    if let Err(e) = server_sender.send(ServerTaskRequest::Command(line)).await {
                        error!("Error sending command to server: {e}");
                    }
                }
            }
        }
        info!("Finished reading from stdin");
    })
}
