//! QConnect WebSocket protocol handling.

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use prost::encoding::{decode_varint, encode_varint};
use prost::Message;
use tokio::net::TcpStream;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{
        client::IntoClientRequest,
        protocol::{Message as WsMessage, WebSocketConfig},
    },
};
use tracing::{debug, trace, warn};

use crate::proto::qconnect::{
    Authenticate, CtrlSrvrAskForQueueState, CtrlSrvrAskForRendererState, CtrlSrvrJoinSession,
    CtrlSrvrSetActiveRenderer, DeviceCapabilities, DeviceInfo, DeviceType, Payload,
    QCloudMessageType, QConnectBatch, QConnectMessage, QConnectMessageType, QueueVersion,
    Subscribe,
};
use crate::{Error, Result};

/// Incoming message from WebSocket.
#[derive(Debug)]
pub enum IncomingMessage {
    /// QConnect batch payload.
    Batch(QConnectBatch),
    /// Raw payload message (for debugging).
    Payload(Payload),
    /// Other envelope message type.
    Other {
        msg_type: i32,
        #[allow(dead_code)] // Kept for debugging
        data: Vec<u8>,
    },
}

/// Default QConnect WebSocket URL.
#[allow(dead_code)] // Useful constant for future use
pub const DEFAULT_WS_URL: &str = "wss://play.qobuz.com/ws";

/// WebSocket transport for QConnect protocol.
pub struct Transport {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    msg_id: u32,
}

impl Transport {
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

    /// Send authentication message.
    async fn authenticate(&mut self, jwt: &str) -> Result<()> {
        let msg = Authenticate {
            msg_id: Some(self.next_msg_id()),
            msg_date: Some(now_ms()),
            jwt: Some(jwt.to_string()),
        };
        let encoded = msg.encode_to_vec();
        trace!(
            msg_id = ?msg.msg_id,
            jwt_len = jwt.len(),
            encoded_len = encoded.len(),
            "TX: Authenticate"
        );
        self.send_envelope(QCloudMessageType::Authenticate as i32, encoded)
            .await?;
        debug!("TX: Authenticate sent");
        Ok(())
    }

    /// Subscribe to the default channel (derived from JWT).
    ///
    /// This must be called after authentication to register for messages.
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

    /// Subscribe to a specific channel.
    #[allow(dead_code)] // Part of the API for future use
    pub async fn subscribe(&mut self, channel: Bytes) -> Result<()> {
        let msg = Subscribe {
            msg_id: Some(self.next_msg_id()),
            msg_date: Some(now_ms()),
            proto: Some(1), // QP_QCONNECT
            channels: vec![channel.to_vec()],
        };
        self.send_envelope(QCloudMessageType::Subscribe as i32, msg.encode_to_vec())
            .await
    }

