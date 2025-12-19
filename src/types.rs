//! Core data types for qonductor.

use serde::{Deserialize, Serialize};

/// Audio format/quality level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum AudioFormat {
    Mp3 = 5,
    FlacLossless = 6,
    FlacHiRes96 = 7,
    FlacHiRes192 = 27,
}

/// Artist metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artist {
    pub id: u64,
    pub name: String,
}

/// Album metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Album {
    pub id: String,
    pub title: String,
    pub artist: Artist,
    #[serde(default)]
    pub image_url: Option<String>,
}

/// Track metadata and streaming info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: u64,
    pub title: String,
    pub artist: Artist,
    pub album: Album,
    pub duration_ms: u64,
    #[serde(default)]
    pub format: Option<AudioFormat>,
    #[serde(default)]
    pub file_url: Option<String>,
}
