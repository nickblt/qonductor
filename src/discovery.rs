//! mDNS service discovery and HTTP endpoint handlers for Qobuz Connect.
//!
//! This module provides service advertisement via mDNS/Zeroconf and HTTP endpoints
//! that allow Qobuz controllers to discover and connect to devices.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use mdns_sd::{ServiceDaemon, ServiceInfo};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::config::{AudioQuality, DeviceConfig, SessionInfo};
use crate::event::SessionEvent;
use crate::proto::qconnect::DeviceType;
use crate::session::{DeviceSession, SharedCommandTx};
use crate::{Error, Result};

/// QConnect SDK version to advertise via mDNS.
/// Must be >= 0.9.5 for Qobuz apps to accept the device.
const QCONNECT_SDK_VERSION: &str = "0.9.6";

/// Extension trait to get mDNS string representation for DeviceType.
pub trait DeviceTypeExt {
    /// Returns the string representation for mDNS TXT record.
    fn as_str(&self) -> &'static str;
}

impl DeviceTypeExt for DeviceType {
    fn as_str(&self) -> &'static str {
        match self {
            DeviceType::Unknown => "UNKNOWN",
            DeviceType::Speaker => "SPEAKER",
            DeviceType::Speakerbox => "SPEAKERBOX",
            DeviceType::Tv => "TV",
            DeviceType::Speakerbox2 => "SPEAKERBOX2",
            DeviceType::Laptop => "LAPTOP",
            DeviceType::Phone => "PHONE",
            DeviceType::GoogleCast => "GOOGLE_CAST",
            DeviceType::Headphones => "HEADPHONES",
            DeviceType::Tablet => "TABLET",
        }
    }
}

/// Convert audio quality capability level to display string for HTTP responses.
fn audio_quality_display(quality: AudioQuality) -> &'static str {
    match quality {
        AudioQuality::Mp3 => "MP3",
        AudioQuality::FlacLossless => "LOSSLESS",
        AudioQuality::HiRes96 => "HIRES_L2",
        AudioQuality::HiRes192 => "HIRES_L3",
    }
}

/// Event emitted when a device is selected in the Qobuz app.
#[derive(Debug, Clone)]
pub struct DeviceSelected {
    /// The device that was selected.
    pub device_uuid: [u8; 16],
    /// Session credentials from Qobuz.
    pub session_info: SessionInfo,
}

// ============================================================================
// HTTP Request/Response Types
// ============================================================================

/// Response for GET /devices/{uuid}/get-display-info
#[derive(Debug, Serialize)]
struct DisplayInfoResponse {
    #[serde(rename = "type")]
    device_type: String,
    friendly_name: String,
    model_display_name: String,
    brand_display_name: String,
    serial_number: String,
    max_audio_quality: String,
}

/// Response for GET /devices/{uuid}/get-connect-info
#[derive(Debug, Serialize)]
struct ConnectInfoResponse {
    current_session_id: String,
    app_id: String,
}

/// JWT info in connect request.
#[derive(Debug, Deserialize)]
struct JwtInfo {
    endpoint: Option<String>,
    jwt: String,
    exp: u64,
}

/// Request body for POST /devices/{uuid}/connect-to-qconnect
#[derive(Debug, Deserialize)]
struct ConnectRequest {
    session_id: String,
    jwt_qconnect: JwtInfo,
    jwt_api: JwtInfo,
}

/// Response for POST /devices/{uuid}/connect-to-qconnect
#[derive(Debug, Serialize)]
struct ConnectResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

// ============================================================================
// Registered Device
// ============================================================================

/// A device registered with the registry.
struct RegisteredDevice {
    config: DeviceConfig,
    /// The mDNS service info for this device (stored for re-announcements).
    service_info: ServiceInfo,
    current_session_id: Option<String>,
    /// Event channel sender for this device's session events.
    event_tx: mpsc::Sender<SessionEvent>,
    /// Shared command sender, populated when session connects.
    command_tx: SharedCommandTx,
}

// ============================================================================
// Shared State
// ============================================================================

struct AppState {
    devices: RwLock<HashMap<[u8; 16], RegisteredDevice>>,
    event_tx: mpsc::Sender<DeviceSelected>,
}

