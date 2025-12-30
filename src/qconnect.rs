//! High-level session management for Qobuz Connect.
//!
//! This module provides a stream-based event API:
//! - `SessionEvent` enum for all events from the session
//! - `Responder<T>` for sending required responses back to the server
//!
//! Events are received via an mpsc channel. Commands that require responses
//! include a `Responder` that must be used to send the response.

use std::pin::Pin;
use std::sync::Arc;

use futures::stream::Stream;
use futures::StreamExt;
use tokio::sync::{mpsc, oneshot, RwLock};
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};

use crate::config::{DeviceConfig, SessionInfo};
use crate::proto::qconnect::{
    BufferState, CtrlSrvrAskForQueueState, CtrlSrvrAskForRendererState, CtrlSrvrSetActiveRenderer,
    LoopMode, PlayingState, Position, QConnectMessage, QConnectMessageType, QueueRendererState,
    QueueVersion, RndrSrvrFileAudioQualityChanged, RndrSrvrMaxAudioQualityChanged,
    RndrSrvrStateUpdated, RndrSrvrVolumeChanged, RndrSrvrVolumeMuted,
};
use crate::connection::{Connection, ConnectionWriter};
use crate::Result;

// ============================================================================
// Handler Types
// ============================================================================

/// Playback state returned from handler methods.
///
/// This is sent to the server automatically after handling commands.
#[derive(Debug, Clone)]
pub struct PlaybackResponse {
    /// Current playback state (playing, paused, stopped).
    pub state: PlayingState,
    /// Buffer state (buffering, ready, empty).
    pub buffer_state: BufferState,
    /// Current position in milliseconds.
    pub position_ms: u32,
    /// Track duration in milliseconds (if known).
    pub duration_ms: Option<u32>,
    /// Current queue item ID (if playing from queue).
    pub queue_item_id: Option<i32>,
    /// Next queue item ID (for gapless playback).
    pub next_queue_item_id: Option<i32>,
}

/// Playback command from the server.
#[derive(Debug, Clone)]
pub struct PlaybackCommand {
    /// Requested playback state, or None if server didn't specify (seek-only command).
    pub state: Option<PlayingState>,
    /// Requested position (seek), if any.
    pub position_ms: Option<u32>,
    /// Current queue item ID to play (if specified).
    pub queue_item_id: Option<u64>,
}

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
    pub playback: PlaybackResponse,
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
// Session Commands (user -> server)
// ============================================================================

/// Commands that can be sent from the application to the Qobuz server.
///
/// These are used when the player initiates state changes (not in response
/// to server commands).
#[derive(Debug, Clone)]
pub enum SessionCommand {
    /// Report current playback state to server.
    ReportState(PlaybackResponse),
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
/// let mut session = manager.add_device(config).await?;
///
/// while let Some(event) = session.recv().await {
///     match event {
///         SessionEvent::PlaybackCommand { cmd, respond, .. } => {
///             let response = handle_command(cmd);
///             respond.send(response);
///         }
///         _ => {}
///     }
///
///     // Player initiates a pause
///     session.report_state(PlaybackResponse {
///         state: PlayingState::Paused,
///         ..current_state
///     }).await?;
/// }
/// ```
pub struct DeviceSession {
    events: mpsc::Receiver<SessionEvent>,
    command_tx: SharedCommandTx,
}

impl DeviceSession {
    /// Create a new device session handle.
    pub(crate) fn new(
        events: mpsc::Receiver<SessionEvent>,
        command_tx: SharedCommandTx,
    ) -> Self {
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
    /// Returns an error if not connected to the Qobuz server.
    pub async fn report_state(&self, state: PlaybackResponse) -> crate::Result<()> {
        self.send_command(SessionCommand::ReportState(state)).await
    }

    /// Report volume level to the server.
    pub async fn report_volume(&self, volume: u32) -> crate::Result<()> {
        self.send_command(SessionCommand::ReportVolume(volume)).await
    }

    /// Report mute state to the server.
    pub async fn report_muted(&self, muted: bool) -> crate::Result<()> {
        self.send_command(SessionCommand::ReportVolumeMuted(muted)).await
    }

    /// Report max audio quality capability to the server.
    pub async fn report_max_audio_quality(&self, quality: i32) -> crate::Result<()> {
        self.send_command(SessionCommand::ReportMaxAudioQuality(quality)).await
    }

    /// Report current file's audio quality (sample rate in Hz) to the server.
    pub async fn report_file_audio_quality(&self, sample_rate_hz: u32) -> crate::Result<()> {
        self.send_command(SessionCommand::ReportFileAudioQuality(sample_rate_hz)).await
    }
}

// ============================================================================
// Session Events (server -> user)
// ============================================================================

/// A track in the queue.
#[derive(Debug, Clone)]
pub struct QueueTrack {
    pub track_id: u64,
    pub queue_item_id: u64,
}

// ============================================================================
// Event Categories
// ============================================================================

/// Commands from the server that require a response via Responder.
///
/// These events must be responded to - call `respond.send(value)` to provide
/// the response. The session will hang if you don't respond.
pub enum CommandEvent {
    /// Server commands play/pause/seek. Must respond with `PlaybackResponse`.
    PlaybackCommand {
        renderer_id: u64,
        cmd: PlaybackCommand,
        respond: Responder<PlaybackResponse>,
    },

