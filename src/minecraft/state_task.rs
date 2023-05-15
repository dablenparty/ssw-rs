use log::{debug, info};
use tokio::{
    select,
    sync::{mpsc, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerState {
    On,
    Off,
}

/// Starts a task that holds the current server state.
///
/// This may seem like an over-abstraction, but it essentially allows me to
/// treat server state changes as events and react to them in a more functional
/// way.
pub fn begin_state_task(
    token: CancellationToken,
) -> (
    JoinHandle<()>,
    (mpsc::Sender<ServerState>, watch::Receiver<ServerState>),
) {
    let (external_tx, mut internal_rx) = mpsc::channel::<ServerState>(2);
    let (internal_tx, external_rx) = watch::channel(ServerState::Off);
    let handle = tokio::spawn(async move {
        loop {
            select! {
                _ = token.cancelled() => {
                    debug!("Status task cancelled");
                    break;
                },
                Some(new_state) = internal_rx.recv() => {
                    let state_changed = internal_tx.send_if_modified(|old_state| {
                        if *old_state == new_state {
                            false
                        } else {
                            *old_state = new_state;
                            true
                        }
                    });
                    if state_changed {
                        info!("Server state changed to {new_state:?}");
                    }
                }
            }
        }
    });
    (handle, (external_tx, external_rx))
}
