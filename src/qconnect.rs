//! Internal WebSocket session runner for Qobuz Connect.
//!
//! This module handles the low-level WebSocket communication with the
//! Qobuz server. It processes messages, manages heartbeats, and routes
//! events to the user via channels.

use std::pin::Pin;

use futures::StreamExt;
use futures::stream::Stream;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Duration, interval};
use tracing::{debug, info, warn};

use crate::Result;
use crate::config::{DeviceConfig, SessionInfo};
use crate::connection::{Connection, ConnectionWriter};
use crate::event::{
    dispatch_notification, ActivationState, Command, Notification, Responder, SessionEvent,
};
use crate::msg::{
    ctrl::{AskForQueueState, AskForRendererState, SetActiveRenderer},
    report::{
        FileAudioQualityChanged, MaxAudioQualityChanged, StateUpdated, VolumeChanged, VolumeMuted,
    },
    QueueRendererState,
};
use crate::proto::qconnect::{QConnectMessage, QConnectMessageType};
use crate::session::SessionCommand;

// ============================================================================
// Public API
// ============================================================================

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

    let _ = event_tx
        .send(SessionEvent::Notification(Notification::Connected))
        .await;

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
        api_jwt: session_info.api_jwt.clone(),
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
    api_jwt: String,
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
                            let _ = self.event_tx.send(SessionEvent::Notification(Notification::Disconnected {
                                session_id: self.session_id.clone(),
                                reason: Some(e.to_string()),
                            })).await;
                            break;
                        }
                        None => {
                            info!("WebSocket closed");
                            let _ = self.event_tx.send(SessionEvent::Notification(Notification::Disconnected {
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
                        let _ = self.event_tx.send(SessionEvent::Command(Command::Heartbeat {
                            respond: Responder::new(tx),
                        })).await;
                        if let Ok(Some(resp)) = rx.await
                            && let Err(e) = self.send_renderer_state(&resp).await
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
        let _ = self
            .event_tx
            .send(SessionEvent::Notification(Notification::SessionClosed {
                device_uuid: self.device_uuid,
            }))
            .await;
        info!(session_id = %self.session_id, "Session runner stopped");
    }

    /// Handle a command from the user.
    async fn handle_command(&mut self, command: SessionCommand) -> Result<()> {
        match command {
            SessionCommand::ReportState(resp) => self.send_renderer_state(&resp).await,
            SessionCommand::ReportVolume(volume) => self.do_report_volume(volume).await,
            SessionCommand::ReportVolumeMuted(muted) => self.do_report_volume_muted(muted).await,
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
            rndr_srvr_volume_changed: Some(VolumeChanged {
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
            rndr_srvr_volume_muted: Some(VolumeMuted {
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
            rndr_srvr_max_audio_quality_changed: Some(MaxAudioQualityChanged {
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
            rndr_srvr_file_audio_quality_changed: Some(FileAudioQualityChanged {
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
            ctrl_srvr_ask_for_queue_state: Some(AskForQueueState {
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
            ctrl_srvr_ask_for_renderer_state: Some(AskForRendererState {
                session_id: Some(session_id),
            }),
            ..Default::default()
        };
        self.writer.send(msg).await
    }

    /// Send a QueueRendererState to the server.
    async fn send_renderer_state(&mut self, state: &QueueRendererState) -> Result<()> {
        let msg = QConnectMessage {
            message_type: Some(QConnectMessageType::MessageTypeRndrSrvrStateUpdated as i32),
            rndr_srvr_state_updated: Some(StateUpdated {
                state: Some(*state),
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
    async fn handle_qconnect_message(&mut self, mut msg: QConnectMessage) -> Result<bool> {
        let msg_type = msg.message_type.unwrap_or(0);

        // Handle messages that need special logic
        match msg_type {
            // SrvrCtrlAddRenderer (83) - Check if it's our device
            t if t == QConnectMessageType::MessageTypeSrvrCtrlAddRenderer as i32 => {
                if let Some(add) = &msg.srvr_ctrl_add_renderer
                    && let Some(renderer) = &add.renderer
                {
                    let renderer_uuid: Option<[u8; 16]> = renderer
                        .device_uuid
                        .as_ref()
                        .and_then(|u| u.as_slice().try_into().ok());

                    let rid = add.renderer_id.unwrap_or(0);

                    // Check if this is our device
                    if renderer_uuid == Some(self.device_uuid) {
                        self.renderer_id = rid;
                        info!(renderer_id = rid, name = %self.device_name, "Our device registered");

                        let _ = self
                            .event_tx
                            .send(SessionEvent::Notification(Notification::DeviceRegistered {
                                device_uuid: self.device_uuid,
                                renderer_id: rid,
                                api_jwt: self.api_jwt.clone(),
                            }))
                            .await;

                        // Declare as active renderer
                        let set_active_msg = QConnectMessage {
                            message_type: Some(
                                QConnectMessageType::MessageTypeCtrlSrvrSetActiveRenderer as i32,
                            ),
                            ctrl_srvr_set_active_renderer: Some(SetActiveRenderer {
                                renderer_id: Some(rid as i32),
                            }),
                            ..Default::default()
                        };
                        self.writer.send(set_active_msg).await?;
                    }
                }
                // Also emit the notification for downstream consumers
                if let Some(notification) = dispatch_notification(&mut msg) {
                    let _ = self
                        .event_tx
                        .send(SessionEvent::Notification(notification))
                        .await;
                }
            }

            // SrvrCtrlRemoveRenderer (85) - Track if it's our device
            t if t == QConnectMessageType::MessageTypeSrvrCtrlRemoveRenderer as i32 => {
                if let Some(rem) = &msg.srvr_ctrl_remove_renderer {
                    let rid = rem.renderer_id.unwrap_or(0);
                    if self.renderer_id == rid {
                        self.renderer_id = 0;
                    }
                }
                // Also emit the notification
                if let Some(notification) = dispatch_notification(&mut msg) {
                    let _ = self
                        .event_tx
                        .send(SessionEvent::Notification(notification))
                        .await;
                }
            }

            // SrvrCtrlSessionState (81) - Store session state
            t if t == QConnectMessageType::MessageTypeSrvrCtrlSessionState as i32 => {
                if let Some(ss) = &msg.srvr_ctrl_session_state {
                    self.state.session_id = Some(ss.session_id.unwrap_or(0));

                    if let Some(uuid_bytes) = &ss.session_uuid
                        && uuid_bytes.len() == 16
                    {
                        let mut uuid = [0u8; 16];
                        uuid.copy_from_slice(uuid_bytes);
                        self.state.session_uuid = Some(uuid);
                    }

                    // Request renderer state to get current playback position
                    if let Err(e) = self.do_request_renderer_state().await {
                        warn!(error = %e, "Failed to request renderer state");
                    }
                }
                // Emit notification for downstream consumers
                if let Some(notification) = dispatch_notification(&mut msg) {
                    let _ = self
                        .event_tx
                        .send(SessionEvent::Notification(notification))
                        .await;
                }
            }

            // SrvrCtrlRendererStateUpdated (82) - Special RestoreState logic
            t if t == QConnectMessageType::MessageTypeSrvrCtrlRendererStateUpdated as i32 => {
                if let Some(rsu) = msg.srvr_ctrl_renderer_state_updated {
                    let rid = rsu.renderer_id.unwrap_or(0);

                    // If from another renderer and we're not active, emit RestoreState
                    if rid != self.renderer_id && !self.is_active {
                        let _ = self
                            .event_tx
                            .send(SessionEvent::Notification(Notification::RestoreState(rsu)))
                            .await;
                    } else {
                        // Otherwise emit as regular RendererStateUpdated
                        let _ = self
                            .event_tx
                            .send(SessionEvent::Notification(Notification::RendererStateUpdated(
                                rsu,
                            )))
                            .await;
                    }
                }
            }

            // SrvrRndrSetState (41) - Server commands play/pause/seek
            t if t == QConnectMessageType::MessageTypeSrvrRndrSetState as i32 => {
                if let Some(ss) = msg.srvr_rndr_set_state {
                    // Only respond if there's an actual state change request
                    if ss.playing_state.is_some() || ss.current_position.is_some() {
                        let (tx, rx) = oneshot::channel();
                        let _ = self
                            .event_tx
                            .send(SessionEvent::Command(Command::SetState {
                                cmd: ss,
                                respond: Responder::new(tx),
                            }))
                            .await;
                        if let Ok(response) = rx.await
                            && let Err(e) = self.send_renderer_state(&response).await
                        {
                            warn!(error = %e, "Failed to send playback response");
                        }
                    }
                }
            }

            // SrvrRndrSetActive (43) - Server activates/deactivates us
            t if t == QConnectMessageType::MessageTypeSrvrRndrSetActive as i32 => {
                if let Some(sa) = msg.srvr_rndr_set_active {
                    let active = sa.active.unwrap_or(false);

                    if active {
                        info!(renderer_id = self.renderer_id, "Server set us active");
                        self.is_active = true;

                        let (tx, rx) = oneshot::channel();
                        let _ = self
                            .event_tx
                            .send(SessionEvent::Command(Command::SetActive {
                                cmd: sa,
                                respond: Responder::new(tx),
                            }))
                            .await;
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
                        info!(renderer_id = self.renderer_id, "Server set us inactive");
                        self.is_active = false;

                        let _ = self
                            .event_tx
                            .send(SessionEvent::Notification(Notification::Deactivated))
                            .await;

                        // Close WebSocket and signal run loop to exit
                        let _ = self.writer.close().await;
                        let _ = self
                            .event_tx
                            .send(SessionEvent::Notification(Notification::Disconnected {
                                session_id: self.session_id.clone(),
                                reason: Some("Server set inactive".to_string()),
                            }))
                            .await;
                        return Ok(true);
                    }
                }
            }

            // All other SrvrCtrl notifications - dispatch via macro-generated function
            _ => {
                if let Some(notification) = dispatch_notification(&mut msg) {
                    let _ = self
                        .event_tx
                        .send(SessionEvent::Notification(notification))
                        .await;
                }
            }
        }

        Ok(false)
    }
}