    /// Device became active renderer. Must respond with `ActivationState`.
    Activate {
        renderer_id: u64,
        respond: Responder<ActivationState>,
    },

    /// Heartbeat while playing. Respond with `Some(PlaybackResponse)` to send
    /// a position update, or `None` to skip.
    Heartbeat {
        renderer_id: u64,
        respond: Responder<Option<PlaybackResponse>>,
    },
}

/// Events about this device's state (no response needed).
pub enum DeviceEvent {
    /// Device was deactivated.
    Deactivated { renderer_id: u64 },

    /// Queue updated from server.
    QueueUpdated {
        tracks: Vec<QueueTrack>,
        version: (u64, i32),
    },

    /// Loop mode changed.
    LoopModeChanged { mode: LoopMode },

    /// Shuffle mode changed.
    ShuffleModeChanged { enabled: bool },

    /// Restore state from another renderer before becoming active.
    RestoreState {
        position_ms: u32,
        queue_index: Option<u32>,
    },
}

/// System-level connection events.
pub enum SystemEvent {
    /// WebSocket connected successfully.
    Connected,

    /// WebSocket disconnected.
    Disconnected {
        session_id: String,
        reason: Option<String>,
    },

    /// One of our devices was registered with the server.
    DeviceRegistered {
        device_uuid: [u8; 16],
        renderer_id: u64,
    },

    /// Session closed (WebSocket disconnected or error).
    SessionClosed { device_uuid: [u8; 16] },
}

/// Broadcasts from other renderers in the session.
///
/// These are informational updates about other devices in the same
/// Qobuz Connect session. No response is needed.
pub enum BroadcastEvent {
    /// Another renderer was added to the session.
    RendererAdded { renderer_id: u64, name: String },

    /// A renderer was removed from the session.
    RendererRemoved { renderer_id: u64 },

    /// The active renderer changed.
    ActiveRendererChanged { renderer_id: u64 },

    /// Renderer state broadcast (from any renderer).
    RendererStateUpdated {
        renderer_id: u64,
        state: PlayingState,
        position_ms: u32,
        duration_ms: u32,
        queue_index: u32,
    },

    /// Volume changed broadcast.
    VolumeChanged { renderer_id: u64, volume: u32 },

    /// Volume muted broadcast.
    VolumeMuted { renderer_id: u64, muted: bool },

    /// Max audio quality changed broadcast (capability level).
    MaxAudioQualityChanged { renderer_id: u64, quality: i32 },

