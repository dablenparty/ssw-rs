//! Minecraft protocol implementation
//! Information taken from here: https://wiki.vg/Protocol
// the protocol must read all fields of the packet, but we don't need to use them all
#![allow(dead_code)]

use async_trait::async_trait;
use getset::Getters;
use tokio::{io::AsyncReadExt, net::TcpStream};
use uuid::Uuid;

const SEGMENT_BITS: u8 = 0x7F;
const CONTINUE_BIT: u8 = 0x80;

// this trait is a little goofy looking, but it allows us to implement the same trait for multiple types
// and even recursively for types that contain other types that implement the trait (see `UncompressedMinecraftPacket`)
#[async_trait]
pub trait AsyncStreamReadable<T> {
    async fn read(stream: &mut TcpStream) -> Result<T, ProtocolError>
    where
        T: Sized;
}

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
        let length = read_varint(stream).await?;
        let packet_id = read_varint(stream).await?;
        let data = T::read(stream).await?;

        Ok(UncompressedServerboundPacket {
            length,
            packet_id,
            data,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("Invalid next state: {0}")]
    InvalidNextState(i32),
    #[error("Failed to read from stream: {0}")]
    StreamReadError(#[from] std::io::Error),
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
        let protocol_version = read_varint(stream).await?;
        let server_address = read_string(stream).await?;
        let server_port = read_unsigned_short(stream).await?;
        let next_state = read_varint(stream).await?.try_into()?;

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
        let name = read_string(stream).await?;
        let has_player_uuid = read_bool(stream).await?;
        let player_uuid = if has_player_uuid {
            Some(read_uuid(stream).await?)
        } else {
            None
        };

        Ok(LoginStartPacket { name, player_uuid })
    }
}

/// Reads a boolean from the stream
async fn read_bool(stream: &mut TcpStream) -> std::io::Result<bool> {
    let mut buf = [0u8; 1];
    stream.read_exact(&mut buf).await?;
    Ok(buf[0] == 1)
}

/// Reads a UUID from the stream
async fn read_uuid(stream: &mut TcpStream) -> std::io::Result<Uuid> {
    let mut buf = [0u8; 16];
    stream.read_exact(&mut buf).await?;
    Ok(Uuid::from_bytes(buf))
}

/// Reads an unsigned short (`u16`) from the stream
async fn read_unsigned_short(stream: &mut TcpStream) -> std::io::Result<u16> {
    let mut buf = [0u8; 2];
    stream.read_exact(&mut buf).await?;
    Ok(u16::from_be_bytes(buf))
}

/// Reads a `String` from the stream
async fn read_string(stream: &mut TcpStream) -> std::io::Result<String> {
    let length = read_varint(stream).await?;
    let mut buf = vec![0u8; length as usize];
    stream.read_exact(&mut buf).await?;
    Ok(String::from_utf8(buf).expect("Invalid UTF-8 sent by Minecraft client"))
}

/// Reads a `VarInt` from the stream
async fn read_varint(stream: &mut TcpStream) -> std::io::Result<i32> {
    let mut buf = [0u8; 1];
    let mut idx = 0;
    let mut value = 0;
    loop {
        stream.read(&mut buf).await?;
        let next_byte = buf[0];
        value |= ((next_byte & SEGMENT_BITS) as i32) << (idx);

        if (next_byte & CONTINUE_BIT) == 0 {
            break;
        }

        idx += 7;

        if idx >= 64 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "VarInt too big",
            ));
        }
    }
    Ok(value)
}
