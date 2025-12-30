//! QConnect WebSocket protocol handling.
//!
//! Provides split reader/writer for concurrent message handling:
//! - `ConnectionReader` yields individual `QConnectMessage`s via async stream
//! - `ConnectionWriter` sends individual `QConnectMessage`s (batched internally)

use futures::stream::{SplitSink, SplitStream, Stream};
use futures::{SinkExt, StreamExt};
use prost::Message;
use prost::encoding::{decode_varint, encode_varint};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{
        client::IntoClientRequest,
        protocol::{Message as WsMessage, WebSocketConfig},
    },
};
use tracing::{debug, trace, warn};

use crate::config::AudioQuality;
use crate::proto::qconnect::{
    Authenticate, CtrlSrvrJoinSession, DeviceCapabilities, DeviceInfo, DeviceType, Payload,
    QCloudMessageType, QConnectBatch, QConnectMessage, QConnectMessageType, Subscribe,
};
use crate::{Error, Result};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

// ============================================================================
// Connection (connection setup)
// ============================================================================

/// WebSocket connection for QConnect protocol.
///
/// Use `connect()` to establish a connection, then `split()` to get
/// separate reader and writer handles for concurrent use.
pub struct Connection {
    ws: WsStream,
    msg_id: u32,
}

impl Connection {
    /// Connect to QConnect WebSocket at the given endpoint.
    pub async fn connect(endpoint: &str, jwt: &str) -> Result<Self> {
        debug!(endpoint, "Connecting to WebSocket");

        let request = endpoint
            .into_client_request()
            .map_err(|e| Error::Protocol(format!("invalid endpoint URL: {e}")))?;

        let config = WebSocketConfig::default();
        let (ws, response) = connect_async_with_config(request, Some(config), false)
            .await
            .map_err(Box::new)?;

        debug!(status = ?response.status(), "WebSocket connected");
        trace!(headers = ?response.headers(), "Response headers");

        let mut session = Self { ws, msg_id: 0 };
        session.authenticate(jwt).await?;
        Ok(session)
    }

    /// Split into reader and writer for concurrent use.
    pub fn split(self) -> (ConnectionReader, ConnectionWriter) {
        let (sink, stream) = self.ws.split();
        (
            ConnectionReader { ws: stream },
            ConnectionWriter {
                ws: sink,
                msg_id: self.msg_id,
            },
        )
    }

    /// Subscribe to the default channel (derived from JWT).
    pub async fn subscribe_default(&mut self) -> Result<()> {
        let msg = Subscribe {
            msg_id: Some(self.next_msg_id()),
            msg_date: Some(now_ms()),
            proto: Some(1), // QP_QCONNECT
            channels: vec![],
        };
        self.send_envelope(QCloudMessageType::Subscribe as i32, msg.encode_to_vec())
            .await?;
        debug!("TX: Subscribe (default channel)");
        Ok(())
    }

    /// Join session as a controller/renderer device.
    pub async fn join_session(
        &mut self,
        device_uuid: &[u8; 16],
        friendly_name: &str,
    ) -> Result<()> {
        let device_info = DeviceInfo {
            device_uuid: Some(device_uuid.to_vec()),
            friendly_name: Some(friendly_name.to_string()),
            brand: Some("Qonductor".to_string()),
            model: Some("Qonductor Rust".to_string()),
            serial_number: None,
            r#type: Some(DeviceType::Speaker as i32),
            capabilities: Some(DeviceCapabilities {
                min_audio_quality: Some(AudioQuality::Mp3 as i32),
                max_audio_quality: Some(AudioQuality::HiRes192 as i32),
                volume_remote_control: Some(2),
            }),
            software_version: Some(format!("qonductor-{}", env!("CARGO_PKG_VERSION"))),
        };

        let join_msg = QConnectMessage {
            message_type: Some(QConnectMessageType::MessageTypeCtrlSrvrJoinSession as i32),
            ctrl_srvr_join_session: Some(CtrlSrvrJoinSession {
                session_uuid: None,
                device_info: Some(device_info),
            }),
            ..Default::default()
        };

        self.send_message(join_msg).await?;
        debug!(friendly_name, "TX: JoinSession");
        Ok(())
    }

