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
    ActivationState, BufferState, DeviceConfig, PlaybackResponse, PlayingState, SessionEvent,
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

    // Start the session manager
    let (mut manager, mut events) = SessionManager::start(7864).await.unwrap();

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

    // Handle events
    loop {
        tokio::select! {
            Some(event) = events.recv() => {
                match event {
                    // === Commands (require response) ===

                    SessionEvent::PlaybackCommand { renderer_id, cmd, respond } => {
                        println!(
                            "Playback command: renderer={} state={:?} position={:?} queue_item={:?}",
                            renderer_id, cmd.state, cmd.position_ms, cmd.queue_item_id
                        );
                        respond.send(PlaybackResponse {
                            state: cmd.state.unwrap_or(PlayingState::Stopped),
                            buffer_state: BufferState::Ok,
                            position_ms: cmd.position_ms.unwrap_or(0),
                            duration_ms: None,
                            queue_item_id: None,
                            next_queue_item_id: None,
                        });
                    }

                    SessionEvent::Activate { renderer_id, respond } => {
                        println!("Device activated: renderer_id={}", renderer_id);
                        respond.send(ActivationState {
                            muted: false,
                            volume: 100,
                            max_quality: 4, // HiRes 192kHz capability level
                            playback: PlaybackResponse {
                                state: PlayingState::Stopped,
                                buffer_state: BufferState::Ok,
                                position_ms: 0,
                                duration_ms: None,
                                queue_item_id: None,
                                next_queue_item_id: None,
                            },
                        });
                    }

                    SessionEvent::Heartbeat { respond, .. } => {
                        respond.send(None); // Not playing, no heartbeat needed
                    }

                    // === Events (no response needed) ===

                    SessionEvent::Deactivated { renderer_id } => {
                        println!("Device deactivated: renderer_id={}", renderer_id);
                    }

                    SessionEvent::QueueUpdated { tracks, version } => {
                        println!(
                            "Queue updated: {} tracks, version={}.{}",
                            tracks.len(),
                            version.0,
                            version.1
                        );
                    }

                    SessionEvent::LoopModeChanged { mode } => {
                        println!("Loop mode changed: {:?}", mode);
                    }

                    SessionEvent::ShuffleModeChanged { enabled } => {
                        println!("Shuffle mode changed: {}", enabled);
                    }

                    SessionEvent::RestoreState { position_ms, queue_index } => {
                        println!("Restore state: position={}ms queue_idx={:?}", position_ms, queue_index);
                    }

                    // === Broadcasts (informational) ===

                    SessionEvent::Connected => {
                        println!("WebSocket connected!");
                    }

                    SessionEvent::Disconnected { session_id, reason } => {
                        println!("Disconnected (session {}): {:?}", session_id, reason);
                    }

                    SessionEvent::DeviceRegistered { device_uuid, renderer_id } => {
                        println!(
                            "Device registered: uuid={} renderer_id={}",
                            Uuid::from_bytes(device_uuid),
                            renderer_id
                        );
                    }

                    SessionEvent::RendererAdded { renderer_id, name } => {
                        println!("Other renderer added: {} (id={})", name, renderer_id);
                    }

                    SessionEvent::RendererRemoved { renderer_id } => {
                        println!("Renderer removed: id={}", renderer_id);
                    }

                    SessionEvent::ActiveRendererChanged { renderer_id } => {
                        println!("Active renderer changed to id={}", renderer_id);
                    }

                    SessionEvent::RendererStateUpdated { renderer_id, state, position_ms, .. } => {
                        println!("Renderer state: id={} state={:?} pos={}ms", renderer_id, state, position_ms);
                    }

                    SessionEvent::VolumeBroadcast { renderer_id, volume } => {
                        println!("Volume broadcast: renderer={} volume={}", renderer_id, volume);
                    }

                    SessionEvent::VolumeMutedBroadcast { renderer_id, muted } => {
                        println!("Mute broadcast: renderer={} muted={}", renderer_id, muted);
                    }

                    SessionEvent::MaxAudioQualityBroadcast { renderer_id, quality } => {
                        println!("Max quality broadcast: renderer={} quality={:?}", renderer_id, quality);
                    }

                    SessionEvent::FileAudioQualityBroadcast { renderer_id, sample_rate_hz } => {
                        println!("File quality broadcast: renderer={} rate={}Hz", renderer_id, sample_rate_hz);
                    }

                    SessionEvent::SessionClosed { device_uuid } => {
                        println!("Session closed: uuid={}", Uuid::from_bytes(device_uuid));
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
