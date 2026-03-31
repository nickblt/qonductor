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
//!     let mut manager = SessionManager::start(7864, "your_app_id").await?;
//!     let mut session = manager.add_device(DeviceConfig::new("Living Room")).await?;
//!
//!     tokio::spawn(async move { manager.run().await });
//!
//!     while let Some(event) = session.recv().await {
//!         match event {
//!             SessionEvent::Command(cmd) => match cmd {
//!                 Command::SetState { cmd, respond } => {
//!                     respond.send(msg::QueueRendererState { /* ... */ });
//!                 }
//!                 Command::SetVolume { cmd, respond } => {
//!                     respond.send(msg::report::VolumeChanged { volume: cmd.volume });
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
pub use config::{AudioQuality, DeviceConfig};
pub use connection::format_qconnect_message;
pub use discovery::DeviceTypeExt;
pub use event::{ActivationState, Command, Notification, Responder, SessionEvent};
pub use manager::SessionManager;
pub use proto::qconnect::{BufferState, DeviceType, LoopMode, PlayingState};
pub use session::{DeviceSession, SessionCommand};

pub use error::Error;
pub use types::*;

/// Result type for qonductor operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Connect directly to a Qobuz Connect WebSocket endpoint, bypassing mDNS/HTTP discovery.
///
/// `ws_endpoint` is the WebSocket URL (e.g., `"wss://qws-us-prod.qobuz.com/ws"`).
/// `ws_jwt` is the JWT token for WebSocket authentication.
///
/// This is useful when you already have credentials (e.g., from the Qobuz API) and don't
/// need the full `SessionManager` flow. The returned [`DeviceSession`] behaves identically
/// to one obtained via [`SessionManager::add_device()`].
pub async fn connect(
    ws_endpoint: &str,
    ws_jwt: &str,
    device_config: &DeviceConfig,
) -> Result<DeviceSession> {
    use std::sync::Arc;
    use tokio::sync::{RwLock, mpsc};

    let (event_tx, event_rx) = mpsc::channel(100);
    let (command_tx, command_rx) = mpsc::channel(100);
    let shared_command_tx: session::SharedCommandTx = Arc::new(RwLock::new(Some(command_tx)));

    let session_id = {
        use rand::Rng;
        let bytes: [u8; 16] = rand::thread_rng().r#gen();
        let u = uuid::Uuid::from_bytes(bytes);
        u.hyphenated().to_string()
    };

    let session_info = config::SessionInfo {
        session_id,
        ws_endpoint: ws_endpoint.to_string(),
        ws_jwt: ws_jwt.to_string(),
        ws_jwt_exp: 0,
        api_jwt: String::new(),
        api_jwt_exp: 0,
    };

    qconnect::spawn_session(&session_info, device_config, event_tx, command_rx).await?;

    Ok(DeviceSession::new(event_rx, shared_command_tx))
}

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