    // --- Private helpers ---

    async fn authenticate(&mut self, jwt: &str) -> Result<()> {
        let msg = Authenticate {
            msg_id: Some(self.next_msg_id()),
            msg_date: Some(now_ms()),
            jwt: Some(jwt.to_string()),
        };
        self.send_envelope(QCloudMessageType::Authenticate as i32, msg.encode_to_vec())
            .await?;
        debug!("TX: Authenticate sent");
        Ok(())
    }

    async fn send_message(&mut self, msg: QConnectMessage) -> Result<()> {
        debug!("TX: {}", format_qconnect_message(&msg));
        let batch = QConnectBatch {
            messages_time: Some(now_ms()),
            messages_id: Some(self.msg_id as i32),
            messages: vec![msg],
        };
        let payload = Payload {
            msg_id: Some(self.next_msg_id()),
            msg_date: Some(now_ms()),
            proto: Some(1),
            src: None,
            dests: vec![],
            payload: Some(batch.encode_to_vec()),
        };
        self.send_envelope(QCloudMessageType::Payload as i32, payload.encode_to_vec())
            .await
    }

    async fn send_envelope(&mut self, msg_type: i32, data: Vec<u8>) -> Result<()> {
        let buf = encode_envelope(msg_type, &data);
        self.ws
            .send(WsMessage::Binary(buf))
            .await
            .map_err(Box::new)?;
        Ok(())
    }

    fn next_msg_id(&mut self) -> u32 {
        self.msg_id += 1;
        self.msg_id
    }
}

// ============================================================================
// ConnectionReader
// ============================================================================

/// Reader half of a split connection.
///
/// Convert to a stream of messages with `into_stream()`.
pub struct ConnectionReader {
    ws: SplitStream<WsStream>,
}

impl ConnectionReader {
    /// Convert into an async stream of QConnect messages.
    ///
    /// Messages from batches are yielded one at a time.
    /// The stream ends when the WebSocket closes.
    pub fn into_stream(mut self) -> impl Stream<Item = Result<QConnectMessage>> {
        async_stream::try_stream! {
            while let Some(msg) = self.ws.next().await {
                match msg.map_err(Box::new)? {
                    WsMessage::Binary(data) => {
                        if let Some(batch) = parse_envelope(&data)? {
                            for m in batch.messages {
                                debug!("RX: {}", format_qconnect_message(&m));
                                yield m;
                            }
                        }
                    }
                    WsMessage::Close(_) => {
                        debug!("RX: Close");
                        break;
                    }
                    WsMessage::Ping(_) => {
                        // Note: tungstenite auto-responds to pings when using split()
                        trace!("RX: Ping (auto-pong)");
                    }
                    WsMessage::Pong(_) => {
                        trace!("RX: Pong");
                    }
                    WsMessage::Text(text) => {
                        debug!(text = %text, "RX: Unexpected text message");
                    }
                    WsMessage::Frame(frame) => {
                        trace!(?frame, "RX: Raw frame");
                    }
                }
            }
        }
    }
}

// ============================================================================
// ConnectionWriter
// ============================================================================

/// Writer half of a split connection.
///
/// Send individual QConnect messages - batching is handled internally.
pub struct ConnectionWriter {
    ws: SplitSink<WsStream, WsMessage>,
    msg_id: u32,
}