// ============================================================================
// HTTP Handlers
// ============================================================================

async fn get_display_info(
    State(state): State<Arc<AppState>>,
    Path(uuid_hex): Path<String>,
) -> std::result::Result<Json<DisplayInfoResponse>, StatusCode> {
    let uuid = Uuid::parse_str(&uuid_hex)
        .ok()
        .map(|u| *u.as_bytes())
        .ok_or(StatusCode::BAD_REQUEST)?;

    let devices = state.devices.read().await;
    let device = devices.get(&uuid).ok_or(StatusCode::NOT_FOUND)?;

    debug!(device = %device.config.friendly_name, "GET get-display-info");

    Ok(Json(DisplayInfoResponse {
        device_type: device.config.device_type.as_str().to_string(),
        friendly_name: device.config.friendly_name.clone(),
        model_display_name: device.config.model.clone(),
        brand_display_name: device.config.brand.clone(),
        serial_number: device.config.uuid_formatted(),
        max_audio_quality: audio_quality_display(device.config.max_audio_quality).to_string(),
    }))
}

async fn get_connect_info(
    State(state): State<Arc<AppState>>,
    Path(uuid_hex): Path<String>,
) -> std::result::Result<Json<ConnectInfoResponse>, StatusCode> {
    let uuid = Uuid::parse_str(&uuid_hex)
        .ok()
        .map(|u| *u.as_bytes())
        .ok_or(StatusCode::BAD_REQUEST)?;

    let devices = state.devices.read().await;
    let device = devices.get(&uuid).ok_or(StatusCode::NOT_FOUND)?;

    debug!(device = %device.config.friendly_name, "GET get-connect-info");

    Ok(Json(ConnectInfoResponse {
        current_session_id: device.current_session_id.clone().unwrap_or_default(),
        app_id: device.config.app_id.clone(),
    }))
}

async fn connect_to_qconnect(
    State(state): State<Arc<AppState>>,
    Path(uuid_hex): Path<String>,
    Json(req): Json<ConnectRequest>,
) -> std::result::Result<Json<ConnectResponse>, StatusCode> {
    let uuid = Uuid::parse_str(&uuid_hex)
        .ok()
        .map(|u| *u.as_bytes())
        .ok_or(StatusCode::BAD_REQUEST)?;

    // Update device's current session
    {
        let mut devices = state.devices.write().await;
        let device = devices.get_mut(&uuid).ok_or(StatusCode::NOT_FOUND)?;

        info!(
            device = %device.config.friendly_name,
            session_id = %req.session_id,
            "POST connect-to-qconnect"
        );

        device.current_session_id = Some(req.session_id.clone());
    }

    // Extract endpoint, defaulting to the standard QConnect WebSocket URL
    let ws_endpoint = req
        .jwt_qconnect
        .endpoint
        .unwrap_or_else(|| "wss://play.qobuz.com/ws".to_string());

    let session_info = SessionInfo {
        session_id: req.session_id,
        ws_endpoint,
        ws_jwt: req.jwt_qconnect.jwt,
        ws_jwt_exp: req.jwt_qconnect.exp,
        api_jwt: req.jwt_api.jwt,
        api_jwt_exp: req.jwt_api.exp,
    };

    // Send device selection event
    if let Err(e) = state
        .event_tx
        .send(DeviceSelected {
            device_uuid: uuid,
            session_info,
        })
        .await
    {
        error!("Failed to send device selected event: {}", e);
        return Ok(Json(ConnectResponse {
            success: false,
            error: Some("Internal error".to_string()),
        }));
    }

    Ok(Json(ConnectResponse {
        success: true,
        error: None,
    }))
}

// ============================================================================
// Device Registry
// ============================================================================

/// Re-announcement interval for mDNS services.
/// Sends periodic announcements to keep devices visible in Qobuz app.
const REANNOUNCE_INTERVAL: Duration = Duration::from_secs(5);

/// Registry for Qobuz Connect devices.
///
/// Manages mDNS announcements and HTTP endpoints for multiple devices.
/// This is an internal type; use `SessionManager` for the public API.
pub struct DeviceRegistry {
    state: Arc<AppState>,
    http_port: u16,
    /// The mDNS daemon shared across all devices.
    mdns_daemon: ServiceDaemon,
}

