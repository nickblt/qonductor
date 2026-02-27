//! # Qonductor
//!
//! Rust implementation of the Qobuz Connect protocol.
//!
//! ## Quick Start
//!
//! ```ignore
//! use qonductor::{SessionManager, DeviceConfig, SessionEvent, Command, Notification, msg};
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
//!             SessionEvent::Command(cmd) => match cmd {
//!                 Command::SetState { cmd, respond } => {
//!                     respond.send(msg::QueueRendererState { /* ... */ });
//!                 }
//!                 Command::SetActive { respond, .. } => {
//!                     respond.send(ActivationState { /* ... */ });
//!                 }
//!                 Command::Heartbeat { respond } => {
//!                     respond.send(None);
//!                 }
//!             },
//!             SessionEvent::Notification(n) => match n {
//!                 Notification::Connected => println!("Connected!"),
//!                 _ => {}
//!             },
//!         }
//!     }
//!     Ok(())
//! }
//! ```

pub mod config;
pub mod credentials;
pub mod error;
pub mod event;
pub mod manager;
pub mod msg;
pub mod session;
pub mod types;

// Internal modules
pub(crate) mod connection;
pub(crate) mod discovery;
pub(crate) mod qconnect;

// Re-export main public API
pub use manager::SessionManager;
pub use event::{ActivationState, Command, Notification, Responder, SessionEvent};
pub use session::{DeviceSession, SessionCommand};
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
    #[allow(clippy::all)]
    pub mod qconnect {
        include!("proto/qconnect.rs");
    }
    #[allow(clippy::all)]
    pub mod ws {
        include!("proto/ws.rs");
    }
}
