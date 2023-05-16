use log::{debug, error, info, warn};
use thiserror::Error;
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    select,
    sync::mpsc,
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use super::{
    protocol::{
        serverbound::{
            HandshakePacket, LoginStartPacket, NextState, UncompressedServerboundPacket,
        },
        AsyncStreamReadable, ProtocolError,
    },
    ServerTaskRequest,
};

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
        let (mut stream, addr) = listener.accept().await?;
        debug!("Accepted connection from {addr}");
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

/// Reads the first packet from the stream and determines if it is a client connection.
///
/// In all honesty, I have little clue why this works. I spent an embarrassing amount of
/// time studying the packets sent by Minecraft clients and found that the initial handshake
/// seems to always end with a `0x02` byte. I'm not sure if this is a Minecraft thing or a
/// Java thing, but it seems to work pretty well. The problem is, if any OTHER packets come
/// in that also end with a `0x02` byte, this will return a false positive. I'm not sure if
/// there's a better way to do this, so it works for now. Without this check, any and all
/// connections (pings, scans, anything) would start the server. I've tried that and it's
/// not fun.
async fn is_client_connection(stream: &mut TcpStream) -> Result<bool, ProtocolError> {
    let packet = UncompressedServerboundPacket::<HandshakePacket>::read(stream).await?;
    if *packet.data().next_state() != NextState::Login {
        return Ok(false);
    }
    let login_start_packet =
        UncompressedServerboundPacket::<LoginStartPacket>::read(stream).await?;
    let username = login_start_packet.data().name();
    let uuid = login_start_packet.data().player_uuid();
    if let Some(uuid) = uuid {
        info!("Found client {username} with UUID {uuid}");
    } else {
        info!("Found client {username}");
    }
    Ok(true)
}
