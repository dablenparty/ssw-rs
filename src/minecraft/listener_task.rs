use log::{error, info, warn};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    select,
    sync::mpsc,
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use super::ServerTaskRequest;

#[derive(Debug, Error)]
enum ListenerError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Tokio channel error: {0}")]
    ChannelError(#[from] tokio::sync::mpsc::error::SendError<ServerTaskRequest>),
}

pub fn begin_listener_task(
    server_address: String,
    server_sender: mpsc::Sender<ServerTaskRequest>,
    token: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        select! {
            _ = token.cancelled() => {
                info!("Listener task cancelled");
            }
            result = inner_listener(server_address, server_sender) => {
                if let Err(e) = result {
                    error!("Listener task failed: {e}");
                }
            }
        }
    })
}

async fn inner_listener(
    server_address: String,
    server_sender: mpsc::Sender<ServerTaskRequest>,
) -> Result<(), ListenerError> {
    let listener = TcpListener::bind(&server_address).await?;
    info!("Listening on {server_address}");
    loop {
        let (mut stream, _) = listener.accept().await?;
        // logging every socket connection would spam the debug logs as the pinger task will be connecting every 5 seconds
        let is_client = is_client_connection(&mut stream).await.unwrap_or_else(|e| {
            warn!("Failed to read client connection: {e}");
            false
        });
        if let Err(e) = stream.shutdown().await {
            error!("Failed to shutdown stream: {e}");
        }
        if is_client {
            info!("Client connected, starting server");
            // start server
            server_sender.send(ServerTaskRequest::Start).await?;
            break;
        }
    }
    Ok(())
}

async fn is_client_connection(stream: &mut TcpStream) -> std::io::Result<bool> {
    let mut buf = [0u8; 1];
    stream.read_exact(&mut buf).await?;
    let next_size = buf[0] as usize;
    let mut buf = vec![0u8; next_size];
    stream.read_exact(&mut buf).await?;
    Ok(buf[next_size - 1] == b'\x02')
}