impl ConnectionWriter {
    /// Send a single QConnectMessage (wrapped in batch internally).
    pub async fn send(&mut self, msg: QConnectMessage) -> Result<()> {
        debug!("TX: {}", format_qconnect_message(&msg));
        let batch = QConnectBatch {
            messages_time: Some(now_ms()),
            messages_id: Some(self.msg_id as i32),
            messages: vec![msg],
        };
        let payload = Payload {
            msg_id: Some(self.next_msg_id()),
            msg_date: Some(now_ms()),
            proto: Some(1),
            src: None,
            dests: vec![],
            payload: Some(batch.encode_to_vec()),
        };
        let buf = encode_envelope(QCloudMessageType::Payload as i32, &payload.encode_to_vec());
        self.ws
            .send(WsMessage::Binary(buf))
            .await
            .map_err(Box::new)?;
        Ok(())
    }

    /// Gracefully close the WebSocket connection.
    pub async fn close(&mut self) -> Result<()> {
        debug!("Closing WebSocket connection");
        self.ws.close().await.map_err(Box::new)?;
        Ok(())
    }

    fn next_msg_id(&mut self) -> u32 {
        self.msg_id += 1;
        self.msg_id
    }
}

// ============================================================================
// Shared helpers
// ============================================================================

/// Format a QConnectMessage for debug logging, showing only the populated field.
pub fn format_qconnect_message(msg: &QConnectMessage) -> String {
    match msg.message_type.unwrap_or(0) {
        // Renderer -> Server
        21 => format!("{:?}", msg.rndr_srvr_join_session),
        22 => format!("{:?}", msg.rndr_srvr_device_info_updated),
        23 => format!("{:?}", msg.rndr_srvr_state_updated),
        24 => format!("{:?}", msg.rndr_srvr_renderer_action),
        25 => format!("{:?}", msg.rndr_srvr_volume_changed),
        26 => format!("{:?}", msg.rndr_srvr_file_audio_quality_changed),
        27 => format!("{:?}", msg.rndr_srvr_device_audio_quality_changed),
        28 => format!("{:?}", msg.rndr_srvr_max_audio_quality_changed),
        29 => format!("{:?}", msg.rndr_srvr_volume_muted),
        // Server -> Renderer
        41 => format!("{:?}", msg.srvr_rndr_set_state),
        42 => format!("{:?}", msg.srvr_rndr_set_volume),
        43 => format!("{:?}", msg.srvr_rndr_set_active),
        44 => format!("{:?}", msg.srvr_rndr_set_max_audio_quality),
        45 => format!("{:?}", msg.srvr_rndr_set_loop_mode),
        46 => format!("{:?}", msg.srvr_rndr_set_shuffle_mode),
        47 => format!("{:?}", msg.srvr_rndr_set_autoplay_mode),
        // Controller -> Server
        61 => format!("{:?}", msg.ctrl_srvr_join_session),
        62 => format!("{:?}", msg.ctrl_srvr_set_player_state),
        63 => format!("{:?}", msg.ctrl_srvr_set_active_renderer),
        64 => format!("{:?}", msg.ctrl_srvr_set_volume),
        65 => format!("{:?}", msg.ctrl_srvr_clear_queue),
        66 => format!("{:?}", msg.ctrl_srvr_queue_load_tracks),
        67 => format!("{:?}", msg.ctrl_srvr_queue_insert_tracks),
        68 => format!("{:?}", msg.ctrl_srvr_queue_add_tracks),
        69 => format!("{:?}", msg.ctrl_srvr_queue_remove_tracks),
        70 => format!("{:?}", msg.ctrl_srvr_queue_reorder_tracks),
        71 => format!("{:?}", msg.ctrl_srvr_set_shuffle_mode),
        72 => format!("{:?}", msg.ctrl_srvr_set_loop_mode),
        73 => format!("{:?}", msg.ctrl_srvr_mute_volume),
        74 => format!("{:?}", msg.ctrl_srvr_set_max_audio_quality),
        75 => format!("{:?}", msg.ctrl_srvr_set_queue_state),
        76 => format!("{:?}", msg.ctrl_srvr_ask_for_queue_state),
        77 => format!("{:?}", msg.ctrl_srvr_ask_for_renderer_state),
        78 => format!("{:?}", msg.ctrl_srvr_set_autoplay_mode),
        79 => format!("{:?}", msg.ctrl_srvr_autoplay_load_tracks),
        80 => format!("{:?}", msg.ctrl_srvr_autoplay_remove_tracks),
        // Server -> Controller
        81 => format!("{:?}", msg.srvr_ctrl_session_state),
        82 => format!("{:?}", msg.srvr_ctrl_renderer_state_updated),
        83 => format!("{:?}", msg.srvr_ctrl_add_renderer),
        84 => format!("{:?}", msg.srvr_ctrl_update_renderer),
        85 => format!("{:?}", msg.srvr_ctrl_remove_renderer),
        86 => format!("{:?}", msg.srvr_ctrl_active_renderer_changed),
        87 => format!("{:?}", msg.srvr_ctrl_volume_changed),
        88 => format!("{:?}", msg.srvr_ctrl_queue_error_message),
        89 => format!("{:?}", msg.srvr_ctrl_queue_cleared),
        90 => format!("{:?}", msg.srvr_ctrl_queue_state),
        91 => format!("{:?}", msg.srvr_ctrl_queue_tracks_loaded),
        92 => format!("{:?}", msg.srvr_ctrl_queue_tracks_inserted),
        93 => format!("{:?}", msg.srvr_ctrl_queue_tracks_added),
        94 => format!("{:?}", msg.srvr_ctrl_queue_tracks_removed),
        95 => format!("{:?}", msg.srvr_ctrl_queue_tracks_reordered),
        96 => format!("{:?}", msg.srvr_ctrl_shuffle_mode_set),
        97 => format!("{:?}", msg.srvr_ctrl_loop_mode_set),
        98 => format!("{:?}", msg.srvr_ctrl_volume_muted),
        99 => format!("{:?}", msg.srvr_ctrl_max_audio_quality_changed),
        100 => format!("{:?}", msg.srvr_ctrl_file_audio_quality_changed),
        101 => format!("{:?}", msg.srvr_ctrl_device_audio_quality_changed),
        102 => format!("{:?}", msg.srvr_ctrl_autoplay_mode_set),
        103 => format!("{:?}", msg.srvr_ctrl_autoplay_tracks_loaded),
        104 => format!("{:?}", msg.srvr_ctrl_autoplay_tracks_removed),
        105 => format!("{:?}", msg.srvr_ctrl_queue_version_changed),
        _ => "None".to_string(),
    }
}

