//! High-level session management for Qobuz Connect.
//!
//! The `SessionManager` is the main entry point for the library.
//! It handles device discovery, session management, and event routing.

use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::config::DeviceConfig;
use crate::discovery::{DeviceRegistry, DeviceSelected};
use crate::qconnect::{spawn_session, SessionEvent};
use crate::{Error, Result};

/// Manager for Qobuz Connect sessions.
///
/// This is the main entry point for using Qobuz Connect. It handles:
/// - Device registration and mDNS announcements
/// - Automatic session creation when devices are selected
///
/// Each device gets its own WebSocket connection and event channel.
///
/// # Example
///
/// ```ignore
/// let mut manager = SessionManager::start(7864).await?;
/// let mut events = manager.add_device(DeviceConfig::new("Living Room", &app_id)).await?;
///
/// // Spawn manager to handle device selections
/// tokio::spawn(async move { manager.run().await });
///
/// // Handle events for this device
/// while let Some(event) = events.recv().await {
///     match event {
///         SessionEvent::PlaybackCommand { cmd, respond, .. } => {
///             respond.send(my_player.handle_playback(cmd));
///         }
///         SessionEvent::QueueUpdated { tracks, .. } => {
///             my_player.queue = tracks;
///         }
///         _ => {}
///     }
/// }
/// ```
pub struct SessionManager {
    registry: DeviceRegistry,
    device_rx: mpsc::Receiver<DeviceSelected>,
}

impl SessionManager {
    /// Start the session manager with HTTP server on the given port.
    ///
    /// # Arguments
    ///
    /// * `port` - Port for the HTTP server. Use 0 for automatic port selection.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut manager = SessionManager::start(7864).await?;
    /// let events = manager.add_device(device_config).await?;
    /// ```
    pub async fn start(port: u16) -> Result<Self> {
        let (registry, device_rx) = DeviceRegistry::start(port).await?;

        Ok(Self {
            registry,
            device_rx,
        })
    }

    /// Register a device for discovery.
    ///
    /// Starts mDNS announcement for the device. When a user selects this device
    /// in the Qobuz app, a session will be automatically created.
    ///
    /// Returns a receiver for session events specific to this device.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let events = manager.add_device(DeviceConfig::new("Living Room", &app_id)).await?;
    /// while let Some(event) = events.recv().await {
    ///     // Handle events for this device
    /// }
    /// ```
    pub async fn add_device(
        &self,
        config: DeviceConfig,
    ) -> Result<mpsc::Receiver<SessionEvent>> {
        self.registry.add_device(config).await
    }

    /// Unregister a device.
    ///
    /// Stops mDNS announcement. The session will end when the server
    /// notices the device is gone.
    pub async fn remove_device(&mut self, device_uuid: &[u8; 16]) -> Result<()> {
        self.registry.remove_device(device_uuid).await
    }

    /// Get all registered device configurations.
    pub async fn devices(&self) -> Vec<DeviceConfig> {
        self.registry.devices().await
    }

    /// Run the manager event loop.
    ///
    /// This handles device selections and creates sessions.
    /// Events are delivered directly to each device's event channel.
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
                Some(s) => {
                    if let Err(e) = self.handle_device_selected(s).await {
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
        // Get the device config
        let device_config = self
            .registry
            .get_device(&selected.device_uuid)
            .await
            .ok_or_else(|| Error::Discovery("Device not found".to_string()))?;

        // Get the device's event sender
        let event_tx = self
            .registry
            .get_event_tx(&selected.device_uuid)
            .await
            .ok_or_else(|| Error::Discovery("Device event channel not found".to_string()))?;

        info!(
            device = %device_config.friendly_name,
            "Device selected, creating session"
        );

        spawn_session(&selected.session_info, &device_config, event_tx).await
    }
}
