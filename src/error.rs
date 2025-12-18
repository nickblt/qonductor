//! Error types for qonductor.

use thiserror::Error;

/// Main error type for qonductor operations.
#[derive(Debug, Error)]
pub enum Error {
    /// Authentication failed.
    #[error("authentication failed: {0}")]
    Auth(String),

    /// HTTP request failed.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// WebSocket error.
    #[error("WebSocket error: {0}")]
    WebSocket(#[from] Box<tokio_tungstenite::tungstenite::Error>),

    /// Protobuf decoding error.
    #[error("protobuf error: {0}")]
    Proto(#[from] prost::DecodeError),

    /// JSON parsing error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Invalid response from server.
    #[error("invalid response: {0}")]
    InvalidResponse(String),

    /// Track not found.
    #[error("track not found: {0}")]
    TrackNotFound(u64),

    /// Stream error.
    #[error("stream error: {0}")]
    Stream(String),

    /// Session expired.
    #[error("session expired")]
    SessionExpired,

    /// Connection closed.
    #[error("connection closed")]
    ConnectionClosed,

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// mDNS error.
    #[error("mDNS error: {0}")]
    Mdns(String),

    /// Discovery server error.
    #[error("discovery error: {0}")]
    Discovery(String),

    /// Protocol error.
    #[error("protocol error: {0}")]
    Protocol(String),
}