    /// Send a QConnect batch message.
    pub async fn send_batch(&mut self, batch: QConnectBatch) -> Result<()> {
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

    /// Join session as a controller/renderer device.
    ///
    /// This registers the device with the Qobuz server and allows it to receive
    /// playback commands and queue updates.
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
                min_audio_quality: Some(1),
                max_audio_quality: Some(4), // Match C++ reference
                volume_remote_control: Some(2),
            }),
            software_version: Some(format!("qonductor-{}", env!("CARGO_PKG_VERSION"))),
        };

        let join_msg = QConnectMessage {
            message_type: Some(QConnectMessageType::MessageTypeCtrlSrvrJoinSession as i32),
            ctrl_srvr_join_session: Some(CtrlSrvrJoinSession {
                session_uuid: None, // Server assigns from JWT
                device_info: Some(device_info),
            }),
            ..Default::default()
        };

        let batch = QConnectBatch {
            messages_time: Some(now_ms()),
            messages_id: Some(self.msg_id as i32),
            messages: vec![join_msg],
        };

        self.send_batch(batch).await?;
        debug!(friendly_name, "TX: JoinSession");
        Ok(())
    }

    /// Ask server for current queue state.
    ///
    /// Should be called after receiving `SrvrCtrlSessionState`.
    pub async fn ask_for_queue_state(
        &mut self,
        queue_uuid: &[u8; 16],
        queue_version: Option<(u64, i32)>,
    ) -> Result<()> {
        let msg = QConnectMessage {
            message_type: Some(QConnectMessageType::MessageTypeCtrlSrvrAskForQueueState as i32),
            ctrl_srvr_ask_for_queue_state: Some(CtrlSrvrAskForQueueState {
                queue_version: queue_version.map(|(major, minor)| QueueVersion {
                    major: Some(major),
                    minor: Some(minor),
                }),
                queue_uuid: Some(queue_uuid.to_vec()),
            }),
            ..Default::default()
        };

        let batch = QConnectBatch {
            messages_time: Some(now_ms()),
            messages_id: Some(self.msg_id as i32),
            messages: vec![msg],
        };

        self.send_batch(batch).await?;
        debug!("TX: AskForQueueState");
        Ok(())
    }

    /// Ask server for current renderer state.
    ///
    /// Should be called after receiving `SrvrCtrlSessionState`.
    pub async fn ask_for_renderer_state(&mut self, session_id: u64) -> Result<()> {

        let msg = QConnectMessage {
            message_type: Some(QConnectMessageType::MessageTypeCtrlSrvrAskForRendererState as i32),
            ctrl_srvr_ask_for_renderer_state: Some(CtrlSrvrAskForRendererState {
                session_id: Some(session_id),
            }),
            ..Default::default()
        };

        let batch = QConnectBatch {
            messages_time: Some(now_ms()),
            messages_id: Some(self.msg_id as i32),
            messages: vec![msg],
        };

        self.send_batch(batch).await?;
        debug!(session_id, "TX: AskForRendererState");
        Ok(())
    }

    /// Confirm this renderer as the active renderer.
    ///
    /// This must be called when receiving `SrvrCtrlActiveRendererChanged` with our renderer_id.
    /// Without this confirmation, the server may switch back to the previous renderer.
    pub async fn set_active_renderer(&mut self, renderer_id: u64) -> Result<()> {

        let msg = QConnectMessage {
            message_type: Some(QConnectMessageType::MessageTypeCtrlSrvrSetActiveRenderer as i32),
            ctrl_srvr_set_active_renderer: Some(CtrlSrvrSetActiveRenderer {
                renderer_id: Some(renderer_id as i32),
            }),
            ..Default::default()
        };

        let batch = QConnectBatch {
            messages_time: Some(now_ms()),
            messages_id: Some(self.msg_id as i32),
            messages: vec![msg],
        };

        self.send_batch(batch).await?;
        debug!(renderer_id, "TX: SetActiveRenderer");
        Ok(())
    }

    /// Gracefully close the WebSocket connection.
    pub async fn close(&mut self) -> Result<()> {
        debug!("Closing WebSocket connection");
        self.ws.close(None).await.map_err(Box::new)?;
        Ok(())
    }

    /// Receive next message from WebSocket.
    pub async fn recv(&mut self) -> Result<Option<IncomingMessage>> {
        while let Some(msg) = self.ws.next().await {
            match msg.map_err(Box::new)? {
                WsMessage::Binary(data) => {
                    trace!(len = data.len(), "RX: Binary message");
                    if let Some(incoming) = Self::parse_envelope(&data)? {
                        return Ok(Some(incoming));
                    }
                }
                WsMessage::Ping(data) => {
                    trace!("RX: Ping");
                    self.ws
                        .send(WsMessage::Pong(data))
                        .await
                        .map_err(Box::new)?;
                    trace!("TX: Pong");
                }
                WsMessage::Pong(_) => {
                    trace!("RX: Pong");
                }
                WsMessage::Close(_) => return Err(Error::ConnectionClosed),
                WsMessage::Text(text) => {
                    debug!(text = %text, "RX: Unexpected text message");
                }
                WsMessage::Frame(frame) => {
                    trace!(?frame, "RX: Raw frame (unexpected)");
                }
            }
        }
        Ok(None)
    }

    /// Parse incoming WebSocket envelope message.
    fn parse_envelope(data: &[u8]) -> Result<Option<IncomingMessage>> {
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

        trace!(msg_type, payload_len = payload.len(), "RX: Parsing envelope");

        match msg_type {
            // PAYLOAD = 6
            6 => {
                match Payload::decode(payload) {
                    Ok(p) => {
                        trace!(
                            msg_id = ?p.msg_id,
                            proto = ?p.proto,
                            "RX: Payload envelope"
                        );

                        // If there's an inner payload, try to decode as QConnectBatch
                        if let Some(inner) = &p.payload {
                            match QConnectBatch::decode(inner.as_slice()) {
                                Ok(batch) => {
                                    trace!(
                                        messages_count = batch.messages.len(),
                                        "RX: QConnectBatch"
                                    );
                                    return Ok(Some(IncomingMessage::Batch(batch)));
                                }
                                Err(e) => {
                                    warn!(error = %e, "RX: Failed to decode inner QConnectBatch");
                                }
                            }
                        }
                        return Ok(Some(IncomingMessage::Payload(p)));
                    }
                    Err(e) => {
                        warn!(error = %e, "RX: Failed to decode Payload");
                    }
                }
            }
            // ERROR = 9
            9 => {
                // Hex dump for debugging
                let hex: String = payload.iter().map(|b| format!("{:02x}", b)).collect();
                // Try to extract any readable strings
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
                warn!(
                    hex = %hex,
                    ascii = %ascii,
                    len = payload.len(),
                    "RX: Error from server"
                );
                return Ok(Some(IncomingMessage::Other {
                    msg_type,
                    data: payload.to_vec(),
                }));
            }
            // Other message types
            _ => {
                debug!(msg_type, "RX: Envelope with unhandled type");
                return Ok(Some(IncomingMessage::Other {
                    msg_type,
                    data: payload.to_vec(),
                }));
            }
        }

        Ok(None)
    }

    async fn send_envelope(&mut self, msg_type: i32, data: Vec<u8>) -> Result<()> {
        // Frame format: [msg_type: 1 byte] [payload_len: varint] [payload]
        let payload_len = data.len();
        let mut buf = Vec::with_capacity(1 + 10 + payload_len);
        buf.push(msg_type as u8);
        encode_varint(payload_len as u64, &mut buf);
        buf.extend(data);

        trace!(
            msg_type,
            payload_len,
            frame_len = buf.len(),
            "TX: Envelope"
        );

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

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    /// Helper to build an envelope: [msg_type: 1 byte] [len: varint] [payload]
    fn build_envelope(msg_type: u8, payload: &[u8]) -> Vec<u8> {
        let mut buf = vec![msg_type];
        encode_varint(payload.len() as u64, &mut buf);
        buf.extend_from_slice(payload);
        buf
    }

    #[test]
    fn parse_envelope_empty_returns_none() {
        let result = Transport::parse_envelope(&[]).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn parse_envelope_incomplete_returns_none() {
        // msg_type=6, varint says 100 bytes, but only 5 bytes of payload
        let mut data = vec![6u8];
        encode_varint(100, &mut data);
        data.extend_from_slice(&[1, 2, 3, 4, 5]);

        let result = Transport::parse_envelope(&data).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn parse_envelope_unknown_type_returns_other() {
        let payload = b"some data";
        let data = build_envelope(42, payload);

        let result = Transport::parse_envelope(&data).unwrap();
        match result {
            Some(IncomingMessage::Other { msg_type, data }) => {
                assert_eq!(msg_type, 42);
                assert_eq!(data, payload);
            }
            _ => panic!("Expected IncomingMessage::Other"),
        }
    }

    #[test]
    fn parse_envelope_error_type_returns_other() {
        let payload = b"error message";
        let data = build_envelope(9, payload); // 9 = ERROR

        let result = Transport::parse_envelope(&data).unwrap();
        match result {
            Some(IncomingMessage::Other { msg_type, data }) => {
                assert_eq!(msg_type, 9);
                assert_eq!(data, payload);
            }
            _ => panic!("Expected IncomingMessage::Other for error type"),
        }
    }

    #[test]
    fn parse_envelope_payload_without_inner_batch() {
        // Build a Payload protobuf without inner payload
        let payload_proto = Payload {
            msg_id: Some(123),
            msg_date: Some(1000),
            proto: Some(1),
            src: None,
            dests: vec![],
            payload: None, // No inner payload
        };
        let encoded = payload_proto.encode_to_vec();
        let data = build_envelope(6, &encoded); // 6 = PAYLOAD

        let result = Transport::parse_envelope(&data).unwrap();
        match result {
            Some(IncomingMessage::Payload(p)) => {
                assert_eq!(p.msg_id, Some(123));
                assert_eq!(p.proto, Some(1));
            }
            _ => panic!("Expected IncomingMessage::Payload"),
        }
    }

    #[test]
    fn parse_envelope_payload_with_batch() {
        // Build a QConnectBatch
        let batch = QConnectBatch {
            messages_time: Some(2000),
            messages_id: Some(1),
            messages: vec![],
        };
        let batch_encoded = batch.encode_to_vec();

        // Wrap in Payload
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

        let result = Transport::parse_envelope(&data).unwrap();
        match result {
            Some(IncomingMessage::Batch(b)) => {
                assert_eq!(b.messages_time, Some(2000));
                assert_eq!(b.messages_id, Some(1));
                assert!(b.messages.is_empty());
            }
            _ => panic!("Expected IncomingMessage::Batch"),
        }
    }

    #[test]
    fn parse_envelope_payload_with_invalid_inner_returns_payload() {
        // Payload with garbage inner data (not valid QConnectBatch)
        let payload_proto = Payload {
            msg_id: Some(789),
            msg_date: Some(1000),
            proto: Some(1),
            src: None,
            dests: vec![],
            payload: Some(vec![0xff, 0xff, 0xff]), // Invalid protobuf
        };
        let encoded = payload_proto.encode_to_vec();
        let data = build_envelope(6, &encoded);

        let result = Transport::parse_envelope(&data).unwrap();
        match result {
            Some(IncomingMessage::Payload(p)) => {
                assert_eq!(p.msg_id, Some(789));
                // Inner decode failed, but we still get the Payload
            }
            _ => panic!("Expected IncomingMessage::Payload when inner decode fails"),
        }
    }
}
