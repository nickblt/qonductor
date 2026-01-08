//! Device session handle for bidirectional communication.
//!
//! This module provides [`DeviceSession`], the user-facing handle returned by
//! [`SessionManager::add_device()`](crate::SessionManager::add_device).

use std::sync::Arc;

use tokio::sync::{RwLock, mpsc};

use crate::event::SessionEvent;
use crate::msg::QueueRendererState;

// ============================================================================
// Session Commands (user -> server)
// ============================================================================

/// Commands that can be sent from the application to the Qobuz server.
///
/// These are used when the player initiates state changes (not in response
/// to server commands).
#[derive(Debug, Clone)]
pub enum SessionCommand {
    /// Report current playback state to server.
    ReportState(QueueRendererState),
    /// Report volume level (0-100).
    ReportVolume(u32),
    /// Report mute state.
    ReportVolumeMuted(bool),
    /// Report max audio quality capability (1-4).
    ReportMaxAudioQuality(i32),
    /// Report current file's sample rate in Hz.
    ReportFileAudioQuality(u32),
}

// ============================================================================
// Device Session Handle
// ============================================================================

/// Shared command sender, set when session connects.
pub(crate) type SharedCommandTx = Arc<RwLock<Option<mpsc::Sender<SessionCommand>>>>;

/// Handle for a device session providing bidirectional communication.
///
/// This is returned by `SessionManager::add_device()` and provides:
/// - Receiving events from the server via `recv()`
/// - Sending state updates to the server via `report_*()` methods
///
/// Note: `report_*()` methods will return an error if called before the
/// Qobuz app connects to this device.
///
/// # Example
///
/// ```ignore
/// use qonductor::{SessionEvent, Command, Notification, msg};
/// use qonductor::msg::PositionExt;
///
/// let mut session = manager.add_device(config).await?;
///
/// while let Some(event) = session.recv().await {
///     match event {
///         SessionEvent::Command(cmd) => match cmd {
///             Command::SetState { cmd, respond, .. } => {
///                 respond.send(msg::QueueRendererState {
///                     playing_state: Some(msg::PlayingState::Playing as i32),
///                     current_position: Some(msg::Position::now(0)),
///                     ..Default::default()
///                 });
///             }
///             Command::SetActive { respond, .. } => { /* ... */ }
///             Command::Heartbeat { respond, .. } => respond.send(None),
///         },
///         SessionEvent::Notification(n) => { /* ... */ }
///     }
/// }
/// ```
pub struct DeviceSession {
    events: mpsc::Receiver<SessionEvent>,
    command_tx: SharedCommandTx,
}

impl DeviceSession {
    /// Create a new device session handle.
    pub(crate) fn new(events: mpsc::Receiver<SessionEvent>, command_tx: SharedCommandTx) -> Self {
        Self { events, command_tx }
    }

    /// Receive the next event from the session.
    ///
    /// Returns `None` when the session is closed.
    pub async fn recv(&mut self) -> Option<SessionEvent> {
        self.events.recv().await
    }

    /// Send a command to the server.
    async fn send_command(&self, cmd: SessionCommand) -> crate::Result<()> {
        let guard = self.command_tx.read().await;
        match &*guard {
            Some(tx) => tx
                .send(cmd)
                .await
                .map_err(|_| crate::Error::Session("Session closed".to_string())),
            None => Err(crate::Error::Session("Not connected".to_string())),
        }
    }

    /// Report current playback state to the server.
    ///
    /// Use this when the player initiates state changes (pause, seek, track change)
    /// rather than responding to server commands.
    ///
    /// The library extrapolates position during playback: provide `current_position`
    /// with `timestamp` (when measured) and `value` (position at that time).
    ///
    /// Returns an error if not connected to the Qobuz server.
    pub async fn report_state(&self, state: QueueRendererState) -> crate::Result<()> {
        self.send_command(SessionCommand::ReportState(state)).await
    }

    /// Report volume level to the server.
    pub async fn report_volume(&self, volume: u32) -> crate::Result<()> {
        self.send_command(SessionCommand::ReportVolume(volume))
            .await
    }

    /// Report mute state to the server.
    pub async fn report_muted(&self, muted: bool) -> crate::Result<()> {
        self.send_command(SessionCommand::ReportVolumeMuted(muted))
            .await
    }

    /// Report max audio quality capability to the server.
    pub async fn report_max_audio_quality(&self, quality: i32) -> crate::Result<()> {
        self.send_command(SessionCommand::ReportMaxAudioQuality(quality))
            .await
    }

    /// Report current file's audio quality (sample rate in Hz) to the server.
    pub async fn report_file_audio_quality(&self, sample_rate_hz: u32) -> crate::Result<()> {
        self.send_command(SessionCommand::ReportFileAudioQuality(sample_rate_hz))
            .await
    }
}