    /// File audio quality changed broadcast.
    FileAudioQualityChanged {
        renderer_id: u64,
        sample_rate_hz: u32,
    },
}

// ============================================================================
// Unified SessionEvent
// ============================================================================

/// Events from a Qobuz Connect session.
///
/// Events are categorized by type:
/// - `Command`: Server commands requiring a response via `Responder`
/// - `Device`: State changes for this device (no response needed)
/// - `System`: Connection lifecycle events
/// - `Broadcast`: Updates from other renderers in the session
///
/// # Example
///
/// ```ignore
/// use qonductor::{SessionEvent, CommandEvent, DeviceEvent, SystemEvent};
///
/// while let Some(event) = session.recv().await {
///     match event {
///         // Commands - must respond
///         SessionEvent::Command(CommandEvent::PlaybackCommand { cmd, respond, .. }) => {
///             let response = my_player.handle_playback(cmd);
///             respond.send(response);
///         }
///         SessionEvent::Command(CommandEvent::Activate { respond, .. }) => {
///             respond.send(ActivationState { ... });
///         }
///
///         // Device events - no response needed
///         SessionEvent::Device(DeviceEvent::QueueUpdated { tracks, .. }) => {
///             my_player.queue = tracks;
///         }
///
///         // System events
///         SessionEvent::System(SystemEvent::Connected) => println!("Connected!"),
///
///         _ => {}
///     }
/// }
/// ```
pub enum SessionEvent {
    /// Server commands requiring a response.
    Command(CommandEvent),
    /// State changes for this device.
    Device(DeviceEvent),
    /// Connection lifecycle events.
    System(SystemEvent),
    /// Updates from other renderers.
    Broadcast(BroadcastEvent),
}

impl SessionEvent {
    /// Returns true if this is a command event requiring a response.
    pub fn is_command(&self) -> bool {
        matches!(self, Self::Command(_))
    }

    /// Returns true if this is a device state event.
    pub fn is_device(&self) -> bool {
        matches!(self, Self::Device(_))
    }

    /// Returns true if this is a system/connection event.
    pub fn is_system(&self) -> bool {
        matches!(self, Self::System(_))
    }

    /// Returns true if this is a broadcast from another renderer.
    pub fn is_broadcast(&self) -> bool {
        matches!(self, Self::Broadcast(_))
    }
}

/// Connect to Qobuz WebSocket and spawn a session runner task.
///
/// The session runs until the WebSocket closes or an error occurs.
/// Events are sent to the provided `event_tx` channel.
/// Commands are received from the `command_rx` channel.
pub(crate) async fn spawn_session(
    session_info: &SessionInfo,
    device_config: &DeviceConfig,
    event_tx: mpsc::Sender<SessionEvent>,
    command_rx: mpsc::Receiver<SessionCommand>,
) -> Result<()> {
    debug!(
        session_id = %session_info.session_id,
        device = %device_config.friendly_name,
        "Connecting session"
    );

    // Connect and set up the WebSocket
    let mut connection =
        Connection::connect(&session_info.ws_endpoint, &session_info.ws_jwt).await?;
    connection.subscribe_default().await?;
    connection
        .join_session(&device_config.device_uuid, &device_config.friendly_name)
        .await?;

    // Split into reader/writer
    let (reader, writer) = connection.split();
    let reader = Box::pin(reader.into_stream());

    let _ = event_tx.send(SessionEvent::System(SystemEvent::Connected)).await;

    // Create and spawn the runner
    let runner = SessionRunner {
        session_id: session_info.session_id.clone(),
        reader,
        writer,
        device_uuid: device_config.device_uuid,
        device_name: device_config.friendly_name.clone(),
        renderer_id: 0,
        is_active: false,
        event_tx,
        command_rx,
        state: SessionState::default(),
    };

    tokio::spawn(async move {
        runner.run().await;
    });

    Ok(())
}

// ============================================================================
// Session Runner (runs in spawned task)
// ============================================================================

/// Internal session state tracking.
#[derive(Default)]
struct SessionState {
    /// Session ID from server.
    #[allow(dead_code)] // May be used for future features
    session_id: Option<u64>,
    /// Session UUID (also used as queue UUID).
    session_uuid: Option<[u8; 16]>,
    /// Current queue version from server (major, minor).
    queue_version: Option<(u64, i32)>,
}

/// Heartbeat interval (matches C++ reference implementation).
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

/// The session runner that handles WebSocket communication.
///
/// This runs in a spawned task and processes WebSocket messages,
/// sends events to the user, receives commands from the user,
/// and waits for responses via Responder.
struct SessionRunner {
    session_id: String,
    reader: Pin<Box<dyn Stream<Item = Result<QConnectMessage>> + Send>>,
    writer: ConnectionWriter,
    device_uuid: [u8; 16],
    device_name: String,
    renderer_id: u64,
    is_active: bool,
    event_tx: mpsc::Sender<SessionEvent>,
    command_rx: mpsc::Receiver<SessionCommand>,
    state: SessionState,
}

impl SessionRunner {
    /// Run the session event loop.
    async fn run(mut self) {
        info!(session_id = %self.session_id, "Session runner starting");

        let mut heartbeat = interval(HEARTBEAT_INTERVAL);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                // Handle WebSocket messages
                msg = self.reader.next() => {
                    match msg {
                        Some(Ok(m)) => {
                            match self.handle_qconnect_message(m).await {
                                Ok(true) => break, // Handler requested exit
                                Ok(false) => {}
                                Err(e) => warn!(error = %e, "Error handling message"),
                            }
                        }
                        Some(Err(e)) => {
                            warn!(error = %e, "WebSocket error");
                            let _ = self.event_tx.send(SessionEvent::System(SystemEvent::Disconnected {
                                session_id: self.session_id.clone(),
                                reason: Some(e.to_string()),
                            })).await;
                            break;
                        }
                        None => {
                            info!("WebSocket closed");
                            let _ = self.event_tx.send(SessionEvent::System(SystemEvent::Disconnected {
                                session_id: self.session_id.clone(),
                                reason: None,
                            })).await;
                            break;
                        }
                    }
                }

                // Heartbeat timer - send event and wait for response
                _ = heartbeat.tick() => {
                    if self.is_active && self.renderer_id != 0 {
                        let (tx, rx) = oneshot::channel();
                        let _ = self.event_tx.send(SessionEvent::Command(CommandEvent::Heartbeat {
                            renderer_id: self.renderer_id,
                            respond: Responder::new(tx),
                        })).await;
                        if let Ok(Some(resp)) = rx.await
                            && let Err(e) = self.send_playback_response(&resp).await
                        {
                            warn!(error = %e, "Failed to send heartbeat");
                        }
                    }
                }

                // Handle commands from user
                cmd = self.command_rx.recv() => {
                    match cmd {
                        Some(command) => {
                            if let Err(e) = self.handle_command(command).await {
                                warn!(error = %e, "Failed to handle command");
                            }
                        }
                        None => {
                            // Command channel closed, user dropped DeviceSession
                            debug!("Command channel closed");
                        }
                    }
                }
            }
        }

