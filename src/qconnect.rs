//! High-level session management for Qobuz Connect.
//!
//! This module provides:
//! - `RendererHandler` trait for handling server commands with automatic responses
//! - `RendererBroadcast` enum for informational events (no response needed)
//!
//! The handler trait ensures protocol compliance by requiring return values
//! that are automatically sent to the server.

use std::pin::Pin;
use std::sync::{Arc, Mutex};

use futures::stream::Stream;
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Duration, interval};
use tracing::{debug, info, warn};

use crate::discovery::{AudioQuality, DeviceConfig, SessionInfo};
use crate::proto::qconnect::{
    BufferState, CtrlSrvrAskForQueueState, CtrlSrvrAskForRendererState,
    CtrlSrvrSetActiveRenderer, LoopMode, PlayingState, Position, QConnectMessage,
    QConnectMessageType, QueueRendererState, QueueVersion, RndrSrvrFileAudioQualityChanged,
    RndrSrvrMaxAudioQualityChanged, RndrSrvrStateUpdated, RndrSrvrVolumeChanged,
    RndrSrvrVolumeMuted,
};
use crate::transport::{Transport, TransportWriter};
use crate::{Error, Result};

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
    /// Requested playback state.
    pub state: PlayingState,
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
    /// Maximum audio quality this renderer supports.
    pub max_quality: AudioQuality,
    /// Current playback state.
    pub playback: PlaybackResponse,
}

// ============================================================================
// RendererHandler Trait
// ============================================================================

/// Handler for renderer commands from the Qobuz server.
///
/// Implement this trait to handle playback commands. Return values are
/// automatically sent to the server, ensuring protocol compliance.
///
/// # Example
///
/// ```ignore
/// struct MyPlayer {
///     state: PlayingState,
///     position_ms: u32,
///     volume: u32,
/// }
///
/// impl RendererHandler for MyPlayer {
///     fn on_playback_command(&mut self, _rid: u64, cmd: PlaybackCommand) -> PlaybackResponse {
///         self.state = cmd.state;
///         if let Some(pos) = cmd.position_ms {
///             self.position_ms = pos;
///         }
///         // Return value is automatically sent to server
///         PlaybackResponse {
///             state: self.state,
///             buffer_state: BufferState::Ok,
///             position_ms: self.position_ms,
///             duration_ms: Some(180_000),
///             queue_item_id: Some(1),
///         }
///     }
///     // ... other methods
/// }
/// ```
pub trait RendererHandler: Send + 'static {
    /// Server commands play/pause/seek.
    ///
    /// Return the new playback state to report back to the server.
    fn on_playback_command(&mut self, renderer_id: u64, cmd: PlaybackCommand) -> PlaybackResponse;

    /// Server commands volume change.
    ///
    /// Return the actual volume to report back (may differ from requested).
    fn on_volume_command(&mut self, renderer_id: u64, volume: u32) -> u32;

    /// Device became active renderer.
    ///
    /// Return the initial state for the activation handshake.
    fn on_activate(&mut self, renderer_id: u64) -> ActivationState;

    /// Device was deactivated.
    fn on_deactivate(&mut self, renderer_id: u64);

    /// Queue updated from server.
    fn on_queue_update(&mut self, tracks: Vec<QueueTrack>, version: (u64, i32));

    /// Loop mode changed.
    fn on_loop_mode(&mut self, mode: LoopMode);

    /// Shuffle mode changed.
    fn on_shuffle_mode(&mut self, enabled: bool);

    /// Called every ~10 seconds for heartbeat while playing.
    ///
    /// Return `Some(state)` to send a position update, or `None` to skip.
    fn on_heartbeat(&mut self, renderer_id: u64) -> Option<PlaybackResponse>;
}

// ============================================================================
// Broadcast Events (informational only - no response needed)
// ============================================================================

/// Informational events from a Qobuz Connect session.
///
/// These are broadcast events that do NOT require a response.
/// Handle them for UI updates or logging, but the library handles
/// all protocol responses automatically via [`RendererHandler`].
#[derive(Debug, Clone)]
pub enum RendererBroadcast {
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
    VolumeBroadcast { renderer_id: u64, volume: u32 },

    /// Volume muted broadcast.
    VolumeMutedBroadcast { renderer_id: u64, muted: bool },

    /// Max audio quality changed broadcast.
    MaxAudioQualityBroadcast {
        renderer_id: u64,
        quality: AudioQuality,
    },

    /// File audio quality changed broadcast.
    /// Sample rate is in Hz (e.g., 44100, 96000, 192000).
    FileAudioQualityBroadcast {
        renderer_id: u64,
        sample_rate_hz: u32,
    },
}


/// A track in the queue.
#[derive(Debug, Clone)]
pub struct QueueTrack {
    pub track_id: u64,
    pub queue_item_id: u64,
}

