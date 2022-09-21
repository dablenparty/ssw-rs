use std::{
    collections::{hash_map, HashMap},
    io,
    net::SocketAddr,
    time::Duration,
};

use log::{debug, error, info, warn};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpListener, TcpStream,
    },
    select,
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

enum ConnectionManagerEvent {
    Connected(SocketAddr, JoinHandle<()>),
    Disconnected(SocketAddr),
}

pub async fn run_proxy(ssw_port: u32, cancellation_token: CancellationToken) -> io::Result<()> {
    info!("Starting proxy on port {}", ssw_port);
    let addr = format!("{}:{}", "0.0.0.0", ssw_port);

    let listener = TcpListener::bind(&addr).await?;
    info!("Listening on {}", addr);

    let (connection_manager_tx, mut connection_manager_rx) =
        tokio::sync::mpsc::channel::<ConnectionManagerEvent>(100);
    // TODO: when error handling is improved, shut this down properly
    let connection_manager_token = cancellation_token.clone();
    let _connection_manager_handle = tokio::spawn(async move {
        // TODO: set initial capacity to server player limit
        let mut connections: HashMap<SocketAddr, JoinHandle<()>> = HashMap::new();
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
                    break;
                }
            }
        }
    });

    loop {
        let (client, client_addr) = listener.accept().await?;
        debug!("Accepted connection from {}", client_addr);
        let tx_clone = connection_manager_tx.clone();
        let connection_token = cancellation_token.clone();
        let connection_handle = tokio::spawn(async move {
            select! {
                r = connection_handler(client) => {
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

/// Handles a connection from a client to the proxy.
/// This will open an extra stream to the server and copy the IO between the two, bidirectionally.
///
/// # Arguments
///
/// * `client_stream` - The `TcpStream` from the client.
///
/// returns: `io::Result<()>`
async fn connection_handler(mut client_stream: TcpStream) -> io::Result<()> {
    // TODO: get port from server
    let mut server_stream = TcpStream::connect("127.0.0.1:25566").await?;
    // I'm upset I didn't find this sooner. This function solved almost all performance issues with
    // the proxy and passing TCP packets between the client and server.
    tokio::io::copy_bidirectional(&mut client_stream, &mut server_stream).await?;
    Ok(())
}

/// A simple proxy that forwards all data from the `from` stream to the `to` stream.
///
/// # Arguments
///
/// * `from` - The stream to read data from
/// * `to` - The stream to write data to
///
/// # Errors
///
/// An error will be returned if one occurs at any step along the way. Waiting for streams to be readable/writable,
/// reading/writing data, or closing the streams. However, it is **not** considered an error if no data is read after
/// 15 seconds.
async fn pass_between_streams(mut from: OwnedReadHalf, mut to: OwnedWriteHalf) -> io::Result<()> {
    from.readable().await?;
    to.writable().await?;
    // TODO: extract constant for buffer size
    let mut timeouts = 0;
    loop {
        const BUFFER_SIZE: usize = 16 * 1024;
        let mut buf = [0; BUFFER_SIZE];
        let read_future = from.read(&mut buf);
        if let Ok(n) = tokio::time::timeout(Duration::from_secs(5), read_future).await {
            let n = n?;
            // most often occurs when EOF is reached
            if n == 0 {
                break;
            }
            to.write_all(&buf[..n]).await?;
            to.flush().await?;
        } else {
            timeouts += 1;
            // TODO: extract to constant
            if timeouts >= 3 {
                // stop trying to read after 3 timeouts
                warn!("No data received from client for 15 seconds, closing connection");
                break;
            }
        }
    }
    Ok(())
}