impl DeviceRegistry {
    /// Start the device registry with HTTP server on the given port.
    ///
    /// Returns the registry and a receiver for device selection events.
    pub async fn start(port: u16) -> Result<(Self, mpsc::Receiver<DeviceSelected>)> {
        let (event_tx, event_rx) = mpsc::channel(16);

        let state = Arc::new(AppState {
            devices: RwLock::new(HashMap::new()),
            event_tx,
        });

        // Bind HTTP server
        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|e| Error::Discovery(format!("Failed to bind: {e}")))?;

        let local_addr = listener
            .local_addr()
            .map_err(|e| Error::Discovery(format!("Failed to get local addr: {e}")))?;

        info!("Device registry HTTP server listening on {}", local_addr);

        // Build router with parameterized routes
        let app = Router::new()
            .route("/devices/{uuid}/get-display-info", get(get_display_info))
            .route("/devices/{uuid}/get-connect-info", get(get_connect_info))
            .route(
                "/devices/{uuid}/connect-to-qconnect",
                post(connect_to_qconnect),
            )
            .with_state(state.clone());

        // Spawn HTTP server task
        tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                error!("HTTP server error: {}", e);
            }
        });

        // Create the mDNS daemon
        let mdns_daemon = ServiceDaemon::new()
            .map_err(|e| Error::Mdns(format!("Failed to create mDNS daemon: {e}")))?;

        info!("mDNS daemon started");

        // Spawn periodic re-announcement task
        let state_clone = state.clone();
        let daemon_clone = mdns_daemon.clone();
        tokio::spawn(async move {
            Self::reannounce_loop(state_clone, daemon_clone).await;
        });

        Ok((
            Self {
                state,
                http_port: local_addr.port(),
                mdns_daemon,
            },
            event_rx,
        ))
    }

    /// Periodically re-announce all registered services.
    /// This works around a bug in the Qobuz app where it only processes
    /// the first PTR record in batched mDNS responses.
    async fn reannounce_loop(state: Arc<AppState>, daemon: ServiceDaemon) {
        loop {
            tokio::time::sleep(REANNOUNCE_INTERVAL).await;

            let devices = state.devices.read().await;
            for device in devices.values() {
                if let Err(e) = daemon.register(device.service_info.clone()) {
                    debug!(
                        error = %e,
                        device = %device.config.friendly_name,
                        "Failed to re-announce service"
                    );
                }
            }
        }
    }

    /// Register a device for discovery.
    ///
    /// Starts mDNS announcement for the device and returns a `DeviceSession`
    /// handle for bidirectional communication with the session.
    pub async fn add_device(&self, config: DeviceConfig) -> Result<DeviceSession> {
        let uuid = config.device_uuid;
        let uuid_str = config.uuid_formatted();

        info!(
            device = %config.friendly_name,
            uuid = %uuid_str,
            "Registering device"
        );

        // Create per-device event channel (server -> user)
        let (event_tx, event_rx) = mpsc::channel(100);

        // Create shared command sender (will be populated when session connects)
        let command_tx: SharedCommandTx = Arc::new(RwLock::new(None));

        // Register mDNS service
        let service_info = self.register_mdns(&config, &uuid_str)?;

        let registered = RegisteredDevice {
            config,
            service_info,
            current_session_id: None,
            event_tx,
            command_tx: command_tx.clone(),
        };

        self.state.devices.write().await.insert(uuid, registered);

        Ok(DeviceSession::new(event_rx, command_tx))
    }

    /// Unregister a device.
    ///
    /// Stops mDNS announcement.
    pub async fn remove_device(&self, device_uuid: &[u8; 16]) -> Result<()> {
        let mut devices = self.state.devices.write().await;

        if let Some(device) = devices.remove(device_uuid) {
            info!(
                device = %device.config.friendly_name,
                "Unregistering device"
            );
            // Unregister from mDNS daemon (sends goodbye packet asynchronously)
            if let Err(e) = self
                .mdns_daemon
                .unregister(device.service_info.get_fullname())
            {
                warn!(error = %e, "Failed to unregister mDNS service");
            }
        } else {
            warn!(uuid = %Uuid::from_bytes(*device_uuid), "Device not found for removal");
        }

        Ok(())
    }

    /// Get a device configuration by UUID.
    pub async fn get_device(&self, device_uuid: &[u8; 16]) -> Option<DeviceConfig> {
        self.state
            .devices
            .read()
            .await
            .get(device_uuid)
            .map(|d| d.config.clone())
    }

    /// Get the event sender for a device by UUID.
    pub async fn get_event_tx(&self, device_uuid: &[u8; 16]) -> Option<mpsc::Sender<SessionEvent>> {
        self.state
            .devices
            .read()
            .await
            .get(device_uuid)
            .map(|d| d.event_tx.clone())
    }

    /// Get the shared command sender for a device by UUID.
    ///
    /// Returns a clone of the Arc, which can be used to set the sender
    /// when a session starts.
    pub async fn get_command_tx(&self, device_uuid: &[u8; 16]) -> Option<SharedCommandTx> {
        self.state
            .devices
            .read()
            .await
            .get(device_uuid)
            .map(|d| d.command_tx.clone())
    }

    /// Get all registered device configurations.
    pub async fn devices(&self) -> Vec<DeviceConfig> {
        self.state
            .devices
            .read()
            .await
            .values()
            .map(|d| d.config.clone())
            .collect()
    }

    /// Register mDNS service for a device.
    fn register_mdns(&self, config: &DeviceConfig, uuid_hex: &str) -> Result<ServiceInfo> {
        let service_type = "_qobuz-connect._tcp.local.";
        let instance_name = &config.friendly_name;

        // Get local IP and hostname
        let ipv4 = get_local_ipv4().unwrap_or(Ipv4Addr::LOCALHOST);
        let ip_str = ipv4.to_string();
        let hostname = "qonductor.local.";

        // Build TXT record properties
        let properties = [
            ("path", format!("/devices/{}", uuid_hex)),
            ("type", config.device_type.as_str().to_string()),
            ("Name", config.friendly_name.clone()),
            ("device_uuid", uuid_hex.to_string()),
            ("sdk_version", QCONNECT_SDK_VERSION.to_string()),
        ];

        let service_info = ServiceInfo::new(
            service_type,
            instance_name,
            hostname,
            &ip_str,
            self.http_port,
            &properties[..],
        )
        .map_err(|e| Error::Mdns(format!("Failed to create service info: {e}")))?;

        info!(
            device = %config.friendly_name,
            port = self.http_port,
            "Registering mDNS service"
        );

        self.mdns_daemon
            .register(service_info.clone())
            .map_err(|e| Error::Mdns(format!("Failed to register mDNS service: {e}")))?;

        info!(device = %config.friendly_name, "mDNS service registered");

        Ok(service_info)
    }

    /// Gracefully shutdown the registry, unregistering all mDNS services.
    ///
    /// This sends mDNS goodbye packets so devices disappear from controllers.
    /// Call this before dropping the registry for clean shutdown.
    pub async fn shutdown(&self) {
        info!("Shutting down device registry");

        let devices = self.state.devices.read().await;
        for device in devices.values() {
            info!(device = %device.config.friendly_name, "Unregistering mDNS service");
            if let Ok(receiver) = self
                .mdns_daemon
                .unregister(device.service_info.get_fullname())
            {
                // Wait for goodbye packet to be sent
                let _ = receiver.recv_timeout(Duration::from_millis(200));
            }
        }
        drop(devices);

        // Shutdown the daemon
        if let Ok(receiver) = self.mdns_daemon.shutdown() {
            let _ = receiver.recv_timeout(Duration::from_secs(1));
        }
        info!("Device registry shutdown complete");
    }
}

/// Get the local IPv4 address (non-loopback).
fn get_local_ipv4() -> Option<Ipv4Addr> {
    use std::net::UdpSocket;
    // Connect to a public address to determine local IP (doesn't actually send data)
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let addr = socket.local_addr().ok()?;
    match addr.ip() {
        std::net::IpAddr::V4(ip) => Some(ip),
        _ => None,
    }
}
