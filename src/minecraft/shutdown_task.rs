use std::time::Duration;

use duration_string::DurationString;
use log::{debug, info, warn};
use tokio::{select, sync::mpsc::Sender, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use super::{ping_task::begin_ping_task, ServerTaskRequest};

pub fn begin_shutdown_task(
    wait_for: Duration,
    server_address: String,
    server_sender: Sender<ServerTaskRequest>,
    token: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let duration_string = DurationString::from(wait_for);
        let (pinger, mut player_count_watcher) =
            begin_ping_task(server_address, token.child_token());
        let mut shutdown_handle: Option<JoinHandle<()>> = None;
        let mut shutdown_token = token.child_token();
        loop {
            // Now wait for the ping to complete or cancellation again
            let player_count = select! {
                r = player_count_watcher.changed() => {
                    match r {
                        Ok(_) => {
                            *player_count_watcher.borrow_and_update()
                        }
                        Err(e) => {
                            warn!("Failed to watch player count: {e}");
                            break;
                        }
                    }
                }
                _ = token.cancelled() => {
                    // since the pinger uses a child token, if this one was cancelled, the pinger will be too
                    pinger.await.unwrap_or_else(|e| warn!("Failed to await pinger: {e}"));
                    debug!("Shutdown task cancelled");
                    break;
                }
            };
            match player_count {
                -1 => {
                    if shutdown_handle.is_some() {
                        // server is offline, cancel shutdown
                        info!("Server is offline, cancelling shutdown.");
                        shutdown_token.cancel();
                        shutdown_handle
                            .unwrap()
                            .await
                            .unwrap_or_else(|e| warn!("Failed to await shutdown: {e}"));
                        shutdown_handle = None;
                        shutdown_token = token.child_token();
                    }
                }
                0 => {
                    if shutdown_handle.is_none() {
                        // start the shutdown
                        let sender = server_sender.clone();
                        let child_token = shutdown_token.clone();
                        shutdown_handle = Some(tokio::spawn(async move {
                            select! {
                                _ = child_token.cancelled() => {}
                                _ = tokio::time::sleep(wait_for) => {
                                    sender.send(ServerTaskRequest::Stop).await.unwrap_or_else(|e| warn!("Failed to send shutdown request: {e}"));
                                }
                            }
                        }));
                        info!("Server is empty, shutting down in {duration_string}.");
                    }
                }
                _ => {
                    if shutdown_handle.is_some() {
                        // cancel the shutdown
                        info!("Server has {player_count} players, cancelling shutdown.");
                        shutdown_token.cancel();
                        if let Some(handle) = shutdown_handle.take() {
                            handle
                                .await
                                .unwrap_or_else(|e| warn!("Failed to await shutdown: {e}"));
                            shutdown_handle = None;
                        }
                        shutdown_token = token.child_token();
                    }
                }
            }
        }
    })
}
