//! Session event types for Qobuz Connect.
//!
//! This module defines the event types that flow from the session to the user:
//! - [`SessionEvent`] - Top-level event wrapper
//! - [`Command`] - Events requiring a response via [`Responder<T>`]
//! - [`Notification`] - Informational events (non-exhaustive)

use tokio::sync::oneshot;

use crate::msg::{self, QueueRendererState};
use crate::proto::qconnect::{QConnectMessage, QConnectMessageType};

// ============================================================================
// Handler Types
// ============================================================================

/// Initial state reported during activation handshake.
///
/// When a device becomes active, this state is sent to the server
/// to establish initial volume, quality, and playback state.
#[derive(Debug, Clone)]
pub struct ActivationState {
    /// Whether audio is muted.
    pub muted: bool,
    /// Current volume (0-100).
    pub volume: u32,
    /// Maximum audio quality capability level (1-4).
    /// 1 = MP3, 2 = FLAC Lossless, 3 = HiRes 96kHz, 4 = HiRes 192kHz
    pub max_quality: i32,
    /// Current playback state.
    pub playback: QueueRendererState,
}

// ============================================================================
// Responder
// ============================================================================

/// A responder for sending required responses back to the server.
///
/// Commands that require a response include a `Responder`. You must call
/// `.send()` to provide the response, or the session will hang waiting.
///
/// # Example
///
/// ```ignore
/// match event {
///     SessionEvent::PlaybackCommand { cmd, respond, .. } => {
///         let response = handle_playback(cmd);
///         respond.send(response);  // Required!
///     }
///     _ => {}
/// }
/// ```
#[must_use = "call .send() to respond or the session will hang"]
pub struct Responder<T> {
    tx: oneshot::Sender<T>,
}

impl<T> Responder<T> {
    /// Create a new responder (internal use).
    pub(crate) fn new(tx: oneshot::Sender<T>) -> Self {
        Self { tx }
    }

    /// Send the response.
    ///
    /// This consumes the responder. The session runner will receive
    /// the response and send it to the server.
    pub fn send(self, value: T) {
        let _ = self.tx.send(value);
    }
}

// ============================================================================
// Session Events (server -> user)
// ============================================================================

/// Events from a Qobuz Connect session.
///
/// Events are split into two categories:
/// - [`Command`]: Must respond via [`Responder<T>`]. Exhaustively matchable.
/// - [`Notification`]: Informational events. Use `_ =>` for forward compatibility.
///
/// # Example
///
/// ```ignore
/// use qonductor::{SessionEvent, Command, Notification, PlaybackResponse, ActivationState};
///
/// while let Some(event) = session.recv().await {
///     match event {
///         SessionEvent::Command(cmd) => match cmd {
///             Command::SetState { cmd, respond, .. } => {
///                 respond.send(handle_playback(&cmd));
///             }
///             Command::SetActive { respond, .. } => {
///                 respond.send(activation_state());
///             }
///             Command::Heartbeat { respond, .. } => {
///                 respond.send(heartbeat_response());
///             }
///         },
///         SessionEvent::Notification(n) => match n {
///             Notification::QueueState(queue) => {
///                 println!("Queue has {} tracks", queue.tracks.len());
///             }
///             Notification::Connected => println!("Connected!"),
///             _ => {} // Ignore unknown notifications
///         },
///     }
/// }
/// ```
pub enum SessionEvent {
    /// Commands that require a response via [`Responder<T>`].
    ///
    /// This is a closed set - new command types would be breaking changes.
    Command(Command),

    /// Informational notifications.
    ///
    /// This is `#[non_exhaustive]` - new variants may be added in future versions.
    /// Always include a `_ =>` catch-all in your match.
    Notification(Notification),
}

/// Commands requiring a response via [`Responder<T>`].
///
/// This is a closed set - adding new commands is a breaking change.
/// You must call `respond.send(value)` for each command or the session will hang.
pub enum Command {
    /// Server commands play/pause/seek. Must respond with [`QueueRendererState`].
    SetState {
        cmd: msg::cmd::SetState,
        respond: Responder<QueueRendererState>,
    },

    /// Device activation. Must respond with [`ActivationState`].
    SetActive {
        cmd: msg::cmd::SetActive,
        respond: Responder<ActivationState>,
    },

    /// Heartbeat during playback. Respond with `Some(QueueRendererState)` or `None`.
    Heartbeat {
        respond: Responder<Option<QueueRendererState>>,
    },
}

// ============================================================================
// Notification Macro - Single Source of Truth
// ============================================================================