        // Notify manager that session is done
        let _ = self.event_tx.send(SessionEvent::System(SystemEvent::SessionClosed {
            device_uuid: self.device_uuid,
        })).await;
        info!(session_id = %self.session_id, "Session runner stopped");
    }

    /// Handle a command from the user.
    async fn handle_command(&mut self, command: SessionCommand) -> Result<()> {
        match command {
            SessionCommand::ReportState(resp) => {
                self.send_playback_response(&resp).await
            }
            SessionCommand::ReportVolume(volume) => {
                self.do_report_volume(volume).await
            }
            SessionCommand::ReportVolumeMuted(muted) => {
                self.do_report_volume_muted(muted).await
            }
            SessionCommand::ReportMaxAudioQuality(quality) => {
                self.do_report_max_audio_quality(quality).await
            }
            SessionCommand::ReportFileAudioQuality(sample_rate_hz) => {
                self.do_report_file_audio_quality(sample_rate_hz).await
            }
        }
    }

    /// Report volume to server.
    async fn do_report_volume(&mut self, volume: u32) -> Result<()> {
        let msg = QConnectMessage {
            message_type: Some(QConnectMessageType::MessageTypeRndrSrvrVolumeChanged as i32),
            rndr_srvr_volume_changed: Some(RndrSrvrVolumeChanged {
                volume: Some(volume),
            }),
            ..Default::default()
        };
        self.writer.send(msg).await
    }

    /// Report volume muted state to server.
    /// Protocol uses None for not muted, Some(true) for muted.
    async fn do_report_volume_muted(&mut self, muted: bool) -> Result<()> {
        let msg = QConnectMessage {
            message_type: Some(QConnectMessageType::MessageTypeRndrSrvrVolumeMuted as i32),
            rndr_srvr_volume_muted: Some(RndrSrvrVolumeMuted {
                value: if muted { Some(true) } else { None },
            }),
            ..Default::default()
        };
        self.writer.send(msg).await
    }

    /// Report max audio quality capability to server.
    /// Uses capability level (1-4), not format IDs.
    async fn do_report_max_audio_quality(&mut self, quality: i32) -> Result<()> {
        let msg = QConnectMessage {
            message_type: Some(
                QConnectMessageType::MessageTypeRndrSrvrMaxAudioQualityChanged as i32,
            ),
            rndr_srvr_max_audio_quality_changed: Some(RndrSrvrMaxAudioQualityChanged {
                value: Some(quality),
            }),
            ..Default::default()
        };
        self.writer.send(msg).await
    }