// ============================================================================
// Actor Pattern: Commands and Handle
// ============================================================================

/// Commands sent from SessionHandle to SessionRunner.
///
/// Note: Report commands are no longer needed - the handler's return values
/// are automatically sent by the SessionRunner.
pub(crate) enum SessionCommand {
    /// Request current queue state from the server.
    RequestQueueState,
    /// Request current renderer state from the server.
    RequestRendererState,
    /// Shutdown the session.
    #[allow(dead_code)] // Part of the API
    Shutdown,
    /// Disconnect the session (e.g., when we lose active status).
    Disconnect,
}

/// Type alias for the shared handler.
pub(crate) type SharedHandler = Arc<Mutex<dyn RendererHandler>>;

/// Handle to a running session.
///
/// This is a lightweight handle that communicates with the session runner
/// via channels. The actual WebSocket handling runs in a spawned task.
pub(crate) struct SessionHandle {
    #[allow(dead_code)] // Part of the API
    session_id: String,
    command_tx: mpsc::Sender<SessionCommand>,
    #[allow(dead_code)] // Kept alive to keep task running; used by shutdown()
    task: JoinHandle<()>,
}

impl SessionHandle {
    /// Connect to Qobuz WebSocket and spawn the session runner.
    pub async fn connect(
        session_info: &SessionInfo,
        device_config: &DeviceConfig,
        handler: SharedHandler,
        broadcast_tx: mpsc::Sender<RendererBroadcast>,
        on_disconnect: impl Fn() + Send + 'static,
    ) -> Result<Self> {
        debug!(
            session_id = %session_info.session_id,
            device = %device_config.friendly_name,
            "Connecting session"
        );

        // Connect and set up the WebSocket
        let mut transport =
            Transport::connect(&session_info.ws_endpoint, &session_info.ws_jwt).await?;
        transport.subscribe_default().await?;
        transport
            .join_session(&device_config.device_uuid, &device_config.friendly_name)
            .await?;

        // Split into reader/writer
        let (reader, writer) = transport.split();
        let reader = Box::pin(reader.into_stream());

        let _ = broadcast_tx.send(RendererBroadcast::Connected).await;

        // Create command channel
        let (command_tx, command_rx) = mpsc::channel(16);

        // Create and spawn the runner
        let runner = SessionRunner {
            session_id: session_info.session_id.clone(),
            reader,
            writer,
            device_uuid: device_config.device_uuid,
            device_name: device_config.friendly_name.clone(),
            renderer_id: 0,
            is_active: false,
            handler,
            broadcast_tx,
            command_rx,
            state: SessionState::default(),
            on_disconnect: Box::new(on_disconnect),
        };

        let task = tokio::spawn(async move {
            runner.run().await;
        });

        Ok(Self {
            session_id: session_info.session_id.clone(),
            command_tx,
            task,
        })
    }

    /// Get the session ID.
    #[allow(dead_code)] // Part of the API
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Request current queue state from the server.
    ///
    /// The server will respond, triggering a call to `handler.on_queue_update()`.
    pub async fn request_queue_state(&self) -> Result<()> {
        self.command_tx
            .send(SessionCommand::RequestQueueState)
            .await
            .map_err(|_| Error::Protocol("Session closed".to_string()))
    }

    /// Request current renderer state from the server.
    ///
    /// The server will respond with a `RendererStateUpdated` broadcast.
    pub async fn request_renderer_state(&self) -> Result<()> {
        self.command_tx
            .send(SessionCommand::RequestRendererState)
            .await
            .map_err(|_| Error::Protocol("Session closed".to_string()))
    }

    /// Disconnect the session.
    #[allow(dead_code)] // Part of the API
    pub async fn disconnect(&self) -> Result<()> {
        self.command_tx
            .send(SessionCommand::Disconnect)
            .await
            .map_err(|_| Error::Protocol("Session already closed".to_string()))
    }

