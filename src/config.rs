//! Configuration types for Qonductor devices.

use md5::{Digest, Md5};
use uuid::Uuid;

use crate::proto::qconnect::DeviceType;

/// Audio quality capability levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum AudioQuality {
    /// MP3 quality.
    Mp3 = 1,
    /// FLAC lossless (44.1/48 kHz).
    FlacLossless = 2,
    /// Hi-Res up to 96 kHz.
    HiRes96 = 3,
    /// Hi-Res up to 192 kHz.
    HiRes192 = 4,
}

/// Configuration for a Qobuz Connect device.
#[derive(Debug, Clone)]
pub struct DeviceConfig {
    /// 16-byte unique device identifier.
    pub device_uuid: [u8; 16],
    /// Human-readable device name.
    pub friendly_name: String,
    /// Device type.
    pub device_type: DeviceType,
    /// Brand name for display.
    pub brand: String,
    /// Model name for display.
    pub model: String,
    /// Maximum audio quality capability level.
    pub max_audio_quality: AudioQuality,
    /// Whether to automatically declare as active renderer on registration.
    ///
    /// When `true` (the default), the device sends `SetActiveRenderer` as soon as
    /// the server confirms registration, taking over playback immediately. Set to
    /// `false` to just register and wait for the user to select this device in the
    /// Qobuz app.
    pub auto_activate: bool,
}

impl DeviceConfig {
    /// Create a new device configuration with a deterministic UUID based on the device name.
    ///
    /// This ensures the same device name always gets the same UUID, so the server
    /// recognizes it as the same device across restarts.
    pub fn new(friendly_name: impl Into<String>) -> Self {
        let name = friendly_name.into();
        // Generate deterministic UUID from device name using MD5
        let mut hasher = Md5::new();
        hasher.update(format!("qonductor:{}", name).as_bytes());
        let uuid: [u8; 16] = hasher.finalize().into();

        Self {
            device_uuid: uuid,
            friendly_name: name,
            device_type: DeviceType::Speaker,
            brand: "Qonductor".to_string(),
            model: "Qonductor Rust".to_string(),
            max_audio_quality: AudioQuality::HiRes192,
            auto_activate: true,
        }
    }

    /// Create a new device configuration with a specific UUID.
    pub fn with_uuid(device_uuid: [u8; 16], friendly_name: impl Into<String>) -> Self {
        Self {
            device_uuid,
            friendly_name: friendly_name.into(),
            device_type: DeviceType::Speaker,
            brand: "Qonductor".to_string(),
            model: "Qonductor Rust".to_string(),
            max_audio_quality: AudioQuality::HiRes192,
            auto_activate: true,
        }
    }

    /// Set whether to automatically declare as active renderer on registration.
    /// See [`auto_activate`](DeviceConfig::auto_activate) field for details.
    pub fn auto_activate(mut self, auto_activate: bool) -> Self {
        self.auto_activate = auto_activate;
        self
    }

    /// Returns the device UUID as a hex string (no dashes).
    pub fn uuid_hex(&self) -> String {
        Uuid::from_bytes(self.device_uuid).simple().to_string()
    }

    /// Returns the device UUID as a standard UUID string (8-4-4-4-12 format).
    pub fn uuid_formatted(&self) -> String {
        Uuid::from_bytes(self.device_uuid).hyphenated().to_string()
    }
}

/// Credentials for a direct WebSocket connection to Qobuz Connect.
///
/// Use [`ConnectCredentials::new()`] with the required WebSocket endpoint and JWT,
/// then optionally chain [`.api_jwt()`](ConnectCredentials::api_jwt) to provide
/// a Qobuz API token for streaming URL resolution.
///
/// ```ignore
/// let creds = ConnectCredentials::new("wss://qws-us-prod.qobuz.com/ws", ws_jwt)
///     .api_jwt(api_jwt);
/// qonductor::connect(&creds, &config).await?;
/// ```
#[derive(Debug, Clone)]
pub struct ConnectCredentials {
    /// WebSocket endpoint URL.
    pub ws_endpoint: String,
    /// JWT token for WebSocket authentication (required).
    pub ws_jwt: String,
    /// JWT token for Qobuz API calls (streaming URLs, etc.).
    pub api_jwt: Option<String>,
}

impl ConnectCredentials {
    /// Create credentials with the required WebSocket endpoint and JWT.
    pub fn new(ws_endpoint: impl Into<String>, ws_jwt: impl Into<String>) -> Self {
        Self {
            ws_endpoint: ws_endpoint.into(),
            ws_jwt: ws_jwt.into(),
            api_jwt: None,
        }
    }

    /// Set the API JWT for Qobuz API calls (streaming URLs, etc.).
    pub fn api_jwt(mut self, api_jwt: impl Into<String>) -> Self {
        self.api_jwt = Some(api_jwt.into());
        self
    }
}