/// Generates `Notification` enum variants and a dispatcher function.
///
/// Each entry: `VariantName, proto_field_name, MessageTypeEnumVariant;`
macro_rules! define_notifications {
    (
        $(
            $variant:ident, $field:ident, $msg_type:ident
        );* $(;)?
    ) => {
        /// Informational notifications from the session.
        ///
        /// Handle what you need, ignore the rest. Always include a `_ =>` catch-all
        /// as new variants may be added in future versions.
        #[derive(Debug)]
        #[non_exhaustive]
        pub enum Notification {
            // === Proto-backed notifications (generated) ===
            $(
                $variant(msg::notify::$variant),
            )*

            // === Special cases (manual) ===

            /// Device was deactivated (another renderer became active).
            Deactivated,

            /// State from previous renderer before activation (for restoring position).
            RestoreState(msg::notify::RendererStateUpdated),

            /// WebSocket connected successfully.
            Connected,

            /// WebSocket disconnected.
            Disconnected {
                session_id: String,
                reason: Option<String>,
            },

            /// Device was registered with the server.
            DeviceRegistered {
                device_uuid: [u8; 16],
                renderer_id: u64,
                /// JWT token for Qobuz API authentication.
                api_jwt: String,
            },

            /// Session closed (WebSocket disconnected or error).
            SessionClosed { device_uuid: [u8; 16] },
        }

        /// Try to dispatch a proto message to a Notification.
        /// Returns `Some(Notification)` if the message type matches a simple notification.
        pub(crate) fn dispatch_notification(msg: &mut QConnectMessage) -> Option<Notification> {
            let msg_type = msg.message_type?;
            $(
                if msg_type == QConnectMessageType::$msg_type as i32 {
                    return msg.$field.take().map(Notification::$variant);
                }
            )*
            None
        }
    };
}

// Single source of truth for proto-backed notifications
define_notifications! {
    // Session
    SessionState, srvr_ctrl_session_state, MessageTypeSrvrCtrlSessionState;

    // Queue events
    QueueState, srvr_ctrl_queue_state, MessageTypeSrvrCtrlQueueState;
    QueueCleared, srvr_ctrl_queue_cleared, MessageTypeSrvrCtrlQueueCleared;
    QueueLoadTracks, srvr_ctrl_queue_tracks_loaded, MessageTypeSrvrCtrlQueueTracksLoaded;
    QueueTracksAdded, srvr_ctrl_queue_tracks_added, MessageTypeSrvrCtrlQueueTracksAdded;
    QueueTracksInserted, srvr_ctrl_queue_tracks_inserted, MessageTypeSrvrCtrlQueueTracksInserted;
    QueueTracksRemoved, srvr_ctrl_queue_tracks_removed, MessageTypeSrvrCtrlQueueTracksRemoved;
    QueueTracksReordered, srvr_ctrl_queue_tracks_reordered, MessageTypeSrvrCtrlQueueTracksReordered;
    QueueVersionChanged, srvr_ctrl_queue_version_changed, MessageTypeSrvrCtrlQueueVersionChanged;
    QueueErrorMessage, srvr_ctrl_queue_error_message, MessageTypeSrvrCtrlQueueErrorMessage;

    // Autoplay
    AutoplayModeSet, srvr_ctrl_autoplay_mode_set, MessageTypeSrvrCtrlAutoplayModeSet;
    AutoplayTracksLoaded, srvr_ctrl_autoplay_tracks_loaded, MessageTypeSrvrCtrlAutoplayTracksLoaded;

    // Mode changes
    LoopModeSet, srvr_ctrl_loop_mode_set, MessageTypeSrvrCtrlLoopModeSet;
    ShuffleModeSet, srvr_ctrl_shuffle_mode_set, MessageTypeSrvrCtrlShuffleModeSet;

    // Renderer presence
    ActiveRendererChanged, srvr_ctrl_active_renderer_changed, MessageTypeSrvrCtrlActiveRendererChanged;
    AddRenderer, srvr_ctrl_add_renderer, MessageTypeSrvrCtrlAddRenderer;
    UpdateRenderer, srvr_ctrl_update_renderer, MessageTypeSrvrCtrlUpdateRenderer;
    RemoveRenderer, srvr_ctrl_remove_renderer, MessageTypeSrvrCtrlRemoveRenderer;

    // Renderer state broadcasts
    RendererStateUpdated, srvr_ctrl_renderer_state_updated, MessageTypeSrvrCtrlRendererStateUpdated;
    VolumeChanged, srvr_ctrl_volume_changed, MessageTypeSrvrCtrlVolumeChanged;
    VolumeMuted, srvr_ctrl_volume_muted, MessageTypeSrvrCtrlVolumeMuted;
    MaxAudioQualityChanged, srvr_ctrl_max_audio_quality_changed, MessageTypeSrvrCtrlMaxAudioQualityChanged;
    FileAudioQualityChanged, srvr_ctrl_file_audio_quality_changed, MessageTypeSrvrCtrlFileAudioQualityChanged;
    DeviceAudioQualityChanged, srvr_ctrl_device_audio_quality_changed, MessageTypeSrvrCtrlDeviceAudioQualityChanged;
}
