use std::time::Duration;

use duration_string::DurationString;
use log::{debug, error};
use tokio::{select, sync::mpsc::Sender, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use super::ServerTaskRequest;

// TODO: USE THIS!!!
pub fn begin_restart_task(
    wait_for: Duration,
    server_sender: Sender<ServerTaskRequest>,
    token: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        const MESSAGE_PREFIX: &str = "/me is restarting in";
        const DURATION_SPLITS: [usize; 6] = [3600, 900, 300, 60, 15, 1];
        let mut time_left = wait_for;
        for split in &DURATION_SPLITS {
            let split_duration = Duration::from_secs(*split as u64);
            let mut ticker = tokio::time::interval(split_duration);
            ticker.tick().await; // immediately tick to start
            while time_left > split_duration {
                let duration_string = DurationString::from(time_left);
                let msg = format!("{MESSAGE_PREFIX} {duration_string}");
                if let Err(e) = server_sender.send(ServerTaskRequest::Command(msg)).await {
                    error!("Failed to send restart notification: {e}");
                }
                select! {
                    _ = ticker.tick() => {
                        time_left -= split_duration;
                    }
                    _ = token.cancelled() => {
                        debug!("Restart task cancelled");
                        return;
                    }
                }
            }
        }
        // send restart request
        if let Err(e) = server_sender.send(ServerTaskRequest::Restart).await {
            error!("Failed to send restart request: {e}");
        }
    })
}
