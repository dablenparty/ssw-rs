//! Minecraft protocol implementation
//! Information taken from [the wiki](https://wiki.vg/Protocol)
// the protocol must read all fields of the packet, but we don't need to use them all
#![allow(dead_code)]

use async_trait::async_trait;
use tokio::{io::AsyncReadExt, net::TcpStream};
use uuid::Uuid;

pub mod serverbound;

const SEGMENT_BITS: u8 = 0x7F;
const CONTINUE_BIT: u8 = 0x80;

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("Invalid next state: {0}")]
    InvalidNextState(i32),
    #[error("Failed to read from stream: {0}")]
    StreamReadError(#[from] std::io::Error),
    #[error("VarInt too big")]
    VarIntTooBig,
    #[error("Invalid UTF-8 sent by client: {0}")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
}

// this trait is a little goofy looking, but it allows me to implement the same trait for multiple types
// and even recursively for types that contain other types that implement the trait (see `UncompressedMinecraftPacket`)
#[async_trait]
pub trait AsyncStreamReadable<T> {
    async fn read(stream: &mut TcpStream) -> Result<T, ProtocolError>
    where
        T: Sized;
}

#[async_trait]
pub trait AsyncMinecraftReadExt: Sized {
    /// Reads a boolean from the stream
    async fn read_bool(&mut self) -> std::io::Result<bool>;
    /// Reads a UUID from the stream
    async fn read_uuid(&mut self) -> std::io::Result<Uuid>;
    /// Reads an unsigned short (`u16`) from the stream
    async fn read_unsigned_short(&mut self) -> std::io::Result<u16>;
    /// Reads an owned `String` from the stream
    async fn read_string(&mut self) -> Result<String, ProtocolError>;
    /// Reads a `VarInt` from the stream as an `i32`
    async fn read_varint(&mut self) -> Result<i32, ProtocolError>;
}

#[async_trait]
impl AsyncMinecraftReadExt for TcpStream {
    /// Reads a boolean from the stream
    async fn read_bool(&mut self) -> std::io::Result<bool> {
        let mut buf = [0u8; 1];
        self.read_exact(&mut buf).await?;
        Ok(buf[0] == 1)
    }

    /// Reads a UUID from the stream
    async fn read_uuid(&mut self) -> std::io::Result<Uuid> {
        let mut buf = [0u8; 16];
        self.read_exact(&mut buf).await?;
        Ok(Uuid::from_bytes(buf))
    }

    /// Reads an unsigned short (`u16`) from the stream
    async fn read_unsigned_short(&mut self) -> std::io::Result<u16> {
        let mut buf = [0u8; 2];
        self.read_exact(&mut buf).await?;
        // in my testing, endianness didn't matter, but from my research Minecraft clients
        // use little-endian for unsigned shorts
        Ok(u16::from_le_bytes(buf))
    }

    /// Reads a `String` from the stream
    async fn read_string(&mut self) -> Result<String, ProtocolError> {
        let length = self.read_varint().await?;
        // lengths are guaranteed to be positive, so we can safely cast to usize
        #[allow(clippy::cast_sign_loss)]
        let mut buf = vec![0u8; length as usize];
        self.read_exact(&mut buf).await?;
        Ok(String::from_utf8(buf)?)
    }

    /// Reads a `VarInt` from the stream
    async fn read_varint(&mut self) -> Result<i32, ProtocolError> {
        let mut buf = [0u8; 1];
        let mut idx = 0;
        let mut value = 0;
        loop {
            let n = self.read(&mut buf).await?;
            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "Unexpected EOF",
                ))?;
            }
            let next_byte = buf[0];
            value |= (i32::from(next_byte & SEGMENT_BITS)) << (idx);

            if (next_byte & CONTINUE_BIT) == 0 {
                break;
            }

            idx += 7;

            if idx >= 64 {
                return Err(ProtocolError::VarIntTooBig);
            }
        }
        Ok(value)
    }
}
