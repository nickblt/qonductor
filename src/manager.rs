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
/// - Routing events to the user via the event channel
///
/// Each device gets its own WebSocket connection to the Qobuz server.
///
/// # Example
///
/// ```ignore
/// let (mut manager, mut events) = SessionManager::start(7864).await?;
/// manager.add_device(DeviceConfig::new("Living Room", &app_id)).await?;
///
/// // Spawn manager to handle device selections and route events
/// tokio::spawn(async move { manager.run().await });
///
/// // Handle events
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
    /// Internal event channel - receives from all SessionRunners.
    internal_rx: mpsc::Receiver<SessionEvent>,
    /// Internal sender - cloned to each SessionRunner.
    internal_tx: mpsc::Sender<SessionEvent>,
    /// User-facing event channel - we forward events here.
    user_tx: mpsc::Sender<SessionEvent>,
}

impl SessionManager {
    /// Start the session manager with HTTP server on the given port.
    ///
    /// Returns the manager and a receiver for events.
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
        let (internal_tx, internal_rx) = mpsc::channel(100);
        let (user_tx, user_rx) = mpsc::channel(100);

        Ok((
            Self {
                registry,
                device_rx,
                internal_rx,
                internal_tx,
                user_tx,
            },
            user_rx,
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
    /// This handles:
    /// - Device selections (creates sessions)
    /// - Event routing (forwards events to user)
    ///
    /// Must be called for events to flow to the user.
    ///
    /// # Example
    ///
    /// ```ignore
    /// tokio::spawn(async move { manager.run().await });
    /// ```
    pub async fn run(&mut self) -> Result<()> {
        info!("SessionManager starting");

        loop {
            tokio::select! {
                // Handle device selections
                selected = self.device_rx.recv() => {
                    match selected {
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

                // Route events from sessions to user
                event = self.internal_rx.recv() => {
                    match event {
                        Some(e) => {
                            // Forward to user
                            if self.user_tx.send(e).await.is_err() {
                                warn!("User event channel closed");
                                break;
                            }
                        }
                        None => {
                            // All internal senders dropped - no more sessions
                        }
                    }
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

        info!(
            device = %device_config.friendly_name,
            "Device selected, creating session"
        );

        spawn_session(
            &selected.session_info,
            &device_config,
            self.internal_tx.clone(),
        )
        .await
    }
}
