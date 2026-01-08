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
    msg, ActivationState, BufferState, Command, DeviceConfig, DeviceSession, Notification,
    PlayingState, SessionEvent, SessionManager,
    msg::{PositionExt, QueueRendererStateExt, SetStateExt, LoopModeSetExt},
};
use serde::Deserialize;
use std::fs;
use tokio::signal;
use tracing::debug;
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
                Command::SetState { cmd, respond } => {
                    let position_ms = cmd.current_position;
                    let queue_item_id = cmd.current_queue_item.as_ref().and_then(|q| q.queue_item_id);

                    println!(
                        "[{}] Playback command: state={:?} position={:?} queue_item={:?}",
                        device_name, cmd.state(), position_ms, queue_item_id
                    );

                    let mut response = msg::QueueRendererState {
                        current_position: Some(msg::Position::now(position_ms.unwrap_or(0))),
                        ..Default::default()
                    };
                    response
                        .set_state(cmd.state().unwrap_or(PlayingState::Stopped))
                        .set_buffer(BufferState::Ok);
                    respond.send(response);
                }

                Command::SetActive { cmd: _, respond } => {
                    println!("[{}] Device activated", device_name);
                    let mut playback = msg::QueueRendererState {
                        current_position: Some(msg::Position::now(0)),
                        ..Default::default()
                    };
                    playback.set_state(PlayingState::Stopped).set_buffer(BufferState::Ok);
                    respond.send(ActivationState {
                        muted: false,
                        volume: 100,
                        max_quality: 4,
                        playback,
                    });
                }

                Command::Heartbeat { respond, .. } => {
                    respond.send(None); // Not playing, no heartbeat needed
                }
            },

            // === Notifications (informational) ===
            SessionEvent::Notification(n) => match n {
                Notification::QueueState(queue) => {
                    let version = queue
                        .queue_version
                        .map(|v| (v.major.unwrap_or(0), v.minor.unwrap_or(0)))
                        .unwrap_or((0, 0));
                    println!(
                        "[{}] Queue updated: {} tracks, version={}.{}",
                        device_name,
                        queue.tracks.len(),
                        version.0,
                        version.1
                    );
                }

                Notification::QueueTracksAdded(added) => {
                    println!(
                        "[{}] Queue tracks added: {} tracks",
                        device_name,
                        added.tracks.len()
                    );
                }

                Notification::QueueTracksInserted(inserted) => {
                    println!(
                        "[{}] Queue tracks inserted: {} tracks at {:?}",
                        device_name,
                        inserted.tracks.len(),
                        inserted.insert_after
                    );
                }

                Notification::QueueTracksRemoved(removed) => {
                    println!(
                        "[{}] Queue tracks removed: {} items",
                        device_name,
                        removed.queue_item_ids.len()
                    );
                }

                Notification::QueueTracksReordered(reordered) => {
                    println!(
                        "[{}] Queue tracks reordered: {} items",
                        device_name,
                        reordered.queue_item_ids.len()
                    );
                }

                Notification::LoopModeSet(lm) => {
                    println!("[{}] Loop mode changed: {:?}", device_name, lm.loop_mode());
                }

                Notification::ShuffleModeSet(sm) => {
                    println!(
                        "[{}] Shuffle mode changed: {}",
                        device_name,
                        sm.shuffle_on.unwrap_or(false)
                    );
                }

                Notification::Deactivated => {
                    println!("[{}] Device deactivated", device_name);
                }

                Notification::RestoreState(rsu) => {
                    let position_ms = rsu
                        .state
                        .as_ref()
                        .and_then(|s| s.current_position.as_ref())
                        .and_then(|p| p.value)
                        .unwrap_or(0);
                    let queue_index = rsu
                        .state
                        .as_ref()
                        .and_then(|s| s.current_queue_index);
                    println!(
                        "[{}] Restore state: position={}ms queue_idx={:?}",
                        device_name, position_ms, queue_index
                    );
                }

                Notification::Connected => {
                    println!("[{}] WebSocket connected!", device_name);
                }

                Notification::Disconnected { session_id, reason } => {
                    println!(
                        "[{}] Disconnected (session {}): {:?}",
                        device_name, session_id, reason
                    );
                }

                Notification::DeviceRegistered {
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

                Notification::SessionClosed { device_uuid } => {
                    println!(
                        "[{}] Session closed: uuid={}",
                        device_name,
                        Uuid::from_bytes(device_uuid)
                    );
                }

                other => {
                    debug!("[{}] Unhandled notification: {:?}", device_name, other);
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

    let mut device_handles = Vec::new();
    for name in &devices {
        let config = DeviceConfig::new(*name, &creds.app_id);
        let session = manager.add_device(config).await.unwrap();
        println!("Registered device: {}", name);

        // Spawn a task to handle events for this device
        let device_name = name.to_string();
        device_handles.push(tokio::spawn(handle_device_events(device_name, session)));
    }

    println!("\nDiscovery server running. Press Ctrl+C to stop.");
    println!("Open the Qobuz app and you should see these devices in the Connect menu.\n");

    // Run manager until Ctrl+C
    tokio::select! {
        result = manager.run() => {
            if let Err(e) = result {
                eprintln!("Manager error: {e}");
            }
        }
        _ = signal::ctrl_c() => {
            println!("\nShutting down...");
        }
    }

    // Clean shutdown: unregister mDNS services first
    manager.shutdown().await;

    for handle in device_handles {
        handle.abort();
    }
}
