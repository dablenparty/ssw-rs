use std::{io, time::Duration};

use log::{debug, error, info, warn};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpListener, TcpStream,
    },
    task::JoinHandle,
};

pub async fn run_proxy(ssw_port: u32) -> io::Result<()> {
    info!("Starting proxy on port {}", ssw_port);
    // TODO: resolve public IP and use it in the proxy
    let addr = format!("{}:{}", "127.0.0.1", ssw_port);

    let listener = TcpListener::bind(&addr).await?;
    info!("Listening on {}", addr);

    // TODO: set initial capacity to server player limit
    let mut connections: Vec<JoinHandle<()>> = Vec::new();

    loop {
        let (client, client_addr) = listener.accept().await?;
        debug!("Accepted connection from {}", client_addr);
        let connection_handle = tokio::spawn(async move {
            if let Err(e) = connection_handler(client).await {
                error!("Error handling connection: {}", e);
            } else {
                debug!("Connection lost from {}", client_addr);
            }
        });
        connections.push(connection_handle);
    }
}

async fn connection_handler(client_stream: TcpStream) -> io::Result<()> {
    // TODO: get port from server
    let server_stream = TcpStream::connect("127.0.0.1:25566").await?;
    // into_split is less efficient than split, but allows concurrent read/write
    let (from_client, to_client) = client_stream.into_split();
    let (from_server, to_server) = server_stream.into_split();
    let client_to_server = pass_between_streams(from_client, to_server);
    let server_to_client = pass_between_streams(from_server, to_client);
    let results = futures::future::join_all(vec![client_to_server, server_to_client]).await;
    for r in results {
        if let Err(e) = r {
            error!("Error passing data between streams: {}", e);
        }
    }
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
        let mut buf = [0; 4096];
        let read_future = from.read(&mut buf);
        match tokio::time::timeout(Duration::from_secs(5), read_future).await {
            Ok(n) => {
                let n = n?;
                // most often occurs when EOF is reached
                if n == 0 {
                    break;
                }
                to.write_all(&buf[..n]).await?;
                to.flush().await?;
            }
            Err(_) => {
                timeouts += 1;
                // TODO: extract to constant
                if timeouts >= 3 {
                    // stop trying to read after 3 timeouts
                    warn!("No data received from client for 15 seconds, closing connection");
                    break;
                }
            }
        }
    }
    Ok(())
}
