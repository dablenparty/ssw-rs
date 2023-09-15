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
    ChannelError(#[from] mpsc::error::SendError<ServerTaskRequest>),
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
            if let ProtocolError::InvalidNextState(_) = e {
                false
            } else {
                warn!("Error determining if connection is client: {e}");
                false
            }
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

/// Reads the first few packets from the stream and determines if it is a client connection.
///
/// If you look at older versions of this function, you'll see me complain about how
/// the Minecraft protocol is a pain to implement yourself. Long story short, I did
/// it. This function not only determines if the connection is a client connection,
/// but it also reads the username and UUID of the client on newer versions of
/// Minecraft.
async fn is_client_connection(stream: &mut TcpStream) -> Result<bool, ProtocolError> {
    let packet = UncompressedServerboundPacket::<HandshakePacket>::read(stream).await?;
    if *packet.data().next_state() != NextState::Login {
        return Ok(false);
    }
    if let Err(e) = read_player_info(stream).await {
        debug!("Error reading login start packet: {e}");
        debug!("This is likely caused by an older version of Minecraft");
    }
    Ok(true)
}

async fn read_player_info(stream: &mut TcpStream) -> Result<(), ProtocolError> {
    let login_start_packet =
        UncompressedServerboundPacket::<LoginStartPacket>::read(stream).await?;
    let username = login_start_packet.data().name();
    let uuid = login_start_packet.data().player_uuid();
    if let Some(uuid) = uuid {
        info!("Found client {username} with UUID {uuid}");
    } else {
        info!("Found client {username}");
    };
    Ok(())
}