    /// Shutdown the session.
    #[allow(dead_code)] // Part of the API
    pub async fn shutdown(self) {
        let _ = self.command_tx.send(SessionCommand::Shutdown).await;
        let _ = self.task.await;
    }
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
/// calls the handler for commands, and sends responses automatically.
struct SessionRunner {
    session_id: String,
    reader: Pin<Box<dyn Stream<Item = Result<QConnectMessage>> + Send>>,
    writer: TransportWriter,
    device_uuid: [u8; 16],
    device_name: String,
    renderer_id: u64,
    is_active: bool,
    handler: SharedHandler,
    broadcast_tx: mpsc::Sender<RendererBroadcast>,
    command_rx: mpsc::Receiver<SessionCommand>,
    state: SessionState,
    on_disconnect: Box<dyn Fn() + Send>,
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
                            let _ = self.broadcast_tx.send(RendererBroadcast::Disconnected {
                                session_id: self.session_id.clone(),
                                reason: Some(e.to_string()),
                            }).await;
                            break;
                        }
                        None => {
                            info!("WebSocket closed");
                            let _ = self.broadcast_tx.send(RendererBroadcast::Disconnected {
                                session_id: self.session_id.clone(),
                                reason: None,
                            }).await;
                            break;
                        }
                    }
                }

                // Handle commands from the handle
                cmd = self.command_rx.recv() => {
                    match cmd {
                        Some(SessionCommand::RequestQueueState) => {
                            if let Err(e) = self.do_request_queue_state().await {
                                warn!(error = %e, "Failed to request queue state");
                            }
                        }
                        Some(SessionCommand::RequestRendererState) => {
                            if let Err(e) = self.do_request_renderer_state().await {
                                warn!(error = %e, "Failed to request renderer state");
                            }
                        }
                        Some(SessionCommand::Disconnect) => {
                            info!("Disconnect command received");
                            let _ = self.writer.close().await;
                            let _ = self.broadcast_tx.send(RendererBroadcast::Disconnected {
                                session_id: self.session_id.clone(),
                                reason: Some("Disconnect requested".to_string()),
                            }).await;
                            break;
                        }
                        Some(SessionCommand::Shutdown) | None => {
                            info!("Session shutdown requested");
                            break;
                        }
                    }
                }

                // Heartbeat timer - call handler for state update
                _ = heartbeat.tick() => {
                    if self.is_active && self.renderer_id != 0 {
                        let response = {
                            let mut handler = self.handler.lock().unwrap();
                            handler.on_heartbeat(self.renderer_id)
                        };
                        if let Some(resp) = response
                            && let Err(e) = self.send_playback_response(&resp).await
                        {
                            warn!(error = %e, "Failed to send heartbeat");
                        }
                    }
                }
            }
        }

        // Notify manager that session is done
        (self.on_disconnect)();
        info!(session_id = %self.session_id, "Session runner stopped");
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
    async fn do_report_volume_muted(&mut self, muted: bool) -> Result<()> {
        let msg = QConnectMessage {
            message_type: Some(QConnectMessageType::MessageTypeRndrSrvrVolumeMuted as i32),
            rndr_srvr_volume_muted: Some(RndrSrvrVolumeMuted { value: Some(muted) }),
            ..Default::default()
        };
        self.writer.send(msg).await
    }

    /// Report max audio quality capability to server.
    async fn do_report_max_audio_quality(&mut self, quality: AudioQuality) -> Result<()> {
        let msg = QConnectMessage {
            message_type: Some(
                QConnectMessageType::MessageTypeRndrSrvrMaxAudioQualityChanged as i32,
            ),
            rndr_srvr_max_audio_quality_changed: Some(RndrSrvrMaxAudioQualityChanged {
                value: Some(quality.into()),
            }),
            ..Default::default()
        };
        self.writer.send(msg).await
    }

    /// Report actual file audio quality (sample rate in Hz) to server.
    #[allow(dead_code)] // May be used when track playback starts
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

    /// Send the activation handshake (muted, volume, max quality, playback state).
    async fn send_activation_handshake(&mut self, state: &ActivationState) -> Result<()> {
        // Send muted state
        self.do_report_volume_muted(state.muted).await?;

        // Send volume
        self.do_report_volume(state.volume).await?;

        // Send max quality
        self.do_report_max_audio_quality(state.max_quality).await?;

        // Send playback state
        self.send_playback_response(&state.playback).await?;

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
                            .broadcast_tx
                            .send(RendererBroadcast::DeviceRegistered {
                                device_uuid: self.device_uuid,
                                renderer_id: rid,
                            })
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
                            .broadcast_tx
                            .send(RendererBroadcast::RendererAdded {
                                renderer_id: rid,
                                name,
                            })
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
                        .broadcast_tx
                        .send(RendererBroadcast::RendererRemoved { renderer_id: rid })
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
                }
            }

            // SrvrCtrlActiveRendererChanged (86)
            t if t == QConnectMessageType::MessageTypeSrvrCtrlActiveRendererChanged as i32 => {
                if let Some(arc) = msg.srvr_ctrl_active_renderer_changed {
                    let rid = arc.renderer_id.unwrap_or(0);

                    let _ = self
                        .broadcast_tx
                        .send(RendererBroadcast::ActiveRendererChanged { renderer_id: rid })
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

                    // Call handler with queue update
                    {
                        let mut handler = self.handler.lock().unwrap();
                        handler.on_queue_update(tracks, version);
                    }
                }
            }

            // SrvrRndrSetState (41) - Server telling us to change playback state
            t if t == QConnectMessageType::MessageTypeSrvrRndrSetState as i32 => {
                if let Some(ss) = msg.srvr_rndr_set_state {
                    let state = ss
                        .playing_state
                        .and_then(|i| PlayingState::try_from(i).ok())
                        .unwrap_or(PlayingState::Stopped);
                    let position_ms = ss.current_position;
                    let queue_item_id =
                        ss.current_queue_item.as_ref().and_then(|q| q.queue_item_id);
                    let server_queue_version = ss
                        .queue_version
                        .as_ref()
                        .map(|qv| (qv.major.unwrap_or(0), qv.minor.unwrap_or(0)));
                    let next_queue_item_id =
                        ss.next_queue_item.as_ref().and_then(|q| q.queue_item_id);
                    let _ = (server_queue_version, next_queue_item_id); // TODO: use these

                    // Call handler and send response
                    let cmd = PlaybackCommand {
                        state,
                        position_ms,
                        queue_item_id,
                    };
                    let response = {
                        let mut handler = self.handler.lock().unwrap();
                        handler.on_playback_command(self.renderer_id, cmd)
                    };
                    if let Err(e) = self.send_playback_response(&response).await {
                        warn!(error = %e, "Failed to send playback response");
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

                        // Call handler and send activation handshake
                        let activation_state = {
                            let mut handler = self.handler.lock().unwrap();
                            handler.on_activate(self.renderer_id)
                        };
                        if let Err(e) = self.send_activation_handshake(&activation_state).await {
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

                        // Notify handler
                        {
                            let mut handler = self.handler.lock().unwrap();
                            handler.on_deactivate(self.renderer_id);
                        }

                        // Close WebSocket and signal run loop to exit
                        let _ = self.writer.close().await;
                        let _ = self
                            .broadcast_tx
                            .send(RendererBroadcast::Disconnected {
                                session_id: self.session_id.clone(),
                                reason: Some("Server set inactive".to_string()),
                            })
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
                        .broadcast_tx
                        .send(RendererBroadcast::VolumeBroadcast {
                            renderer_id: rid,
                            volume,
                        })
                        .await;
                }
            }

            // SrvrCtrlShuffleModeSet (96)
            t if t == QConnectMessageType::MessageTypeSrvrCtrlShuffleModeSet as i32 => {
                if let Some(sm) = msg.srvr_ctrl_shuffle_mode_set {
                    let enabled = sm.shuffle_on.unwrap_or(false);

                    // Call handler
                    {
                        let mut handler = self.handler.lock().unwrap();
                        handler.on_shuffle_mode(enabled);
                    }
                }
            }

            // SrvrCtrlLoopModeSet (97)
            t if t == QConnectMessageType::MessageTypeSrvrCtrlLoopModeSet as i32 => {
                if let Some(lm) = msg.srvr_ctrl_loop_mode_set {
                    let mode = lm
                        .mode
                        .and_then(|i| LoopMode::try_from(i).ok())
                        .unwrap_or_default();

                    // Call handler
                    {
                        let mut handler = self.handler.lock().unwrap();
                        handler.on_loop_mode(mode);
                    }
                }
            }

            // SrvrCtrlVolumeMuted (98) - Broadcast
            t if t == QConnectMessageType::MessageTypeSrvrCtrlVolumeMuted as i32 => {
                if let Some(vm) = msg.srvr_ctrl_volume_muted {
                    let rid = vm.renderer_id.unwrap_or(0);
                    let muted = vm.value.unwrap_or(false);

                    let _ = self
                        .broadcast_tx
                        .send(RendererBroadcast::VolumeMutedBroadcast {
                            renderer_id: rid,
                            muted,
                        })
                        .await;
                }
            }

            // SrvrCtrlMaxAudioQualityChanged (99) - Broadcast
            t if t == QConnectMessageType::MessageTypeSrvrCtrlMaxAudioQualityChanged as i32 => {
                if let Some(mq) = msg.srvr_ctrl_max_audio_quality_changed {
                    let quality = mq
                        .max_audio_quality
                        .map(AudioQuality::from)
                        .unwrap_or_default();

                    // This broadcast doesn't include renderer_id; applies to active renderer
                    let _ = self
                        .broadcast_tx
                        .send(RendererBroadcast::MaxAudioQualityBroadcast {
                            renderer_id: self.renderer_id,
                            quality,
                        })
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
                        .broadcast_tx
                        .send(RendererBroadcast::FileAudioQualityBroadcast {
                            renderer_id: self.renderer_id,
                            sample_rate_hz,
                        })
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

                        let _ = self
                            .broadcast_tx
                            .send(RendererBroadcast::RendererStateUpdated {
                                renderer_id: rid,
                                state: play_state,
                                position_ms,
                                duration_ms,
                                queue_index,
                            })
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
