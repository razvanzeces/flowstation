use std::time::Instant;

use serde::{Deserialize, Serialize};

#[cfg(test)]
pub mod mock;

/// QUIC transport implementation
pub mod quic;

/// WebSocket transport implementation
pub mod websocket;

/// Basic TCP transport implementation
pub mod tcp;

/// Largest accepted length-prefixed message on the stream transports (TCP, QUIC reliable).
pub(crate) const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// Pull every complete `u32-BE length + payload` frame out of `buf`, leaving any partial
/// tail in place for the next read.
///
/// Stream reads split frames across calls whenever a message spans TCP segments. Consuming
/// those partial bytes and then discarding them — what `read_exact` on a non-blocking socket
/// does — desyncs the framing permanently, so the buffer has to survive between calls.
/// An over-long length prefix means the stream is already desynced: the caller is told to
/// drop the connection rather than try to resynchronise.
pub(crate) fn drain_length_prefixed(buf: &mut Vec<u8>) -> Result<Vec<Vec<u8>>, String> {
    let mut frames = Vec::new();
    loop {
        if buf.len() < 4 {
            return Ok(frames);
        }
        let mut len_bytes = [0u8; 4];
        len_bytes.copy_from_slice(&buf[..4]);
        let payload_len = u32::from_be_bytes(len_bytes) as usize;
        if payload_len > MAX_FRAME_BYTES {
            return Err(format!("framing desync: message length {} bytes", payload_len));
        }
        if buf.len() < 4 + payload_len {
            return Ok(frames); // incomplete — keep the tail buffered
        }
        frames.push(buf[4..4 + payload_len].to_vec());
        buf.drain(..4 + payload_len);
    }
}

/// Network transport abstraction for Entity-to-network external communications
///
/// This trait defines a unified interface for both reliable (TCP, QUIC streams)
/// and unreliable (UDP, QUIC datagrams) transports. Transports should either
/// implement those methods or raise an unimplemented!() panic.
pub trait NetworkTransport: Send {
    /// Connect or reconnect the transport. Destroys any existing connection.
    fn connect(&mut self) -> Result<(), NetworkError>;

    /// Send a message reliably (guaranteed delivery, ordered arrival)
    fn send_reliable(&mut self, payload: &[u8]) -> Result<(), NetworkError>;

    /// Send a message unreliably (no delivery guarantee, unordered, lower latency)
    fn send_unreliable(&mut self, payload: &[u8]) -> Result<(), NetworkError>;

    /// Receive pending messages from the reliable channel (non-blocking)
    fn receive_reliable(&mut self) -> Vec<NetworkMessage>;

    /// Receive pending messages from the unreliable channel (non-blocking)
    fn receive_unreliable(&mut self) -> Vec<NetworkMessage>;

    /// Wait for a single response on the reliable channel (blocking with timeout)
    fn wait_for_response_reliable(&mut self) -> Result<NetworkMessage, NetworkError>;

    /// Disconnect the transport gracefully
    fn disconnect(&mut self) {}

    /// Check if the transport is currently connected
    fn is_connected(&self) -> bool {
        true
    }

    /// Return the Brew protocol version advertised by the server in the last connect response.
    /// Default is 0 (v0 / unknown). WebSocketTransport overrides this.
    fn server_brew_version(&self) -> u8 {
        0
    }

    /// Return the Brew extensions the server offered in the last connect response
    /// (`X-Brew-Extensions`). Empty when it offered none, as TetraPack and BrandMeister do.
    /// WebSocketTransport overrides this.
    fn server_brew_extensions(&self) -> &[String] {
        &[]
    }
}

/// Factory trait for creating transport instances
///
/// Each transport type implements this to define how it gets constructed
/// from a configuration type. This allows generic workers to create transports
/// without knowing the specific construction details.
pub trait TransportFactory: NetworkTransport + Sized {
    /// Configuration type needed to construct this transport
    type Config: Send + 'static;

    /// Create a new transport instance from configuration
    fn create(config: Self::Config) -> Result<Self, NetworkError>;
}

/// Network address abstraction
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NetworkAddress {
    /// TCP endpoint
    Tcp { host: String, port: u16 },
    /// UDP endpoint  
    Udp { host: String, port: u16 },
    /// Custom addressing scheme
    Custom { scheme: String, address: String },
}

/// Network message received from external source
#[derive(Debug, Clone)]
pub struct NetworkMessage {
    pub source: NetworkAddress,
    pub payload: Vec<u8>,
    pub timestamp: Instant,
}

/// Network-related errors
#[derive(Debug, Clone)]
pub enum NetworkError {
    ConnectionFailed(String),
    SendFailed(String),
    ReceiveFailed(String),
    SerializationError(String),
    InvalidService(String),
    InvalidServiceVersion(String),
    Timeout,
}

impl std::fmt::Display for NetworkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetworkError::ConnectionFailed(msg) => write!(f, "Connection failed: {}", msg),
            NetworkError::SendFailed(msg) => write!(f, "Send failed: {}", msg),
            NetworkError::SerializationError(msg) => write!(f, "Serialization error: {}", msg),
            NetworkError::InvalidService(msg) => write!(f, "Invalid service: {}", msg),
            NetworkError::InvalidServiceVersion(msg) => write!(f, "Invalid service version: {}", msg),
            NetworkError::ReceiveFailed(_) => write!(f, "Receive failed"),
            NetworkError::Timeout => write!(f, "Operation timed out"),
        }
    }
}

impl std::error::Error for NetworkError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_survives_a_split_read() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&4u32.to_be_bytes());
        buf.extend_from_slice(&[1, 2]); // only half the payload arrived

        assert!(drain_length_prefixed(&mut buf).unwrap().is_empty());
        assert_eq!(buf.len(), 6, "partial frame must stay buffered");

        buf.extend_from_slice(&[3, 4]);
        assert_eq!(drain_length_prefixed(&mut buf).unwrap(), vec![vec![1, 2, 3, 4]]);
        assert!(buf.is_empty());
    }

    #[test]
    fn framing_handles_several_frames_in_one_read() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&2u32.to_be_bytes());
        buf.extend_from_slice(&[9, 9]);
        buf.extend_from_slice(&1u32.to_be_bytes());
        buf.extend_from_slice(&[7]);
        buf.extend_from_slice(&5u32.to_be_bytes()); // header of a frame that has not arrived

        assert_eq!(drain_length_prefixed(&mut buf).unwrap(), vec![vec![9, 9], vec![7]]);
        assert_eq!(buf.len(), 4);
    }

    #[test]
    fn framing_rejects_an_absurd_length_prefix() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&u32::MAX.to_be_bytes());
        assert!(drain_length_prefixed(&mut buf).is_err());
    }
}
