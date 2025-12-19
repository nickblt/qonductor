//! Configuration types for Qonductor devices.

use md5::{Digest, Md5};
use uuid::Uuid;

use crate::proto::qconnect::DeviceType;

/// Audio quality capability levels.
/// 1 = MP3, 2 = FLAC Lossless, 3 = HiRes 96kHz, 4 = HiRes 192kHz
pub mod audio_quality {
    pub const MP3: i32 = 1;
    pub const FLAC_LOSSLESS: i32 = 2;
    pub const HIRES_96: i32 = 3;
    pub const HIRES_192: i32 = 4;
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
    /// Maximum audio quality capability level (1-4).
    /// Use constants from `audio_quality` module.
    pub max_audio_quality: i32,
    /// Qobuz app_id (required for get-connect-info).
    pub app_id: String,
}

impl DeviceConfig {
    /// Create a new device configuration with a deterministic UUID based on the device name.
    ///
    /// This ensures the same device name always gets the same UUID, so the server
    /// recognizes it as the same device across restarts.
    pub fn new(friendly_name: impl Into<String>, app_id: impl Into<String>) -> Self {
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
            max_audio_quality: audio_quality::HIRES_192,
            app_id: app_id.into(),
        }
    }

    /// Create a new device configuration with a specific UUID.
    pub fn with_uuid(
        device_uuid: [u8; 16],
        friendly_name: impl Into<String>,
        app_id: impl Into<String>,
    ) -> Self {
        Self {
            device_uuid,
            friendly_name: friendly_name.into(),
            device_type: DeviceType::Speaker,
            brand: "Qonductor".to_string(),
            model: "Qonductor Rust".to_string(),
            max_audio_quality: audio_quality::HIRES_192,
            app_id: app_id.into(),
        }
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

/// Session information received from a Qobuz controller.
#[derive(Debug, Clone)]
pub(crate) struct SessionInfo {
    /// Session UUID.
    pub session_id: String,
    /// WebSocket endpoint URL.
    pub ws_endpoint: String,
    /// JWT token for WebSocket authentication.
    pub ws_jwt: String,
    /// JWT expiration timestamp (for future refresh logic).
    #[allow(dead_code)]
    pub ws_jwt_exp: u64,
    /// JWT token for API authentication (for streaming URLs).
    #[allow(dead_code)]
    pub api_jwt: String,
    /// API JWT expiration timestamp.
    #[allow(dead_code)]
    pub api_jwt_exp: u64,
}
