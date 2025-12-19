//! High-level session management for Qobuz Connect.
//!
//! The `SessionManager` is the main entry point for the library.
//! It handles device discovery, session management, and event routing.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::config::DeviceConfig;
use crate::discovery::{DeviceRegistry, DeviceSelected};
use crate::qconnect::{RendererBroadcast, RendererHandler, SessionHandle, SharedHandler};
use crate::{Error, Result};

/// Handle for interacting with sessions while the manager is running.
///
/// This can be cloned and used from multiple tasks concurrently with `SessionManager::run()`.
#[derive(Clone)]
pub struct SessionManagerHandle {
    sessions: Arc<Mutex<HashMap<[u8; 16], Arc<SessionHandle>>>>,
}

impl SessionManagerHandle {
    /// Request current queue state from the server.
    ///
    /// The server will respond by calling `handler.on_queue_update()`.
    pub async fn request_queue_state(&self) -> Result<()> {
        let handles: Vec<_> = self.sessions.lock().unwrap().values().cloned().collect();
        for session in handles {
            if let Ok(()) = session.request_queue_state().await {
                return Ok(());
            }
        }

        Err(Error::Protocol("No active session".to_string()))
    }

    /// Request current renderer state from the server.
    ///
    /// The server will respond with a `RendererStateUpdated` broadcast.
    pub async fn request_renderer_state(&self) -> Result<()> {
        let handles: Vec<_> = self.sessions.lock().unwrap().values().cloned().collect();
        for session in handles {
            if let Ok(()) = session.request_renderer_state().await {
                return Ok(());
            }
        }

        Err(Error::Protocol("No active session".to_string()))
    }
}

/// Manager for Qobuz Connect sessions.
///
/// This is the main entry point for using Qobuz Connect. It handles:
/// - Device registration and mDNS announcements
/// - Automatic session creation when devices are selected
/// - Calling [`RendererHandler`] methods for commands and sending responses automatically
///
/// Each device gets its own WebSocket connection to the Qobuz server.
///
/// # Example
///
/// ```ignore
/// struct MyPlayer { /* ... */ }
/// impl RendererHandler for MyPlayer { /* ... */ }
///
/// let player = Arc::new(Mutex::new(MyPlayer::new()));
/// let (mut manager, mut broadcasts) = SessionManager::start(7864, player).await?;
/// manager.add_device(DeviceConfig::new("Living Room", &app_id)).await?;
///
/// // Spawn manager to handle device selections
/// tokio::spawn(async move { manager.run().await });
///
/// // Handle broadcasts (optional - informational only)
/// while let Some(broadcast) = broadcasts.recv().await {
///     match broadcast {
///         RendererBroadcast::RendererStateUpdated { renderer_id, state, .. } => {
///             println!("Renderer {} state: {:?}", renderer_id, state);
///         }
///         _ => {}
///     }
/// }
/// ```
pub struct SessionManager {
    registry: DeviceRegistry,
    device_rx: mpsc::Receiver<DeviceSelected>,
    /// Sessions keyed by device_uuid (each device gets its own connection).
    sessions: Arc<Mutex<HashMap<[u8; 16], Arc<SessionHandle>>>>,
    /// Handler for renderer commands (shared across all sessions).
    handler: SharedHandler,
    /// Channel for broadcast events.
    broadcast_tx: mpsc::Sender<RendererBroadcast>,
}

impl SessionManager {
    /// Start the session manager with HTTP server on the given port.
    ///
    /// Returns the manager and a receiver for broadcast events.
    ///
    /// # Arguments
    ///
    /// * `port` - Port for the HTTP server. Use 0 for automatic port selection.
    /// * `handler` - Implementation of [`RendererHandler`] for handling commands.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let player = Arc::new(Mutex::new(MyPlayer::new()));
    /// let (mut manager, mut broadcasts) = SessionManager::start(7864, player).await?;
    /// ```
    pub async fn start<H: RendererHandler>(
        port: u16,
        handler: H,
    ) -> Result<(Self, mpsc::Receiver<RendererBroadcast>)> {
        let (registry, device_rx) = DeviceRegistry::start(port).await?;
        let (broadcast_tx, broadcast_rx) = mpsc::channel(100);

        Ok((
            Self {
                registry,
                device_rx,
                sessions: Arc::new(Mutex::new(HashMap::new())),
                handler: Arc::new(Mutex::new(handler)),
                broadcast_tx,
            },
            broadcast_rx,
        ))
    }

    /// Register a device for discovery.
    ///
    /// Starts mDNS announcement for the device. When a user selects this device
    /// in the Qobuz app, a session will be automatically created.
    ///
    /// # Example
    ///
    /// ```ignore
    /// manager.add_device(DeviceConfig::new("Living Room", &app_id)).await?;
    /// ```
    pub async fn add_device(&self, config: DeviceConfig) -> Result<()> {
        self.registry.add_device(config).await
    }

    /// Unregister a device.
    ///
    /// Stops mDNS announcement and removes the device from any active session.
    pub async fn remove_device(&mut self, device_uuid: &[u8; 16]) -> Result<()> {
        self.registry.remove_device(device_uuid).await?;

        // Note: We don't currently have a way to remove devices from sessions.
        // The server will remove them when the mDNS goes away.

        Ok(())
    }

    /// Get all registered device configurations.
    pub async fn devices(&self) -> Vec<DeviceConfig> {
        self.registry.devices().await
    }

    /// Get a handle for interacting with sessions.
    ///
    /// The handle can be cloned and used from multiple tasks while `run()` is active.
    /// Use this to call `request_queue_state()` and `request_renderer_state()`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let handle = manager.handle();
    /// tokio::spawn(async move { manager.run().await });
    ///
    /// // Use handle from another task
    /// handle.request_queue_state().await?;
    /// ```
    pub fn handle(&self) -> SessionManagerHandle {
        SessionManagerHandle {
            sessions: Arc::clone(&self.sessions),
        }
    }

    /// Run the manager event loop.
    ///
    /// This handles device selections and creates sessions. Sessions notify
    /// the manager via callback when they disconnect.
    ///
    /// # Example
    ///
    /// ```ignore
    /// tokio::spawn(async move { manager.run().await });
    /// ```
    pub async fn run(&mut self) -> Result<()> {
        info!("SessionManager starting");

        loop {
            match self.device_rx.recv().await {
                Some(selected) => {
                    if let Err(e) = self.handle_device_selected(selected).await {
                        error!(error = %e, "Failed to handle device selection");
                    }
                }
                None => {
                    warn!("Device selection channel closed");
                    break;
                }
            }
        }

        Ok(())
    }

    /// Handle a device being selected in the Qobuz app.
    async fn handle_device_selected(&mut self, selected: DeviceSelected) -> Result<()> {
        let device_uuid = selected.device_uuid;

        // Get the device config
        let device_config = self
            .registry
            .get_device(&device_uuid)
            .await
            .ok_or_else(|| Error::Discovery("Device not found".to_string()))?;

        info!(
            device = %device_config.friendly_name,
            "Device selected, creating session"
        );

        // Create callback to remove session on disconnect
        let sessions_clone = Arc::clone(&self.sessions);
        let on_disconnect = move || {
            debug!("Session disconnected, removing from manager");
            sessions_clone.lock().unwrap().remove(&device_uuid);
        };

        let handle = SessionHandle::connect(
            &selected.session_info,
            &device_config,
            Arc::clone(&self.handler),
            self.broadcast_tx.clone(),
            on_disconnect,
        )
        .await?;

        self.sessions
            .lock()
            .unwrap()
            .insert(device_uuid, Arc::new(handle));

        Ok(())
    }
}
