use log::{debug, error, info, warn};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    select,
    sync::mpsc,
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use super::ServerTaskRequest;

pub fn begin_listener_task(
    server_address: String,
    server_sender: mpsc::Sender<ServerTaskRequest>,
    token: CancellationToken,
) -> JoinHandle<std::io::Result<()>> {
    tokio::spawn(async move {
        let listener = TcpListener::bind(&server_address).await?;
        info!("Listening on {server_address}");
        loop {
            select! {
                r = listener.accept() => {
                    match r {
                        Ok((mut stream, _)) => {
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
                                if let Err(e) = server_sender.send(ServerTaskRequest::Start).await {
                                    error!("Failed to send start request: {e}");
                                }
                                break;
                            }
                        }
                        Err(e) => {
                            error!("Error accepting connection: {e}");
                        }
                    }
                }
                _ = token.cancelled() => {
                    debug!("Listener task cancelled");
                    break;
                }
            }
        }
        Ok(())
    })
}

async fn is_client_connection(stream: &mut TcpStream) -> std::io::Result<bool> {
    let mut buf = [0u8; 1];
    stream.read_exact(&mut buf).await?;
    let next_size = buf[0] as usize;
    let mut buf = vec![0u8; next_size];
    stream.read_exact(&mut buf).await?;
    Ok(buf[next_size - 1] == b'\x02')
}
