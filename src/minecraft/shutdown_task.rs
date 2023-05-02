use std::time::Duration;

use duration_string::DurationString;
use log::{info, warn};
use tokio::{
    select,
    sync::{mpsc::Sender, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use super::ServerTaskRequest;

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
                        info!("Server is empty, shutting down in {duration_string}.");
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

fn begin_ping_task(
    server_address: String,
    token: CancellationToken,
) -> (JoinHandle<()>, watch::Receiver<i64>) {
    const PING_INTERVAL: Duration = Duration::from_secs(5);
    let (sender, receiver) = watch::channel::<i64>(-1);
    let handle = tokio::spawn(async move {
        let java_config = mcping::Java {
            server_address,
            timeout: Some(PING_INTERVAL),
        };
        let mut interval = tokio::time::interval(PING_INTERVAL);
        loop {
            // Wait for the next tick or cancellation
            select! {
                _ = interval.tick() => {}
                _ = token.cancelled() => {
                    break;
                }
            }
            // Now wait for the ping to complete or cancellation again
            select! {
                r = mcping::tokio::get_status(java_config.clone()) => {
                    match r {
                        Ok((_, status)) => {
                            let player_count = status.players.online;
                            sender.send(player_count).unwrap_or_else(|e| warn!("Failed to send player count: {e}"));
                        }
                        Err(_) => {
                            // this will error if the server is offline, so just set the player count to -1
                            // logging every error would be too spammy
                            sender.send(-1).unwrap_or_else(|e| warn!("Failed to send player count: {e}"));
                        }
                    }
                }
                _ = token.cancelled() => {
                    break;
                }
            }
        }
    });
    (handle, receiver)
}
