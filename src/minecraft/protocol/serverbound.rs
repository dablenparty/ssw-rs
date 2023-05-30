use async_trait::async_trait;
use getset::Getters;
use tokio::net::TcpStream;
use uuid::Uuid;

use super::{AsyncMinecraftReadExt, AsyncStreamReadable, ProtocolError};

#[derive(Debug, Getters)]
pub struct UncompressedServerboundPacket<T>
where
    T: AsyncStreamReadable<T>,
{
    length: i32,
    packet_id: i32,
    #[get = "pub"]
    data: T,
}

#[async_trait]
impl<T> AsyncStreamReadable<UncompressedServerboundPacket<T>> for UncompressedServerboundPacket<T>
where
    T: AsyncStreamReadable<T>,
{
    async fn read(stream: &mut TcpStream) -> Result<UncompressedServerboundPacket<T>, ProtocolError>
    where
        T: Sized,
    {
        let length = stream.read_varint().await?;
        let packet_id = stream.read_varint().await?;
        let data = T::read(stream).await?;

        Ok(UncompressedServerboundPacket {
            length,
            packet_id,
            data,
        })
    }
}

#[repr(u8)]
#[derive(Debug, PartialEq, Eq)]
pub enum NextState {
    Status = 1,
    Login = 2,
}

impl TryFrom<i32> for NextState {
    type Error = ProtocolError;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(NextState::Status),
            2 => Ok(NextState::Login),
            v => Err(ProtocolError::InvalidNextState(v)),
        }
    }
}

#[derive(Debug, Getters)]
#[get = "pub"]
pub struct HandshakePacket {
    protocol_version: i32,
    server_address: String,
    server_port: u16,
    next_state: NextState,
}

#[async_trait]
impl AsyncStreamReadable<HandshakePacket> for HandshakePacket {
    async fn read(stream: &mut TcpStream) -> Result<HandshakePacket, ProtocolError> {
        let protocol_version = stream.read_varint().await?;
        let server_address = stream.read_string().await?;
        let server_port = stream.read_unsigned_short().await?;
        let next_state = stream.read_varint().await?.try_into()?;

        Ok(HandshakePacket {
            protocol_version,
            server_address,
            server_port,
            next_state,
        })
    }
}

#[derive(Debug, Getters)]
#[get = "pub"]
pub struct LoginStartPacket {
    name: String,
    // thanks to Rust, we don't need this field. it's implicit in the Option
    // has_player_uuid: bool
    player_uuid: Option<Uuid>,
}

#[async_trait]
impl AsyncStreamReadable<LoginStartPacket> for LoginStartPacket {
    async fn read(stream: &mut TcpStream) -> Result<LoginStartPacket, ProtocolError> {
        let name = stream.read_string().await?;
        let has_player_uuid = stream.read_bool().await?;
        let player_uuid = if has_player_uuid {
            Some(stream.read_uuid().await?)
        } else {
            None
        };

        Ok(LoginStartPacket { name, player_uuid })
    }
}
