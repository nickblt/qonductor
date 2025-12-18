//! High-level session management for Qobuz Connect.
//!
//! The `SessionManager` is the main entry point for the library.
//! It handles device discovery, session management, and event routing.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::discovery::{DeviceConfig, DeviceRegistry, DeviceSelected};
use crate::session::{PlayState, SessionEvent, SessionHandle};
use crate::{Error, Result};

/// Manager for Qobuz Connect sessions.
///
/// This is the main entry point for using Qobuz Connect. It handles:
/// - Device registration and mDNS announcements
/// - Automatic session creation when devices are selected
/// - Event routing from all sessions to a single channel
///
/// Each device gets its own WebSocket connection to the Qobuz server.
///
/// # Example
///
/// ```ignore
/// let (mut manager, mut events) = SessionManager::start(7864).await?;
/// manager.add_device(DeviceConfig::new("Living Room", &app_id)).await?;
/// manager.add_device(DeviceConfig::new("Kitchen", &app_id)).await?;
///
/// // Spawn manager to handle device selections
/// tokio::spawn(async move { manager.run().await });
///
/// // Handle events
/// while let Some(event) = events.recv().await {
///     match event {
///         SessionEvent::PlaybackCommand { renderer_id, state, .. } => {
///             // Handle playback
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
    event_tx: mpsc::Sender<SessionEvent>,
}

impl SessionManager {
    /// Start the session manager with HTTP server on the given port.
    ///
    /// Returns the manager and a receiver for all session events.
    ///
    /// # Arguments
    ///
    /// * `port` - Port for the HTTP server. Use 0 for automatic port selection.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let (mut manager, mut events) = SessionManager::start(7864).await?;
    /// ```
    pub async fn start(port: u16) -> Result<(Self, mpsc::Receiver<SessionEvent>)> {
        let (registry, device_rx) = DeviceRegistry::start(port).await?;
        let (event_tx, event_rx) = mpsc::channel(100);

        Ok((
            Self {
                registry,
                device_rx,
                sessions: Arc::new(Mutex::new(HashMap::new())),
                event_tx,
            },
            event_rx,
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

    /// Report playback state to the server.
    ///
    /// Call this when playback state changes (play, pause, stop, seek).
    pub async fn report_playback_state(
        &self,
        renderer_id: u64,
        state: PlayState,
        position_ms: u32,
        current_queue_item_id: Option<i32>,
    ) -> Result<()> {
        // Find the session that might own this renderer
        // Since we don't track renderer_id -> session mapping, try all sessions
        // Clone handles so we don't hold lock across await
        let handles: Vec<_> = self.sessions.lock().unwrap().values().cloned().collect();
        for session in handles {
            // Try to send - if the session doesn't have this renderer, it will still work
            // (the command is fire-and-forget for the renderer_id lookup)
            if let Ok(()) = session
                .report_playback_state(renderer_id, state, position_ms, current_queue_item_id)
                .await
            {
                return Ok(());
            }
        }

        Err(Error::Protocol(format!(
            "No session found for renderer {}",
            renderer_id
        )))
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
            self.event_tx.clone(),
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
