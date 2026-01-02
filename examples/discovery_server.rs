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
    ActivationState, BroadcastEvent, BufferState, CommandEvent, DeviceConfig, DeviceEvent,
    DeviceSession, PlaybackResponse, PlayingState, SessionEvent, SessionManager, SystemEvent,
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

/// Handle events for a single device.
async fn handle_device_events(device_name: String, mut session: DeviceSession) {
    while let Some(event) = session.recv().await {
        match event {
            // === Commands (require response) ===
            SessionEvent::Command(cmd) => match cmd {
                CommandEvent::PlaybackCommand {
                    renderer_id,
                    cmd,
                    respond,
                } => {
                    println!(
                        "[{}] Playback command: renderer={} state={:?} position={:?} queue_item={:?}",
                        device_name, renderer_id, cmd.state, cmd.position_ms, cmd.queue_item_id
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

                CommandEvent::Activate { renderer_id, respond } => {
                    println!("[{}] Device activated: renderer_id={}", device_name, renderer_id);
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

                CommandEvent::Heartbeat { respond, .. } => {
                    respond.send(None); // Not playing, no heartbeat needed
                }
            },

            // === Device events (no response needed) ===
            SessionEvent::Device(dev) => match dev {
                DeviceEvent::Deactivated { renderer_id } => {
                    println!("[{}] Device deactivated: renderer_id={}", device_name, renderer_id);
                }

                DeviceEvent::QueueUpdated { tracks, version } => {
                    println!(
                        "[{}] Queue updated: {} tracks, version={}.{}",
                        device_name,
                        tracks.len(),
                        version.0,
                        version.1
                    );
                }

                DeviceEvent::LoopModeChanged { mode } => {
                    println!("[{}] Loop mode changed: {:?}", device_name, mode);
                }

                DeviceEvent::ShuffleModeChanged { enabled } => {
                    println!("[{}] Shuffle mode changed: {}", device_name, enabled);
                }

                DeviceEvent::RestoreState {
                    position_ms,
                    queue_index,
                } => {
                    println!(
                        "[{}] Restore state: position={}ms queue_idx={:?}",
                        device_name, position_ms, queue_index
                    );
                }
            },

            // === System events ===
            SessionEvent::System(sys) => match sys {
                SystemEvent::Connected => {
                    println!("[{}] WebSocket connected!", device_name);
                }

                SystemEvent::Disconnected { session_id, reason } => {
                    println!(
                        "[{}] Disconnected (session {}): {:?}",
                        device_name, session_id, reason
                    );
                }

                SystemEvent::DeviceRegistered {
                    device_uuid,
                    renderer_id,
                    api_jwt,
                } => {
                    println!(
                        "[{}] Device registered: uuid={} renderer_id={} api_jwt={}",
                        device_name,
                        Uuid::from_bytes(device_uuid),
                        renderer_id,
                        api_jwt,
                    );
                }

                SystemEvent::SessionClosed { device_uuid } => {
                    println!(
                        "[{}] Session closed: uuid={}",
                        device_name,
                        Uuid::from_bytes(device_uuid)
                    );
                }
            },

            // === Broadcasts (from other renderers) ===
            SessionEvent::Broadcast(bc) => match bc {
                BroadcastEvent::RendererAdded { renderer_id, name } => {
                    println!(
                        "[{}] Other renderer added: {} (id={})",
                        device_name, name, renderer_id
                    );
                }

                BroadcastEvent::RendererRemoved { renderer_id } => {
                    println!("[{}] Renderer removed: id={}", device_name, renderer_id);
                }

                BroadcastEvent::ActiveRendererChanged { renderer_id } => {
                    println!(
                        "[{}] Active renderer changed to id={}",
                        device_name, renderer_id
                    );
                }

                BroadcastEvent::RendererStateUpdated {
                    renderer_id,
                    state,
                    position_ms,
                    ..
                } => {
                    println!(
                        "[{}] Renderer state: id={} state={:?} pos={}ms",
                        device_name, renderer_id, state, position_ms
                    );
                }

                BroadcastEvent::VolumeChanged { renderer_id, volume } => {
                    println!(
                        "[{}] Volume broadcast: renderer={} volume={}",
                        device_name, renderer_id, volume
                    );
                }

                BroadcastEvent::VolumeMuted { renderer_id, muted } => {
                    println!(
                        "[{}] Mute broadcast: renderer={} muted={}",
                        device_name, renderer_id, muted
                    );
                }

                BroadcastEvent::MaxAudioQualityChanged { renderer_id, quality } => {
                    println!(
                        "[{}] Max quality broadcast: renderer={} quality={:?}",
                        device_name, renderer_id, quality
                    );
                }

                BroadcastEvent::FileAudioQualityChanged {
                    renderer_id,
                    sample_rate_hz,
                } => {
                    println!(
                        "[{}] File quality broadcast: renderer={} rate={}Hz",
                        device_name, renderer_id, sample_rate_hz
                    );
                }
            },
        }
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

    // Start the session manager
    let mut manager = SessionManager::start(7864).await.unwrap();

    // Register multiple devices, each with its own event handler
    let devices = vec!["Living Room Speaker", "Kitchen Speaker", "Bedroom Speaker"];

    for name in &devices {
        let config = DeviceConfig::new(*name, &creds.app_id);
        let session = manager.add_device(config).await.unwrap();
        println!("Registered device: {}", name);

        // Spawn a task to handle events for this device
        let device_name = name.to_string();
        tokio::spawn(handle_device_events(device_name, session));
    }

    println!("\nDiscovery server running. Press Ctrl+C to stop.");
    println!("Open the Qobuz app and you should see these devices in the Connect menu.\n");

    // Spawn manager in background
    let manager_handle = tokio::spawn(async move {
        if let Err(e) = manager.run().await {
            eprintln!("Manager error: {e}");
        }
    });

    // Wait for shutdown signal
    signal::ctrl_c().await.unwrap();
    println!("\nShutting down...");

    manager_handle.abort();
}
