//! High-level session management for Qobuz Connect.
//!
//! This module provides semantic events and session management,
//! hiding the raw protobuf message handling from consumers.

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::discovery::{AudioQuality, DeviceConfig, SessionInfo};
use crate::proto::qconnect::{
    Position, QConnectBatch, QConnectMessage, QConnectMessageType, QueueRendererState,
    RndrSrvrFileAudioQualityChanged, RndrSrvrMaxAudioQualityChanged, RndrSrvrStateUpdated,
    RndrSrvrVolumeChanged, RndrSrvrVolumeMuted,
};
use crate::transport::{IncomingMessage, Transport};
use crate::{Error, Result};

/// High-level events from a Qobuz Connect session.
#[derive(Debug, Clone)]
pub enum SessionEvent {
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

    /// One of our devices became the active renderer.
    DeviceActive {
        device_uuid: [u8; 16],
        renderer_id: u64,
    },

    /// Another renderer was added to the session.
    RendererAdded { renderer_id: u64, name: String },

    /// A renderer was removed from the session.
    RendererRemoved { renderer_id: u64 },

    /// The active renderer changed.
    ActiveRendererChanged { renderer_id: u64 },

    /// Our device lost active status - session will disconnect.
    DeviceDeactivated {
        device_uuid: [u8; 16],
        renderer_id: u64,
        new_active_renderer_id: u64,
    },

    /// Queue was updated.
    QueueUpdated {
        tracks: Vec<QueueTrack>,
        version: (u64, i32),
    },

    /// Renderer state updated (response to request_renderer_state).
    RendererStateUpdated {
        renderer_id: u64,
        state: PlayState,
        position_ms: u32,
        duration_ms: u32,
        queue_index: u32,
    },

    /// Server requests playback state change.
    PlaybackCommand {
        renderer_id: u64,
        state: PlayState,
        position_ms: Option<u32>,
    },

    /// Server requests volume change.
    VolumeCommand { renderer_id: u64, volume: u32 },

    /// Loop mode changed.
    LoopModeChanged { mode: LoopMode },

    /// Shuffle mode changed.
    ShuffleModeChanged { enabled: bool },

    // ========================================================================
    // Broadcast events (informational - renderer should NOT respond to these)
    // ========================================================================

    /// Volume changed broadcast (from another renderer).
    VolumeBroadcast { renderer_id: u64, volume: u32 },

    /// Volume muted broadcast (from another renderer).
    VolumeMutedBroadcast { renderer_id: u64, muted: bool },

    /// Max audio quality changed broadcast (from another renderer).
    MaxAudioQualityBroadcast {
        renderer_id: u64,
        quality: AudioQuality,
    },

    /// File audio quality changed broadcast (from another renderer).
    /// Sample rate is in Hz (e.g., 44100, 96000, 192000).
    FileAudioQualityBroadcast {
        renderer_id: u64,
        sample_rate_hz: u32,
    },
}

/// Playback state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    Stopped,
    Playing,
    Paused,
}

impl From<i32> for PlayState {
    fn from(value: i32) -> Self {
        match value {
            1 => PlayState::Stopped,
            2 => PlayState::Playing,
            3 => PlayState::Paused,
            _ => PlayState::Stopped,
        }
    }
}

impl From<PlayState> for i32 {
    fn from(value: PlayState) -> Self {
        match value {
            PlayState::Stopped => 1,
            PlayState::Playing => 2,
            PlayState::Paused => 3,
        }
    }
}

/// Buffer state for renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferState {
    Empty,
    Buffering,
    Ready,
}

impl From<i32> for BufferState {
    fn from(value: i32) -> Self {
        match value {
            1 => BufferState::Empty,
            2 => BufferState::Buffering,
            3 => BufferState::Ready,
            _ => BufferState::Empty,
        }
    }
}

impl From<BufferState> for i32 {
    fn from(value: BufferState) -> Self {
        match value {
            BufferState::Empty => 1,
            BufferState::Buffering => 2,
            BufferState::Ready => 3,
        }
    }
}

/// Loop/repeat mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoopMode {
    #[default]
    Off,
    One,
    All,
}

