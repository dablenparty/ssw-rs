use std::{
    collections::{hash_map, HashMap},
    io,
    net::SocketAddr,
};

use log::{debug, error, info, warn};
use tokio::{
    net::{TcpListener, TcpStream},
    select,
    sync::mpsc::{Receiver, Sender},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::minecraft::DEFAULT_MC_PORT;
use crate::SswEvent;

enum ConnectionManagerEvent {
    Connected(SocketAddr, JoinHandle<()>),
    Disconnected(SocketAddr),
}

/// Runs the proxy server
///
/// This includes a connection manager that keeps track of all connections as well as a TCP listener
/// that accepts new connections and removes them from the connection manager when they disconnect.
///
/// # Arguments
///
/// * `ssw_port` - The port for the proxy to listen on
/// * `cancellation_token` - The cancellation token to use
///
/// # Errors
///
/// An error is returned if the TCP listener fails to bind to the port or accept a new connection.
pub async fn run_proxy(
    ssw_port: u16,
    cancellation_token: CancellationToken,
    tx: Sender<SswEvent>,
    mut rx: Receiver<u16>,
) -> io::Result<()> {
    debug!("Starting proxy on port {}", ssw_port);
    // TODO: optional command line arg for IP
    let addr = format!("{}:{}", "0.0.0.0", ssw_port);

    let listener = TcpListener::bind(&addr).await?;
    debug!("Listening on {}", addr);

    let (connection_manager_tx, connection_manager_rx) =
        tokio::sync::mpsc::channel::<ConnectionManagerEvent>(100);
    // TODO: when error handling is improved, shut this down properly
    let connection_manager_token = cancellation_token.clone();
    let _connection_manager_handle = tokio::spawn(async move {
        connection_manager(connection_manager_rx, connection_manager_token).await;
    });

    loop {
        let (client, client_addr) = listener.accept().await?;
        debug!("Accepted connection from {}", client_addr);

        let mc_port = if let Err(e) = tx.send(SswEvent::McPortRequest).await {
            error!("Failed to request MC port: {}", e);
            error!("Using default port {}", DEFAULT_MC_PORT);
            DEFAULT_MC_PORT
        } else {
            rx.recv().await.unwrap_or(DEFAULT_MC_PORT)
        };
        if mc_port == ssw_port {
            warn!(
                "MC port is the same as SSW port, ignoring connection from {}",
                client_addr
            );
            continue;
        }
        let tx_clone = connection_manager_tx.clone();
        let connection_token = cancellation_token.clone();
        let connection_handle = tokio::spawn(async move {
            select! {
                r = connection_handler(client, mc_port) => {
                    if let Err(e) = r {
                        warn!("Error handling connection: {}", e);
                    } else {
                        debug!("Connection lost from {}", client_addr);
                    }
                    if let Err(e) = tx_clone
                        .send(ConnectionManagerEvent::Disconnected(client_addr))
                        .await
                    {
                        error!("Error sending connection manager event: {}", e);
                    }
                },
                _ = connection_token.cancelled() => {
                    debug!("Connection cancelled from {}", client_addr);
                }
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
    mut connection_manager_rx: Receiver<ConnectionManagerEvent>,
    connection_manager_token: CancellationToken,
) {
    // TODO: set initial capacity to server player limit
    let mut connections = HashMap::<SocketAddr, JoinHandle<()>>::new();
    loop {
        select! {
            msg = connection_manager_rx.recv() => {
                if let Some(event) = msg {
                    match event {
                        ConnectionManagerEvent::Connected(addr, handle) => {
                            if let hash_map::Entry::Vacant(entry) = connections.entry(addr) {
                                entry.insert(handle);
                            } else {
                                // this shouldn't happen
                                warn!("Connection already exists for {}", addr);
                            }
                        }
                        ConnectionManagerEvent::Disconnected(addr) => {
                            connections.remove(&addr);
                        }
                    }
                    debug!("There are now {} connections", connections.len());
                } else {
                    error!("Connection manager channel closed");
                }
            },

            _ = connection_manager_token.cancelled() => {
                info!("Waiting for all proxy connections to close");
                futures::future::join_all(connections.drain().map(|(_, handle)| handle)).await;
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
