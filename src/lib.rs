//! # Qonductor
//!
//! Rust implementation of the Qobuz Connect protocol.
//!
//! ## Quick Start
//!
//! ```ignore
//! use qonductor::{SessionManager, DeviceConfig, SessionEvent};
//!
//! #[tokio::main]
//! async fn main() -> qonductor::Result<()> {
//!     let (mut manager, mut events) = SessionManager::start(7864).await?;
//!     manager.add_device(DeviceConfig::new("Living Room", "your_app_id")).await?;
//!
//!     tokio::spawn(async move { manager.run().await });
//!
//!     while let Some(event) = events.recv().await {
//!         match event {
//!             SessionEvent::PlaybackCommand { renderer_id, state, .. } => {
//!                 println!("Playback command: {:?}", state);
//!             }
//!             _ => {}
//!         }
//!     }
//!     Ok(())
//! }
//! ```

pub mod credentials;
pub mod error;
pub mod manager;
pub mod types;

// Internal modules
pub(crate) mod discovery;
pub(crate) mod qconnect;
pub(crate) mod transport;

// Re-export main public API
pub use manager::{SessionManager, SessionManagerHandle};
pub use qconnect::{
    ActivationState, BufferState, LoopMode, PlayState, PlaybackCommand, PlaybackResponse,
    QueueTrack, RendererBroadcast, RendererHandler,
};
pub use discovery::{DeviceConfig, DeviceType, AudioQuality};

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