    /// Report actual file audio quality (sample rate in Hz) to server.
    async fn do_report_file_audio_quality(&mut self, sample_rate_hz: u32) -> Result<()> {
        let msg = QConnectMessage {
            message_type: Some(
                QConnectMessageType::MessageTypeRndrSrvrFileAudioQualityChanged as i32,
            ),
            rndr_srvr_file_audio_quality_changed: Some(RndrSrvrFileAudioQualityChanged {
                value: Some(sample_rate_hz as i32),
            }),
            ..Default::default()
        };
        self.writer.send(msg).await
    }

    /// Request queue state from server.
    async fn do_request_queue_state(&mut self) -> Result<()> {
        let queue_uuid = self.state.session_uuid.unwrap_or(self.device_uuid);
        let msg = QConnectMessage {
            message_type: Some(QConnectMessageType::MessageTypeCtrlSrvrAskForQueueState as i32),
            ctrl_srvr_ask_for_queue_state: Some(CtrlSrvrAskForQueueState {
                queue_version: None,
                queue_uuid: Some(queue_uuid.to_vec()),
            }),
            ..Default::default()
        };
        self.writer.send(msg).await
    }

    /// Request renderer state from server.
    async fn do_request_renderer_state(&mut self) -> Result<()> {
        let session_id = self.state.session_id.unwrap_or(0);
        let msg = QConnectMessage {
            message_type: Some(QConnectMessageType::MessageTypeCtrlSrvrAskForRendererState as i32),
            ctrl_srvr_ask_for_renderer_state: Some(CtrlSrvrAskForRendererState {
                session_id: Some(session_id),
            }),
            ..Default::default()
        };
        self.writer.send(msg).await
    }

    /// Send a PlaybackResponse to the server.
    async fn send_playback_response(&mut self, resp: &PlaybackResponse) -> Result<()> {
        let queue_version = self.state.queue_version;

        let msg = QConnectMessage {
            message_type: Some(QConnectMessageType::MessageTypeRndrSrvrStateUpdated as i32),
            rndr_srvr_state_updated: Some(RndrSrvrStateUpdated {
                state: Some(QueueRendererState {
                    playing_state: Some(resp.state.into()),
                    buffer_state: Some(resp.buffer_state.into()),
                    current_position: Some(Position {
                        timestamp: Some(now_ms()),
                        value: Some(resp.position_ms),
                    }),
                    duration: resp.duration_ms,
                    queue_version: queue_version.map(|(major, minor)| QueueVersion {
                        major: Some(major),
                        minor: Some(minor),
                    }),
                    current_queue_item_id: resp.queue_item_id,
                    next_queue_item_id: resp.next_queue_item_id,
                }),
            }),
            ..Default::default()
        };
        self.writer.send(msg).await
    }

    /// Send the activation handshake (muted, volume, max quality).
    ///
    /// Note: Playback state is NOT sent here. We trigger a heartbeat immediately
    /// after activation to report state, allowing the handler to use any restored
    /// position from the previous renderer.
    async fn send_activation_handshake(&mut self, state: &ActivationState) -> Result<()> {
        // Send muted state
        self.do_report_volume_muted(state.muted).await?;

        // Send volume
        self.do_report_volume(state.volume).await?;

        // Send max quality
        self.do_report_max_audio_quality(state.max_quality).await?;

        Ok(())
    }