impl From<i32> for LoopMode {
    fn from(value: i32) -> Self {
        match value {
            1 => LoopMode::Off,
            2 => LoopMode::One,
            3 => LoopMode::All,
            _ => LoopMode::Off,
        }
    }
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
pub(crate) enum SessionCommand {
    /// Report playback state to the server.
    ReportPlaybackState {
        renderer_id: u64,
        state: PlayState,
        position_ms: u32,
        queue_item_id: Option<i32>,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Report volume to the server.
    ReportVolume {
        volume: u32,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Report volume muted state to the server.
    ReportVolumeMuted {
        muted: bool,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Report max audio quality capability to the server.
    ReportMaxAudioQuality {
        quality: AudioQuality,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Report actual file audio quality (sample rate in Hz) to the server.
    ReportFileAudioQuality {
        sample_rate_hz: u32,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Request current queue state from the server.
    RequestQueueState {
        reply: oneshot::Sender<Result<()>>,
    },
    /// Request current renderer state from the server.
    RequestRendererState {
        reply: oneshot::Sender<Result<()>>,
    },
    /// Shutdown the session.
    #[allow(dead_code)] // Part of the API
    Shutdown,
    /// Disconnect the session (e.g., when we lose active status).
    Disconnect,
}

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
        event_tx: mpsc::Sender<SessionEvent>,
        on_disconnect: impl Fn() + Send + 'static,
    ) -> Result<Self> {
        debug!(
            session_id = %session_info.session_id,
            device = %device_config.friendly_name,
            "Connecting session"
        );

        // Connect and set up the WebSocket
        let mut ws = Transport::connect(&session_info.ws_endpoint, &session_info.ws_jwt).await?;
        ws.subscribe_default().await?;
        ws.join_session(&device_config.device_uuid, &device_config.friendly_name)
            .await?;

        let _ = event_tx.send(SessionEvent::Connected).await;

        // Create command channel
        let (command_tx, command_rx) = mpsc::channel(16);

        // Create and spawn the runner
        let runner = SessionRunner {
            session_id: session_info.session_id.clone(),
            ws,
            device_uuid: device_config.device_uuid,
            device_name: device_config.friendly_name.clone(),
            renderer_id: 0,
            event_tx,
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

    /// Report playback state to the server.
    pub async fn report_playback_state(
        &self,
        renderer_id: u64,
        state: PlayState,
        position_ms: u32,
        queue_item_id: Option<i32>,
    ) -> Result<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(SessionCommand::ReportPlaybackState {
                renderer_id,
                state,
                position_ms,
                queue_item_id,
                reply: reply_tx,
            })
            .await
            .map_err(|_| Error::Protocol("Session closed".to_string()))?;

        reply_rx
            .await
            .map_err(|_| Error::Protocol("Session closed".to_string()))?
    }

    /// Report volume to the server.
    ///
    /// Call this after receiving a volume command or during activation handshake.
    pub async fn report_volume(&self, volume: u32) -> Result<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(SessionCommand::ReportVolume {
                volume,
                reply: reply_tx,
            })
            .await
            .map_err(|_| Error::Protocol("Session closed".to_string()))?;

        reply_rx
            .await
            .map_err(|_| Error::Protocol("Session closed".to_string()))?
    }

    /// Report volume muted state to the server.
    ///
    /// Call this during activation handshake or when mute state changes.
    pub async fn report_volume_muted(&self, muted: bool) -> Result<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(SessionCommand::ReportVolumeMuted {
                muted,
                reply: reply_tx,
            })
            .await
            .map_err(|_| Error::Protocol("Session closed".to_string()))?;

        reply_rx
            .await
            .map_err(|_| Error::Protocol("Session closed".to_string()))?
    }

    /// Report max audio quality capability to the server.
    ///
    /// Call this during activation handshake to inform the server of our capabilities.
    pub async fn report_max_audio_quality(&self, quality: AudioQuality) -> Result<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(SessionCommand::ReportMaxAudioQuality {
                quality,
                reply: reply_tx,
            })
            .await
            .map_err(|_| Error::Protocol("Session closed".to_string()))?;

        reply_rx
            .await
            .map_err(|_| Error::Protocol("Session closed".to_string()))?
    }

    /// Report actual file audio quality (sample rate) to the server.
    ///
    /// Call this after loading a track to report the actual sample rate in Hz
    /// (e.g., 44100, 96000, 192000).
    pub async fn report_file_audio_quality(&self, sample_rate_hz: u32) -> Result<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(SessionCommand::ReportFileAudioQuality {
                sample_rate_hz,
                reply: reply_tx,
            })
            .await
            .map_err(|_| Error::Protocol("Session closed".to_string()))?;

        reply_rx
            .await
            .map_err(|_| Error::Protocol("Session closed".to_string()))?
    }

