//! mDNS service discovery and HTTP endpoint handlers for Qobuz Connect.
//!
//! This module provides service advertisement via mDNS/Zeroconf and HTTP endpoints
//! that allow Qobuz controllers to discover and connect to devices.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, error, info, warn};
use uuid::Uuid;
use zeroconf_tokio::prelude::*;
use zeroconf_tokio::{MdnsService, MdnsServiceAsync, ServiceType, TxtRecord};

use crate::config::{AudioQuality, DeviceConfig, SessionInfo};
use crate::proto::qconnect::DeviceType;
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
    mdns_handle: MdnsServiceAsync,
    current_session_id: Option<String>,
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

/// Registry for Qobuz Connect devices.
///
/// Manages mDNS announcements and HTTP endpoints for multiple devices.
/// This is an internal type; use `SessionManager` for the public API.
pub struct DeviceRegistry {
    state: Arc<AppState>,
    http_port: u16,
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

        Ok((
            Self {
                state,
                http_port: local_addr.port(),
            },
            event_rx,
        ))
    }

    /// Register a device for discovery.
    ///
    /// Starts mDNS announcement for the device.
    pub async fn add_device(&self, config: DeviceConfig) -> Result<()> {
        let uuid = config.device_uuid;
        let uuid_str = config.uuid_formatted();

        info!(
            device = %config.friendly_name,
            uuid = %uuid_str,
            "Registering device"
        );

        // Register mDNS service
        let mdns_handle = self.register_mdns(&config, &uuid_str).await?;

        let registered = RegisteredDevice {
            config,
            mdns_handle,
            current_session_id: None,
        };

        self.state.devices.write().await.insert(uuid, registered);

        Ok(())
    }

    /// Unregister a device.
    ///
    /// Stops mDNS announcement.
    pub async fn remove_device(&self, device_uuid: &[u8; 16]) -> Result<()> {
        let mut devices = self.state.devices.write().await;

        if let Some(mut device) = devices.remove(device_uuid) {
            info!(
                device = %device.config.friendly_name,
                "Unregistering device"
            );
            if let Err(e) = device.mdns_handle.shutdown().await {
                warn!(error = %e, "Failed to shutdown mDNS service");
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
    async fn register_mdns(&self, config: &DeviceConfig, uuid_hex: &str) -> Result<MdnsServiceAsync> {
        let service_type = ServiceType::new("qobuz-connect", "tcp")
            .map_err(|e| Error::Mdns(format!("Failed to create service type: {e}")))?;

        let mut service = MdnsService::new(service_type, self.http_port);
        service.set_name(&config.friendly_name);

        // Set TXT records with device-specific path
        let mut txt = TxtRecord::new();
        txt.insert("path", &format!("/devices/{}", uuid_hex))
            .map_err(|e| Error::Mdns(format!("Failed to insert TXT record: {e}")))?;
        txt.insert("type", config.device_type.as_str())
            .map_err(|e| Error::Mdns(format!("Failed to insert TXT record: {e}")))?;
        txt.insert("Name", &config.friendly_name)
            .map_err(|e| Error::Mdns(format!("Failed to insert TXT record: {e}")))?;
        txt.insert("device_uuid", uuid_hex)
            .map_err(|e| Error::Mdns(format!("Failed to insert TXT record: {e}")))?;
        txt.insert("sdk_version", QCONNECT_SDK_VERSION)
            .map_err(|e| Error::Mdns(format!("Failed to insert TXT record: {e}")))?;
        service.set_txt_record(txt);

        info!(
            device = %config.friendly_name,
            port = self.http_port,
            "Registering mDNS service"
        );

        let mut service_async = MdnsServiceAsync::new(service)
            .map_err(|e| Error::Mdns(format!("Failed to create async service: {e}")))?;

        service_async
            .start_with_timeout(std::time::Duration::from_millis(100))
            .await
            .map_err(|e| Error::Mdns(format!("Failed to start service: {e}")))?;

        info!(device = %config.friendly_name, "mDNS service registered");

        Ok(service_async)
    }
}