    /// Handle a single QConnect message.
    /// Returns Ok(true) if the session should exit.
    async fn handle_qconnect_message(&mut self, msg: QConnectMessage) -> Result<bool> {
        let msg_type = msg.message_type.unwrap_or(0);

        match msg_type {
            // SrvrCtrlAddRenderer (83)
            t if t == QConnectMessageType::MessageTypeSrvrCtrlAddRenderer as i32 => {
                if let Some(add) = msg.srvr_ctrl_add_renderer
                    && let Some(renderer) = &add.renderer
                {
                    let renderer_uuid: Option<[u8; 16]> = renderer
                        .device_uuid
                        .as_ref()
                        .and_then(|u| u.as_slice().try_into().ok());

                    let name = renderer.friendly_name.clone().unwrap_or_default();
                    let rid = add.renderer_id.unwrap_or(0);

                    // Check if this is our device
                    if renderer_uuid == Some(self.device_uuid) {
                        self.renderer_id = rid;
                        info!(renderer_id = rid, name = %self.device_name, "Our device registered");

                        let _ = self
                            .event_tx
                            .send(SessionEvent::System(SystemEvent::DeviceRegistered {
                                device_uuid: self.device_uuid,
                                renderer_id: rid,
                            }))
                            .await;

                        // Declare as active renderer
                        let msg = QConnectMessage {
                            message_type: Some(
                                QConnectMessageType::MessageTypeCtrlSrvrSetActiveRenderer as i32,
                            ),
                            ctrl_srvr_set_active_renderer: Some(CtrlSrvrSetActiveRenderer {
                                renderer_id: Some(rid as i32),
                            }),
                            ..Default::default()
                        };
                        self.writer.send(msg).await?;
                    } else {
                        let _ = self
                            .event_tx
                            .send(SessionEvent::Broadcast(BroadcastEvent::RendererAdded {
                                renderer_id: rid,
                                name,
                            }))
                            .await;
                    }
                }
            }

            // SrvrCtrlRemoveRenderer (85)
            t if t == QConnectMessageType::MessageTypeSrvrCtrlRemoveRenderer as i32 => {
                if let Some(rem) = msg.srvr_ctrl_remove_renderer {
                    let rid = rem.renderer_id.unwrap_or(0);

                    // Check if it's ours and mark as unregistered
                    if self.renderer_id == rid {
                        self.renderer_id = 0;
                    }

                    let _ = self
                        .event_tx
                        .send(SessionEvent::Broadcast(BroadcastEvent::RendererRemoved { renderer_id: rid }))
                        .await;
                }
            }

            // SrvrCtrlSessionState (81)
            t if t == QConnectMessageType::MessageTypeSrvrCtrlSessionState as i32 => {
                if let Some(ss) = msg.srvr_ctrl_session_state {
                    let sid = ss.session_id.unwrap_or(0);

                    // Extract queue_version - this comes before activation so we have it ready
                    if let Some(qv) = ss.queue_version {
                        let version = (qv.major.unwrap_or(0), qv.minor.unwrap_or(0));
                        self.state.queue_version = Some(version);
                    }

                    self.state.session_id = Some(sid);

                    if let Some(uuid_bytes) = ss.session_uuid
                        && uuid_bytes.len() == 16
                    {
                        let mut uuid = [0u8; 16];
                        uuid.copy_from_slice(&uuid_bytes);
                        self.state.session_uuid = Some(uuid);
                    }

                    // Request renderer state to get current playback position
                    // This allows us to restore position when taking over from another renderer
                    if let Err(e) = self.do_request_renderer_state().await {
                        warn!(error = %e, "Failed to request renderer state");
                    }
                }
            }

            // SrvrCtrlActiveRendererChanged (86)
            t if t == QConnectMessageType::MessageTypeSrvrCtrlActiveRendererChanged as i32 => {
                if let Some(arc) = msg.srvr_ctrl_active_renderer_changed {
                    let rid = arc.renderer_id.unwrap_or(0);

                    let _ = self
                        .event_tx
                        .send(SessionEvent::Broadcast(BroadcastEvent::ActiveRendererChanged { renderer_id: rid }))
                        .await;
                }
            }

            // SrvrCtrlQueueState (90)
            t if t == QConnectMessageType::MessageTypeSrvrCtrlQueueState as i32 => {
                if let Some(qs) = msg.srvr_ctrl_queue_state {
                    let version = qs
                        .queue_version
                        .map(|v| (v.major.unwrap_or(0), v.minor.unwrap_or(0)))
                        .unwrap_or((0, 0));

                    // Store queue version for use in responses
                    self.state.queue_version = Some(version);

                    let tracks: Vec<QueueTrack> = qs
                        .tracks
                        .iter()
                        .filter_map(|t| {
                            Some(QueueTrack {
                                track_id: t.track_id? as u64,
                                queue_item_id: t.queue_item_id?,
                            })
                        })
                        .collect();

                    // Send queue update event
                    let _ = self.event_tx.send(SessionEvent::Device(DeviceEvent::QueueUpdated {
                        tracks,
                        version,
                    })).await;
                }
            }

            // SrvrRndrSetState (41) - Server telling us to change playback state
            t if t == QConnectMessageType::MessageTypeSrvrRndrSetState as i32 => {
                if let Some(ss) = msg.srvr_rndr_set_state {
                    // Extract queue info (for future use)
                    let server_queue_version = ss
                        .queue_version
                        .as_ref()
                        .map(|qv| (qv.major.unwrap_or(0), qv.minor.unwrap_or(0)));
                    let next_queue_item_id =
                        ss.next_queue_item.as_ref().and_then(|q| q.queue_item_id);
                    let _ = (server_queue_version, next_queue_item_id); // TODO: use these

                    // Only respond if there's an actual state change request
                    // Messages with just queue_version/next_queue_item are informational
                    if ss.playing_state.is_some() || ss.current_position.is_some() {
                        let state = ss
                            .playing_state
                            .and_then(|i| PlayingState::try_from(i).ok());
                        let position_ms = ss.current_position;
                        let queue_item_id =
                            ss.current_queue_item.as_ref().and_then(|q| q.queue_item_id);

                        // Send event and wait for response
                        let cmd = PlaybackCommand {
                            state,
                            position_ms,
                            queue_item_id,
                        };
                        let (tx, rx) = oneshot::channel();
                        let _ = self.event_tx.send(SessionEvent::Command(CommandEvent::PlaybackCommand {
                            renderer_id: self.renderer_id,
                            cmd,
                            respond: Responder::new(tx),
                        })).await;
                        if let Ok(response) = rx.await
                            && let Err(e) = self.send_playback_response(&response).await
                        {
                            warn!(error = %e, "Failed to send playback response");
                        }
                    }
                }
            }

            // SrvrRndrSetActive (43) - Server telling us we're active/inactive
            t if t == QConnectMessageType::MessageTypeSrvrRndrSetActive as i32 => {
                if let Some(sa) = msg.srvr_rndr_set_active {
                    let active = sa.active.unwrap_or(false);

                    if active {
                        info!(renderer_id = self.renderer_id, "Server set us active");
                        self.is_active = true;

                        // Send activation event and wait for response
                        // Note: We do NOT send playback state here. The server will send us
                        // SrvrRndrSetState with the current position, and we respond to that.
                        // This matches the C++ implementation behavior.
                        let (tx, rx) = oneshot::channel();
                        let _ = self.event_tx.send(SessionEvent::Command(CommandEvent::Activate {
                            renderer_id: self.renderer_id,
                            respond: Responder::new(tx),
                        })).await;
                        if let Ok(activation_state) = rx.await
                            && let Err(e) = self.send_activation_handshake(&activation_state).await
                        {
                            warn!(error = %e, "Failed to send activation handshake");
                        }

                        // Request queue state after activation
                        if let Err(e) = self.do_request_queue_state().await {
                            warn!(error = %e, "Failed to request queue state");
                        }
                    } else {
                        info!(
                            renderer_id = self.renderer_id,
                            "Server set us inactive, disconnecting"
                        );
                        self.is_active = false;

                        // Send deactivated event
                        let _ = self.event_tx.send(SessionEvent::Device(DeviceEvent::Deactivated {
                            renderer_id: self.renderer_id,
                        })).await;

                        // Close WebSocket and signal run loop to exit
                        let _ = self.writer.close().await;
                        let _ = self
                            .event_tx
                            .send(SessionEvent::System(SystemEvent::Disconnected {
                                session_id: self.session_id.clone(),
                                reason: Some("Server set inactive".to_string()),
                            }))
                            .await;
                        return Ok(true);
                    }
                }
            }

            // SrvrCtrlVolumeChanged (87) - Broadcast
            t if t == QConnectMessageType::MessageTypeSrvrCtrlVolumeChanged as i32 => {
                if let Some(vc) = msg.srvr_ctrl_volume_changed {
                    let rid = vc.renderer_id.unwrap_or(0);
                    let volume = vc.volume.unwrap_or(0);

                    let _ = self
                        .event_tx
                        .send(SessionEvent::Broadcast(BroadcastEvent::VolumeChanged {
                            renderer_id: rid,
                            volume,
                        }))
                        .await;
                }
            }

            // SrvrCtrlShuffleModeSet (96)
            t if t == QConnectMessageType::MessageTypeSrvrCtrlShuffleModeSet as i32 => {
                if let Some(sm) = msg.srvr_ctrl_shuffle_mode_set {
                    let enabled = sm.shuffle_on.unwrap_or(false);

                    // Send shuffle mode event
                    let _ = self.event_tx.send(SessionEvent::Device(DeviceEvent::ShuffleModeChanged {
                        enabled,
                    })).await;
                }
            }

            // SrvrCtrlLoopModeSet (97)
            t if t == QConnectMessageType::MessageTypeSrvrCtrlLoopModeSet as i32 => {
                if let Some(lm) = msg.srvr_ctrl_loop_mode_set {
                    let mode = lm
                        .mode
                        .and_then(|i| LoopMode::try_from(i).ok())
                        .unwrap_or_default();

                    // Send loop mode event
                    let _ = self.event_tx.send(SessionEvent::Device(DeviceEvent::LoopModeChanged {
                        mode,
                    })).await;
                }
            }

            // SrvrCtrlVolumeMuted (98) - Broadcast
            t if t == QConnectMessageType::MessageTypeSrvrCtrlVolumeMuted as i32 => {
                if let Some(vm) = msg.srvr_ctrl_volume_muted {
                    let rid = vm.renderer_id.unwrap_or(0);
                    let muted = vm.value.unwrap_or(false);

                    let _ = self
                        .event_tx
                        .send(SessionEvent::Broadcast(BroadcastEvent::VolumeMuted {
                            renderer_id: rid,
                            muted,
                        }))
                        .await;
                }
            }

            // SrvrCtrlMaxAudioQualityChanged (99) - Broadcast
            t if t == QConnectMessageType::MessageTypeSrvrCtrlMaxAudioQualityChanged as i32 => {
                if let Some(mq) = msg.srvr_ctrl_max_audio_quality_changed {
                    let quality = mq.max_audio_quality.unwrap_or(0);

                    // This broadcast doesn't include renderer_id; applies to active renderer
                    let _ = self
                        .event_tx
                        .send(SessionEvent::Broadcast(BroadcastEvent::MaxAudioQualityChanged {
                            renderer_id: self.renderer_id,
                            quality,
                        }))
                        .await;
                }
            }

            // SrvrCtrlFileAudioQualityChanged (100) - Broadcast
            t if t == QConnectMessageType::MessageTypeSrvrCtrlFileAudioQualityChanged as i32 => {
                if let Some(fq) = msg.srvr_ctrl_file_audio_quality_changed {
                    // file_audio_quality is the sample rate in Hz (e.g., 44100, 96000)
                    let sample_rate_hz = fq.file_audio_quality.unwrap_or(0) as u32;

                    // This broadcast doesn't include renderer_id; applies to active renderer
                    let _ = self
                        .event_tx
                        .send(SessionEvent::Broadcast(BroadcastEvent::FileAudioQualityChanged {
                            renderer_id: self.renderer_id,
                            sample_rate_hz,
                        }))
                        .await;
                }
            }

            // SrvrCtrlRendererStateUpdated (82)
            t if t == QConnectMessageType::MessageTypeSrvrCtrlRendererStateUpdated as i32 => {
                if let Some(rsu) = msg.srvr_ctrl_renderer_state_updated {
                    let rid = rsu.renderer_id.unwrap_or(0);

                    if let Some(state) = rsu.state {
                        let play_state = state
                            .playing_state
                            .and_then(|i| PlayingState::try_from(i).ok())
                            .unwrap_or(PlayingState::Stopped);
                        let position_ms = state.current_position.and_then(|p| p.value).unwrap_or(0);
                        let duration_ms = state.duration.unwrap_or(0);
                        let queue_index = state.current_queue_index.unwrap_or(0);

                        // If this is from another renderer and we're not active yet,
                        // restore the position so we can continue from where they left off
                        if rid != self.renderer_id && !self.is_active {
                            let queue_idx = if queue_index > 0 {
                                Some(queue_index)
                            } else {
                                None
                            };
                            let _ = self.event_tx.send(SessionEvent::Device(DeviceEvent::RestoreState {
                                position_ms,
                                queue_index: queue_idx,
                            })).await;
                        }

                        let _ = self
                            .event_tx
                            .send(SessionEvent::Broadcast(BroadcastEvent::RendererStateUpdated {
                                renderer_id: rid,
                                state: play_state,
                                position_ms,
                                duration_ms,
                                queue_index,
                            }))
                            .await;
                    }
                }
            }

            _ => {}
        }

        Ok(false)
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