    /// Request current queue state from the server.
    ///
    /// The server will respond with a `QueueUpdated` event containing the current queue.
    pub async fn request_queue_state(&self) -> Result<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(SessionCommand::RequestQueueState { reply: reply_tx })
            .await
            .map_err(|_| Error::Protocol("Session closed".to_string()))?;

        reply_rx
            .await
            .map_err(|_| Error::Protocol("Session closed".to_string()))?
    }

    /// Request current renderer state from the server.
    ///
    /// The server will respond with a `RendererStateUpdated` event containing
    /// the current playback state, position, and queue index.
    pub async fn request_renderer_state(&self) -> Result<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(SessionCommand::RequestRendererState { reply: reply_tx })
            .await
            .map_err(|_| Error::Protocol("Session closed".to_string()))?;

        reply_rx
            .await
            .map_err(|_| Error::Protocol("Session closed".to_string()))?
    }

    /// Disconnect the session.
    ///
    /// This closes the WebSocket connection and stops the session runner.
    /// The session will need to be re-established via mDNS discovery.
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
}

/// The session runner that handles WebSocket communication.
///
/// This runs in a spawned task and processes both WebSocket messages
/// and commands from the SessionHandle.
struct SessionRunner {
    session_id: String,
    ws: Transport,
    device_uuid: [u8; 16],
    device_name: String,
    renderer_id: u64,
    event_tx: mpsc::Sender<SessionEvent>,
    command_rx: mpsc::Receiver<SessionCommand>,
    state: SessionState,
    on_disconnect: Box<dyn Fn() + Send>,
}

