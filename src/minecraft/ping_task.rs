use std::time::Duration;

use log::warn;
use tokio::{select, sync::watch, task::JoinHandle};
use tokio_util::sync::CancellationToken;

pub fn begin_ping_task(
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
