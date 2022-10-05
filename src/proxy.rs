use std::{
    collections::{hash_map, HashMap},
    io,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use log::{debug, error, info, warn};
use tokio::{
    net::{TcpListener, TcpStream},
    select,
    sync::{broadcast, mpsc},
    task::JoinHandle,
    time::Duration,
};
use tokio_util::sync::CancellationToken;

use crate::minecraft::{MCServerState, SswConfig, DEFAULT_MC_PORT};
use crate::SswEvent;

enum ConnectionManagerEvent {
    Connected(SocketAddr, JoinHandle<()>),
    SetReal(SocketAddr),
    Disconnected(SocketAddr),
}

struct ConnectedClient {
    addr: SocketAddr,
    handle: JoinHandle<()>,
    is_real: bool,
}

pub struct ProxyConfig {
    /// SSW config to use.
    pub ssw_config: SswConfig,
    /// Thread-safe mutex around the Minecraft server state.
    pub server_state: Arc<Mutex<MCServerState>>,
    /// Sender to send events to the main thread.
    pub ssw_event_tx: mpsc::Sender<SswEvent>,
    /// Receiver to receive Minecraft server port changes.
    pub server_port_rx: mpsc::Receiver<u16>,
    /// Receiver to receive Minecraft server state changes.
    pub server_state_rx: broadcast::Receiver<MCServerState>,
}

/// Runs the proxy server
///
/// This includes a connection manager that keeps track of all connections as well as a TCP listener
/// that accepts new connections and removes them from the connection manager when they disconnect.
///
/// # Arguments
///
/// * `config` - The ProxyConfig struct containing all the necessary information to run the proxy
/// * `cancellation_token` - The cancellation token to use
///
/// # Errors
///
/// An error is returned if the TCP listener fails to bind to the port or accept a new connection.
pub async fn run_proxy(
    config: ProxyConfig,
    cancellation_token: CancellationToken,
) -> io::Result<()> {
    let ProxyConfig {
        ssw_config,
        server_state,
        ssw_event_tx,
        mut server_port_rx,
        server_state_rx,
    } = config;
    const REAL_CONNECTION_SECS: u64 = 5;
    debug!("Starting proxy on port {}", ssw_config.ssw_port);
    // TODO: optional command line arg for IP
    let addr = format!("{}:{}", "0.0.0.0", ssw_config.ssw_port);

    let listener = TcpListener::bind(&addr).await?;
    debug!("Listening on {}", addr);

    let (connection_manager_tx, connection_manager_rx) =
        tokio::sync::mpsc::channel::<ConnectionManagerEvent>(100);
    // TODO: when error handling is improved, shut this down properly
    let connection_manager_token = cancellation_token.clone();
    let _connection_manager_handle = tokio::spawn(connection_manager(
        connection_manager_rx,
        server_state_rx,
        connection_manager_token,
        server_state,
    ));

    loop {
        let (client, client_addr) = listener.accept().await?;
        debug!("Accepted connection from {}", client_addr);

        let mc_port = if let Err(e) = ssw_event_tx.send(SswEvent::McPortRequest).await {
            error!("Failed to request MC port: {}", e);
            error!("Using default port {}", DEFAULT_MC_PORT);
            DEFAULT_MC_PORT
        } else {
            server_port_rx.recv().await.unwrap_or(DEFAULT_MC_PORT)
        };
        if mc_port == ssw_config.ssw_port {
            warn!(
                "MC port is the same as SSW port, ignoring connection from {}",
                client_addr
            );
            continue;
        }
        let tx_clone = connection_manager_tx.clone();
        let connection_token = cancellation_token.clone();
        let connection_handle = tokio::spawn(async move {
            // this isn't necessarily needed, but it covers the bases
            let conn_timer_token = connection_token.clone();
            let real_tx_clone = tx_clone.clone();
            let real_conn_timer = tokio::spawn(async move {
                select! {
                    _ = tokio::time::sleep(Duration::from_secs(REAL_CONNECTION_SECS)) => {
                        debug!("Real connection accepted, cancelling shutdown timer");
                        if let Err(e) = real_tx_clone.send(ConnectionManagerEvent::SetReal(client_addr)).await {
                            error!("Failed to set {} to real connection: {}", client_addr, e);
                        }
                    }
                    _ = conn_timer_token.cancelled() => {
                        debug!("Cancelled real connection timer");
                    }
                }
            });
            select! {
                r = connection_handler(client, mc_port) => {
                    if let Err(e) = r {
                        warn!("Error handling connection: {}", e);
                    } else {
                        debug!("Connection lost from {}", client_addr);
                    }
                },
                _ = connection_token.cancelled() => {
                    debug!("Connection cancelled from {}", client_addr);
                }
            }
            real_conn_timer.abort();
            if let Err(e) = tx_clone
                .send(ConnectionManagerEvent::Disconnected(client_addr))
                .await
            {
                error!("Error sending connection manager event: {}", e);
            }
        });
        if let Err(e) = connection_manager_tx
            .send(ConnectionManagerEvent::Connected(
                client_addr,
                connection_handle,
            ))
            .await
        {
            error!("Error sending connection event: {}", e);
        }
    }
}

/// Run the connection manager
///
/// This is responsible for keeping track of all connections and waiting for them to finish.
/// There are a few expectations for this function:
/// * It will be spawned as a task
/// * It will be cancelled when the proxy is shutting down
/// * It will be sent a `ConnectionManagerEvent` for every connection
/// * The `JoinHandle` for each connection will be cancelled outside of this function
///
/// # Arguments
///
/// * `connection_manager_rx` - The channel to receive connection events from
/// * `cancellation_token` - The cancellation token to use
async fn connection_manager(
    mut connection_manager_rx: mpsc::Receiver<ConnectionManagerEvent>,
    mut server_state_channel: broadcast::Receiver<MCServerState>,
    connection_manager_token: CancellationToken,
    server_state: Arc<Mutex<MCServerState>>,
) {
    // TODO: set initial capacity to server player limit
    let mut connections = HashMap::<SocketAddr, ConnectedClient>::new();
    let mut shutdown_task = None;
    loop {
        if connections.is_empty() && shutdown_task.is_none() {
            let current_state = { *server_state.lock().unwrap() };
            if current_state == MCServerState::Running {
                info!("Server is empty, starting shutdown timer");
                shutdown_task = Some(());
            }
        }
        select! {
            msg = connection_manager_rx.recv() => {
                if let Some(event) = msg {
                    match event {
                        ConnectionManagerEvent::Connected(addr, handle) => {
                            if let hash_map::Entry::Vacant(entry) = connections.entry(addr) {
                                entry.insert(ConnectedClient {
                                    addr,
                                    handle,
                                    is_real: false,
                                });
                            } else {
                                // this shouldn't happen
                                warn!("Connection already exists for {}", addr);
                            }
                        }
                        ConnectionManagerEvent::Disconnected(addr) => {
                            connections.remove(&addr);
                        }
                        ConnectionManagerEvent::SetReal(addr) => {
                            if let hash_map::Entry::Occupied(mut entry) = connections.entry(addr) {
                                entry.get_mut().is_real = true;
                                info!("Player joined, cancelling shutdown timer");
                                shutdown_task = None;
                            } else {
                                // this shouldn't happen
                                warn!("Connection doesn't exist for {}", addr);
                            }
                        }
                    }
                    debug!("There are now {} connections", connections.len());
                } else {
                    error!("Connection manager channel closed");
                }
            },
            state = server_state_channel.recv() => {
                // TODO: this is a bit of a hack, but it works for now
                // forces a re-check of connections if server state changes
                if let Err(e) = state {
                    error!("Error waiting for server state: {}", e);
                }
            },
            _ = connection_manager_token.cancelled() => {
                info!("Waiting for all proxy connections to close");
                futures::future::join_all(connections.drain().map(|(_, client)| client.handle)).await;
                info!("All proxy connections closed");
                break;
            }
        }
    }
}

/// Handles a connection from a client to the proxy.
/// This will open an extra stream to the server and copy the IO between the two, bidirectionally.
///
/// # Arguments
///
/// * `client_stream` - The `TcpStream` from the client.
///
/// returns: `io::Result<()>`
async fn connection_handler(mut client_stream: TcpStream, mc_port: u16) -> io::Result<()> {
    let mut server_stream = TcpStream::connect(format!("127.0.0.1:{}", mc_port)).await?;
    // I'm upset I didn't find this sooner. This function solved almost all performance issues with
    // the proxy and passing TCP packets between the client and server.
    tokio::io::copy_bidirectional(&mut client_stream, &mut server_stream).await?;
    Ok(())
}