impl SessionRunner {
    /// Run the session event loop.
    async fn run(mut self) {
        info!(session_id = %self.session_id, "Session runner starting");

        loop {
            tokio::select! {
                // Handle WebSocket messages
                msg = self.ws.recv() => {
                    match msg {
                        Ok(Some(m)) => {
                            match self.handle_message(m).await {
                                Ok(true) => break, // Handler requested exit
                                Ok(false) => {}
                                Err(e) => warn!(error = %e, "Error handling message"),
                            }
                        }
                        Ok(None) => {
                            info!("WebSocket closed");
                            let _ = self.event_tx.send(SessionEvent::Disconnected {
                                session_id: self.session_id.clone(),
                                reason: None,
                            }).await;
                            break;
                        }
                        Err(e) => {
                            warn!(error = %e, "WebSocket error");
                            let _ = self.event_tx.send(SessionEvent::Disconnected {
                                session_id: self.session_id.clone(),
                                reason: Some(e.to_string()),
                            }).await;
                            break;
                        }
                    }
                }

                // Handle commands from the handle
                cmd = self.command_rx.recv() => {
                    match cmd {
                        Some(SessionCommand::ReportPlaybackState { renderer_id, state, position_ms, queue_item_id, reply }) => {
                            let result = self.do_report_playback_state(renderer_id, state, position_ms, queue_item_id).await;
                            let _ = reply.send(result);
                        }
                        Some(SessionCommand::ReportVolume { volume, reply }) => {
                            let result = self.do_report_volume(volume).await;
                            let _ = reply.send(result);
                        }
                        Some(SessionCommand::ReportVolumeMuted { muted, reply }) => {
                            let result = self.do_report_volume_muted(muted).await;
                            let _ = reply.send(result);
                        }
                        Some(SessionCommand::ReportMaxAudioQuality { quality, reply }) => {
                            let result = self.do_report_max_audio_quality(quality).await;
                            let _ = reply.send(result);
                        }
                        Some(SessionCommand::ReportFileAudioQuality { sample_rate_hz, reply }) => {
                            let result = self.do_report_file_audio_quality(sample_rate_hz).await;
                            let _ = reply.send(result);
                        }
                        Some(SessionCommand::RequestQueueState { reply }) => {
                            let result = self.do_request_queue_state().await;
                            let _ = reply.send(result);
                        }
                        Some(SessionCommand::RequestRendererState { reply }) => {
                            let result = self.do_request_renderer_state().await;
                            let _ = reply.send(result);
                        }
                        Some(SessionCommand::Disconnect) => {
                            info!("Disconnect command received");
                            let _ = self.ws.close().await;
                            let _ = self.event_tx.send(SessionEvent::Disconnected {
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
            }
        }

        // Notify manager that session is done
        (self.on_disconnect)();
        info!(session_id = %self.session_id, "Session runner stopped");
    }

    /// Report playback state to server.
    async fn do_report_playback_state(
        &mut self,
        _renderer_id: u64,
        state: PlayState,
        position_ms: u32,
        current_queue_item_id: Option<i32>,
    ) -> Result<()> {
        debug!(?state, position_ms, "TX: ReportPlaybackState");

        let msg = QConnectMessage {
            message_type: Some(QConnectMessageType::MessageTypeRndrSrvrStateUpdated as i32),
            rndr_srvr_state_updated: Some(RndrSrvrStateUpdated {
                state: Some(QueueRendererState {
                    playing_state: Some(state.into()),
                    current_position: Some(Position {
                        timestamp: Some(now_ms()),
                        value: Some(position_ms),
                    }),
                    current_queue_item_id,
                    ..Default::default()
                }),
            }),
            ..Default::default()
        };

        let batch = QConnectBatch {
            messages_time: Some(now_ms()),
            messages_id: None,
            messages: vec![msg],
        };

        self.ws.send_batch(batch).await
    }

    /// Report volume to server.
    async fn do_report_volume(&mut self, volume: u32) -> Result<()> {
        debug!(volume, "TX: ReportVolume");

        let msg = QConnectMessage {
            message_type: Some(QConnectMessageType::MessageTypeRndrSrvrVolumeChanged as i32),
            rndr_srvr_volume_changed: Some(RndrSrvrVolumeChanged {
                volume: Some(volume),
            }),
            ..Default::default()
        };

        let batch = QConnectBatch {
            messages_time: Some(now_ms()),
            messages_id: None,
            messages: vec![msg],
        };

        self.ws.send_batch(batch).await
    }

    /// Report volume muted state to server.
    async fn do_report_volume_muted(&mut self, muted: bool) -> Result<()> {
        debug!(muted, "TX: ReportVolumeMuted");

        let msg = QConnectMessage {
            message_type: Some(QConnectMessageType::MessageTypeRndrSrvrVolumeMuted as i32),
            rndr_srvr_volume_muted: Some(RndrSrvrVolumeMuted { value: Some(muted) }),
            ..Default::default()
        };

        let batch = QConnectBatch {
            messages_time: Some(now_ms()),
            messages_id: None,
            messages: vec![msg],
        };

        self.ws.send_batch(batch).await
    }

    /// Report max audio quality capability to server.
    async fn do_report_max_audio_quality(&mut self, quality: AudioQuality) -> Result<()> {
        debug!(?quality, "TX: ReportMaxAudioQuality");

        let msg = QConnectMessage {
            message_type: Some(QConnectMessageType::MessageTypeRndrSrvrMaxAudioQualityChanged as i32),
            rndr_srvr_max_audio_quality_changed: Some(RndrSrvrMaxAudioQualityChanged {
                value: Some(quality.into()),
            }),
            ..Default::default()
        };

        let batch = QConnectBatch {
            messages_time: Some(now_ms()),
            messages_id: None,
            messages: vec![msg],
        };

        self.ws.send_batch(batch).await
    }

    /// Report actual file audio quality (sample rate in Hz) to server.
    async fn do_report_file_audio_quality(&mut self, sample_rate_hz: u32) -> Result<()> {
        debug!(sample_rate_hz, "TX: ReportFileAudioQuality");

        let msg = QConnectMessage {
            message_type: Some(QConnectMessageType::MessageTypeRndrSrvrFileAudioQualityChanged as i32),
            rndr_srvr_file_audio_quality_changed: Some(RndrSrvrFileAudioQualityChanged {
                value: Some(sample_rate_hz as i32),
            }),
            ..Default::default()
        };

        let batch = QConnectBatch {
            messages_time: Some(now_ms()),
            messages_id: None,
            messages: vec![msg],
        };

        self.ws.send_batch(batch).await
    }

    /// Request queue state from server.
    async fn do_request_queue_state(&mut self) -> Result<()> {
        // Use session_uuid as queue_uuid (they're the same in QConnect)
        let queue_uuid = self.state.session_uuid.unwrap_or(self.device_uuid);
        debug!("TX: RequestQueueState");
        self.ws.ask_for_queue_state(&queue_uuid, None).await
    }

    /// Request renderer state from server.
    async fn do_request_renderer_state(&mut self) -> Result<()> {
        let session_id = self.state.session_id.unwrap_or(0);
        debug!("TX: RequestRendererState");
        self.ws.ask_for_renderer_state(session_id).await
    }

    /// Handle an incoming WebSocket message.
    /// Returns Ok(true) if the session should exit.
    async fn handle_message(&mut self, msg: IncomingMessage) -> Result<bool> {
        match msg {
            IncomingMessage::Batch(batch) => {
                for m in batch.messages {
                    if self.handle_qconnect_message(m).await? {
                        return Ok(true);
                    }
                }
            }
            IncomingMessage::Payload(p) => {
                debug!(msg_id = ?p.msg_id, "RX: Payload (no batch)");
            }
            IncomingMessage::Other { msg_type, .. } => {
                debug!(msg_type, "RX: Other message type");
            }
        }
        Ok(false)
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
                            .send(SessionEvent::DeviceRegistered {
                                device_uuid: self.device_uuid,
                                renderer_id: rid,
                            })
                            .await;

                        // Declare as active renderer
                        self.ws.set_active_renderer(rid).await?;
                    } else {
                        debug!(renderer_id = rid, name = %name, "RX: AddRenderer (other)");
                        let _ = self
                            .event_tx
                            .send(SessionEvent::RendererAdded {
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
                    debug!(renderer_id = rid, "RX: RemoveRenderer");

                    // Check if it's ours and mark as unregistered
                    if self.renderer_id == rid {
                        self.renderer_id = 0;
                    }

                    let _ = self
                        .event_tx
                        .send(SessionEvent::RendererRemoved { renderer_id: rid })
                        .await;
                }
            }

            // SrvrCtrlSessionState (81)
            t if t == QConnectMessageType::MessageTypeSrvrCtrlSessionState as i32 => {
                if let Some(ss) = msg.srvr_ctrl_session_state {
                    let sid = ss.session_id.unwrap_or(0);
                    debug!(session_id = sid, "RX: SessionState");

                    self.state.session_id = Some(sid);

                    if let Some(uuid_bytes) = ss.session_uuid
                        && uuid_bytes.len() == 16
                    {
                        let mut uuid = [0u8; 16];
                        uuid.copy_from_slice(&uuid_bytes);
                        self.state.session_uuid = Some(uuid);
                    }

                    // Note: Queue state is now requested by the player via request_queue_state()
                    // after receiving DeviceActive event.
                }
            }

            // SrvrCtrlActiveRendererChanged (86)
            t if t == QConnectMessageType::MessageTypeSrvrCtrlActiveRendererChanged as i32 => {
                if let Some(arc) = msg.srvr_ctrl_active_renderer_changed {
                    let rid = arc.renderer_id.unwrap_or(0);
                    debug!(renderer_id = rid, "RX: ActiveRendererChanged");

                    let _ = self
                        .event_tx
                        .send(SessionEvent::ActiveRendererChanged { renderer_id: rid })
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

                    debug!(track_count = tracks.len(), "RX: QueueState");

                    let _ = self
                        .event_tx
                        .send(SessionEvent::QueueUpdated { tracks, version })
                        .await;
                }
            }

            // SrvrRndrSetState (41) - Server telling us to change playback state
            t if t == QConnectMessageType::MessageTypeSrvrRndrSetState as i32 => {
                if let Some(ss) = msg.srvr_rndr_set_state {
                    let state = ss.playing_state.map(PlayState::from).unwrap_or(PlayState::Stopped);
                    let position_ms = ss.current_position;

                    debug!(renderer_id = self.renderer_id, ?state, ?position_ms, "RX: SetState");

                    let _ = self
                        .event_tx
                        .send(SessionEvent::PlaybackCommand {
                            renderer_id: self.renderer_id,
                            state,
                            position_ms,
                        })
                        .await;
                }
            }

            // SrvrRndrSetActive (43) - Server telling us we're active/inactive
            t if t == QConnectMessageType::MessageTypeSrvrRndrSetActive as i32 => {
                if let Some(sa) = msg.srvr_rndr_set_active {
                    let active = sa.active.unwrap_or(false);
                    debug!(active, "RX: SetActive");

                    if active {
                        info!(renderer_id = self.renderer_id, "Server set us active");
                        let _ = self
                            .event_tx
                            .send(SessionEvent::DeviceActive {
                                device_uuid: self.device_uuid,
                                renderer_id: self.renderer_id,
                            })
                            .await;
                    } else {
                        info!(
                            renderer_id = self.renderer_id,
                            "Server set us inactive, disconnecting"
                        );
                        let _ = self
                            .event_tx
                            .send(SessionEvent::DeviceDeactivated {
                                device_uuid: self.device_uuid,
                                renderer_id: self.renderer_id,
                                new_active_renderer_id: 0,
                            })
                            .await;

                        // Close WebSocket and signal run loop to exit
                        let _ = self.ws.close().await;
                        let _ = self
                            .event_tx
                            .send(SessionEvent::Disconnected {
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
                    debug!(renderer_id = rid, volume, "RX: VolumeChanged (broadcast)");

                    let _ = self
                        .event_tx
                        .send(SessionEvent::VolumeBroadcast {
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
                    debug!(enabled, "RX: ShuffleModeSet");

                    let _ = self
                        .event_tx
                        .send(SessionEvent::ShuffleModeChanged { enabled })
                        .await;
                }
            }

            // SrvrCtrlLoopModeSet (97)
            t if t == QConnectMessageType::MessageTypeSrvrCtrlLoopModeSet as i32 => {
                if let Some(lm) = msg.srvr_ctrl_loop_mode_set {
                    let mode = lm.mode.map(LoopMode::from).unwrap_or_default();
                    debug!(?mode, "RX: LoopModeSet");

                    let _ = self
                        .event_tx
                        .send(SessionEvent::LoopModeChanged { mode })
                        .await;
                }
            }

            // SrvrCtrlVolumeMuted (98) - Broadcast
            t if t == QConnectMessageType::MessageTypeSrvrCtrlVolumeMuted as i32 => {
                if let Some(vm) = msg.srvr_ctrl_volume_muted {
                    let rid = vm.renderer_id.unwrap_or(0);
                    let muted = vm.value.unwrap_or(false);
                    debug!(renderer_id = rid, muted, "RX: VolumeMuted (broadcast)");

                    let _ = self
                        .event_tx
                        .send(SessionEvent::VolumeMutedBroadcast {
                            renderer_id: rid,
                            muted,
                        })
                        .await;
                }
            }

            // SrvrCtrlMaxAudioQualityChanged (99) - Broadcast
            t if t == QConnectMessageType::MessageTypeSrvrCtrlMaxAudioQualityChanged as i32 => {
                if let Some(mq) = msg.srvr_ctrl_max_audio_quality_changed {
                    let quality = mq.max_audio_quality.map(AudioQuality::from).unwrap_or_default();
                    debug!(?quality, "RX: MaxAudioQualityChanged (broadcast)");

                    // This broadcast doesn't include renderer_id; applies to active renderer
                    let _ = self
                        .event_tx
                        .send(SessionEvent::MaxAudioQualityBroadcast {
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
                    debug!(sample_rate_hz, "RX: FileAudioQualityChanged (broadcast)");

                    // This broadcast doesn't include renderer_id; applies to active renderer
                    let _ = self
                        .event_tx
                        .send(SessionEvent::FileAudioQualityBroadcast {
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
                            .map(PlayState::from)
                            .unwrap_or(PlayState::Stopped);
                        let position_ms = state
                            .current_position
                            .and_then(|p| p.value)
                            .unwrap_or(0);
                        let duration_ms = state.duration.unwrap_or(0);
                        let queue_index = state.current_queue_index.unwrap_or(0);

                        debug!(
                            renderer_id = rid,
                            ?play_state,
                            position_ms,
                            duration_ms,
                            queue_index,
                            "RX: RendererStateUpdated"
                        );

                        let _ = self
                            .event_tx
                            .send(SessionEvent::RendererStateUpdated {
                                renderer_id: rid,
                                state: play_state,
                                position_ms,
                                duration_ms,
                                queue_index,
                            })
                            .await;
                    } else {
                        debug!(renderer_id = rid, "RX: RendererStateUpdated (no state)");
                    }
                }
            }

            _ => {
                debug!(msg_type, "RX: Unhandled message type");
            }
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