/// Encode an envelope frame: [msg_type: 1 byte] [payload_len: varint] [payload]
fn encode_envelope(msg_type: i32, data: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(1 + 10 + data.len());
    buf.push(msg_type as u8);
    encode_varint(data.len() as u64, &mut buf);
    buf.extend(data);
    buf
}

/// Parse incoming WebSocket envelope message.
///
/// Returns `Some(batch)` for valid QConnect batches, `None` for other message types.
fn parse_envelope(data: &[u8]) -> Result<Option<QConnectBatch>> {
    if data.is_empty() {
        return Ok(None);
    }

    // Frame format: [msg_type: 1 byte] [payload_len: varint] [payload]
    let msg_type = data[0] as i32;

    // Read varint length
    let mut cursor = &data[1..];
    let payload_len = match decode_varint(&mut cursor) {
        Ok(v) => v,
        Err(_) => {
            warn!("Failed to decode varint length");
            return Ok(None);
        }
    };
    let varint_bytes = data.len() - 1 - cursor.len();

    let payload_start = 1 + varint_bytes;
    if data.len() < payload_start + payload_len as usize {
        warn!(
            expected = payload_start + payload_len as usize,
            got = data.len(),
            "Incomplete envelope"
        );
        return Ok(None);
    }

    let payload = &data[payload_start..payload_start + payload_len as usize];

    trace!(
        msg_type,
        payload_len = payload.len(),
        "RX: Parsing envelope"
    );

    match msg_type {
        // PAYLOAD = 6
        6 => match Payload::decode(payload) {
            Ok(p) => {
                trace!(msg_id = ?p.msg_id, proto = ?p.proto, "RX: Payload envelope");

                if let Some(inner) = &p.payload {
                    match QConnectBatch::decode(inner.as_slice()) {
                        Ok(batch) => return Ok(Some(batch)),
                        Err(e) => {
                            warn!(error = %e, "RX: Failed to decode inner QConnectBatch");
                        }
                    }
                } else {
                    debug!(msg_id = ?p.msg_id, "RX: Payload without inner batch");
                }
            }
            Err(e) => {
                warn!(error = %e, "RX: Failed to decode Payload");
            }
        },
        // ERROR = 9
        9 => {
            let hex: String = payload.iter().map(|b| format!("{:02x}", b)).collect();
            let ascii: String = payload
                .iter()
                .map(|&b| {
                    if b.is_ascii_graphic() || b == b' ' {
                        b as char
                    } else {
                        '.'
                    }
                })
                .collect();
            warn!(hex = %hex, ascii = %ascii, len = payload.len(), "RX: Error from server");
        }
        // Other message types
        _ => {
            debug!(msg_type, "RX: Envelope with unhandled type");
        }
    }

    Ok(None)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to build an envelope: [msg_type: 1 byte] [len: varint] [payload]
    fn build_envelope(msg_type: u8, payload: &[u8]) -> Vec<u8> {
        let mut buf = vec![msg_type];
        encode_varint(payload.len() as u64, &mut buf);
        buf.extend_from_slice(payload);
        buf
    }

    #[test]
    fn parse_envelope_empty_returns_none() {
        let result = parse_envelope(&[]).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn parse_envelope_incomplete_returns_none() {
        let mut data = vec![6u8];
        encode_varint(100, &mut data);
        data.extend_from_slice(&[1, 2, 3, 4, 5]);

        let result = parse_envelope(&data).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn parse_envelope_unknown_type_returns_none() {
        let payload = b"some data";
        let data = build_envelope(42, payload);

        let result = parse_envelope(&data).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn parse_envelope_error_type_returns_none() {
        let payload = b"error message";
        let data = build_envelope(9, payload);

        let result = parse_envelope(&data).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn parse_envelope_payload_without_inner_batch_returns_none() {
        let payload_proto = Payload {
            msg_id: Some(123),
            msg_date: Some(1000),
            proto: Some(1),
            src: None,
            dests: vec![],
            payload: None,
        };
        let encoded = payload_proto.encode_to_vec();
        let data = build_envelope(6, &encoded);

        let result = parse_envelope(&data).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn parse_envelope_payload_with_batch() {
        let batch = QConnectBatch {
            messages_time: Some(2000),
            messages_id: Some(1),
            messages: vec![],
        };
        let batch_encoded = batch.encode_to_vec();

        let payload_proto = Payload {
            msg_id: Some(456),
            msg_date: Some(1000),
            proto: Some(1),
            src: None,
            dests: vec![],
            payload: Some(batch_encoded),
        };
        let encoded = payload_proto.encode_to_vec();
        let data = build_envelope(6, &encoded);

        let result = parse_envelope(&data).unwrap();
        let batch = result.expect("Expected Some(QConnectBatch)");
        assert_eq!(batch.messages_time, Some(2000));
        assert_eq!(batch.messages_id, Some(1));
        assert!(batch.messages.is_empty());
    }

    #[test]
    fn parse_envelope_payload_with_invalid_inner_returns_none() {
        let payload_proto = Payload {
            msg_id: Some(789),
            msg_date: Some(1000),
            proto: Some(1),
            src: None,
            dests: vec![],
            payload: Some(vec![0xff, 0xff, 0xff]),
        };
        let encoded = payload_proto.encode_to_vec();
        let data = build_envelope(6, &encoded);

        let result = parse_envelope(&data).unwrap();
        assert!(result.is_none());
    }
}
