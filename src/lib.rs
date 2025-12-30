//! # Qonductor
//!
//! Rust implementation of the Qobuz Connect protocol.
//!
//! ## Quick Start
//!
//! ```ignore
//! use qonductor::{SessionManager, DeviceConfig, SessionEvent, ActivationState, PlaybackResponse};
//!
//! #[tokio::main]
//! async fn main() -> qonductor::Result<()> {
//!     let mut manager = SessionManager::start(7864).await?;
//!     let mut session = manager.add_device(DeviceConfig::new("Living Room", "your_app_id")).await?;
//!
//!     tokio::spawn(async move { manager.run().await });
//!
//!     while let Some(event) = session.recv().await {
//!         match event {
//!             SessionEvent::PlaybackCommand { renderer_id, cmd, respond } => {
//!                 println!("Playback command: {:?}", cmd);
//!                 respond.send(PlaybackResponse { /* ... */ });
//!             }
//!             SessionEvent::Activate { renderer_id, respond } => {
//!                 respond.send(ActivationState { /* ... */ });
//!             }
//!             _ => {}
//!         }
//!
//!         // Player can also send state updates to the server
//!         session.report_state(PlaybackResponse { /* ... */ }).await?;
//!     }
//!     Ok(())
//! }
//! ```

pub mod config;
pub mod credentials;
pub mod error;
pub mod manager;
pub mod types;

// Internal modules
pub(crate) mod connection;
pub(crate) mod discovery;
pub(crate) mod qconnect;

// Re-export main public API
pub use manager::SessionManager;
pub use qconnect::{
    ActivationState, BroadcastEvent, CommandEvent, DeviceEvent, DeviceSession, PlaybackCommand,
    PlaybackResponse, QueueTrack, Responder, SessionCommand, SessionEvent, SystemEvent,
};
pub use proto::qconnect::{BufferState, DeviceType, LoopMode, PlayingState};
pub use config::{AudioQuality, DeviceConfig};
pub use discovery::DeviceTypeExt;
pub use connection::format_qconnect_message;

pub use error::Error;
pub use types::*;

/// Result type for qonductor operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Generated protobuf types.
pub mod proto {
    pub mod qconnect {
        include!(concat!(env!("OUT_DIR"), "/qconnect.rs"));
    }
    pub mod ws {
        include!(concat!(env!("OUT_DIR"), "/ws.rs"));
    }
}
