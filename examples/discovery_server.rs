//! Run the Qobuz Connect discovery server with multiple devices.
//!
//! This example starts an mDNS service and HTTP endpoints that allow
//! Qobuz controllers to discover and connect to multiple devices.
//!
//! Setup:
//!   1. Copy credentials.toml.example to credentials.toml
//!   2. Fill in your app_id and app_secret
//!
//! Run with: cargo run --example discovery_server
//! Run with debug: RUST_LOG=qonductor=debug cargo run --example discovery_server

use qonductor::{
    ActivationState, AudioQuality, BufferState, DeviceConfig, LoopMode, PlaybackCommand,
    PlaybackResponse, PlayingState, QueueTrack, RendererBroadcast, RendererHandler,
    SessionManager,
};
use serde::Deserialize;
use std::fs;
use tokio::signal;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[derive(Deserialize)]
struct Credentials {
    app_id: String,
    #[allow(dead_code)]
    app_secret: String,
}

/// Minimal handler that just logs events (for discovery testing).
struct LoggingHandler;

impl RendererHandler for LoggingHandler {
    fn on_playback_command(&mut self, renderer_id: u64, cmd: PlaybackCommand) -> PlaybackResponse {
        println!(
            "Playback command: renderer={} state={:?} position={:?} queue_item={:?}",
            renderer_id, cmd.state, cmd.position_ms, cmd.queue_item_id
        );
        PlaybackResponse {
            state: cmd.state,
            buffer_state: BufferState::Ok,
            position_ms: cmd.position_ms.unwrap_or(0),
            duration_ms: None,
            queue_item_id: None,
            next_queue_item_id: None,
        }
    }

    fn on_volume_command(&mut self, renderer_id: u64, volume: u32) -> u32 {
        println!("Volume command: renderer={} volume={}", renderer_id, volume);
        volume
    }

    fn on_activate(&mut self, renderer_id: u64) -> ActivationState {
        println!("Device activated: renderer_id={}", renderer_id);
        ActivationState {
            muted: false,
            volume: 100,
            max_quality: AudioQuality::FlacHiRes192,
            playback: PlaybackResponse {
                state: PlayingState::Stopped,
                buffer_state: BufferState::Ok,
                position_ms: 0,
                duration_ms: None,
                queue_item_id: None,
                next_queue_item_id: None,
            },
        }
    }

    fn on_deactivate(&mut self, renderer_id: u64) {
        println!("Device deactivated: renderer_id={}", renderer_id);
    }

    fn on_queue_update(&mut self, tracks: Vec<QueueTrack>, version: (u64, i32)) {
        println!(
            "Queue updated: {} tracks, version={}.{}",
            tracks.len(),
            version.0,
            version.1
        );
    }

    fn on_loop_mode(&mut self, mode: LoopMode) {
        println!("Loop mode changed: {:?}", mode);
    }

    fn on_shuffle_mode(&mut self, enabled: bool) {
        println!("Shuffle mode changed: {}", enabled);
    }

    fn on_heartbeat(&mut self, _renderer_id: u64) -> Option<PlaybackResponse> {
        None // Not playing, no heartbeat needed
    }
}

#[tokio::main]
async fn main() {
    // Initialize tracing from RUST_LOG env var
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    println!("Loading credentials from credentials.toml...");

    let contents = match fs::read_to_string("credentials.toml") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to read credentials.toml: {e}");
            eprintln!(
                "Hint: Copy credentials.toml.example to credentials.toml and fill in your credentials"
            );
            std::process::exit(1);
        }
    };

    let creds: Credentials = match toml::from_str(&contents) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to parse credentials.toml: {e}");
            std::process::exit(1);
        }
    };

    println!("Starting discovery server...");

    // Create handler for all devices
    let handler = LoggingHandler;

    // Start the session manager with handler
    let (mut manager, mut broadcasts) = SessionManager::start(7864, handler).await.unwrap();

    // Register multiple devices
    let devices = vec!["Living Room Speaker", "Kitchen Speaker", "Bedroom Speaker"];

    for name in &devices {
        let config = DeviceConfig::new(*name, &creds.app_id);
        manager.add_device(config).await.unwrap();
        println!("Registered device: {}", name);
    }

    println!("\nDiscovery server running. Press Ctrl+C to stop.");
    println!("Open the Qobuz app and you should see these devices in the Connect menu.\n");

    // Spawn manager in background
    let manager_handle = tokio::spawn(async move {
        if let Err(e) = manager.run().await {
            eprintln!("Manager error: {e}");
        }
    });

    // Handle broadcasts
    loop {
        tokio::select! {
            Some(broadcast) = broadcasts.recv() => {
                match broadcast {
                    RendererBroadcast::Connected => {
                        println!("WebSocket connected!");
                    }
                    RendererBroadcast::Disconnected { session_id, reason } => {
                        println!("Disconnected (session {}): {:?}", session_id, reason);
                    }
                    RendererBroadcast::DeviceRegistered { device_uuid, renderer_id } => {
                        println!(
                            "Device registered: uuid={} renderer_id={}",
                            Uuid::from_bytes(device_uuid),
                            renderer_id
                        );
                    }
                    RendererBroadcast::RendererAdded { renderer_id, name } => {
                        println!("Other renderer added: {} (id={})", name, renderer_id);
                    }
                    RendererBroadcast::RendererRemoved { renderer_id } => {
                        println!("Renderer removed: id={}", renderer_id);
                    }
                    RendererBroadcast::ActiveRendererChanged { renderer_id } => {
                        println!("Active renderer changed to id={}", renderer_id);
                    }
                    RendererBroadcast::RendererStateUpdated { renderer_id, state, position_ms, .. } => {
                        println!("Renderer state: id={} state={:?} pos={}ms", renderer_id, state, position_ms);
                    }
                    RendererBroadcast::VolumeBroadcast { renderer_id, volume } => {
                        println!("Volume broadcast: renderer={} volume={}", renderer_id, volume);
                    }
                    RendererBroadcast::VolumeMutedBroadcast { renderer_id, muted } => {
                        println!("Mute broadcast: renderer={} muted={}", renderer_id, muted);
                    }
                    RendererBroadcast::MaxAudioQualityBroadcast { renderer_id, quality } => {
                        println!("Max quality broadcast: renderer={} quality={:?}", renderer_id, quality);
                    }
                    RendererBroadcast::FileAudioQualityBroadcast { renderer_id, sample_rate_hz } => {
                        println!("File quality broadcast: renderer={} rate={}Hz", renderer_id, sample_rate_hz);
                    }
                }
            }
            _ = signal::ctrl_c() => {
                println!("\nShutting down...");
                break;
            }
        }
    }

    manager_handle.abort();
}
