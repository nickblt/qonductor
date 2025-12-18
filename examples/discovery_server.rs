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

use qonductor::{DeviceConfig, SessionEvent, SessionManager};
use serde::Deserialize;
use std::fs;
use tokio::signal;
use uuid::Uuid;
use tracing_subscriber::EnvFilter;

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

    println!("Got app_id: {}", creds.app_id);

    // Start the session manager
    let (mut manager, mut events) = match SessionManager::start(7864).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Failed to start session manager: {e}");
            std::process::exit(1);
        }
    };

    // Register a device
    let device = DeviceConfig::new("Qonductor Test Device", &creds.app_id);
    println!("Device UUID: {}", device.uuid_formatted());

    if let Err(e) = manager.add_device(device).await {
        eprintln!("Failed to add device: {e}");
        std::process::exit(1);
    }

    println!();
    println!("Discovery server running!");
    println!("  HTTP endpoints available at http://0.0.0.0:7864");
    println!("  mDNS service: Qonductor Test Device._qobuz-connect._tcp.local.");
    println!();
    println!("Waiting for controller to connect... (Press Ctrl+C to exit)");
    println!();

    // Spawn manager to handle device selections and session management
    tokio::spawn(async move {
        if let Err(e) = manager.run().await {
            eprintln!("Session manager error: {e}");
        }
    });

    // Handle events
    loop {
        tokio::select! {
            Some(event) = events.recv() => {
                match event {
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
                    SessionEvent::DeviceActive { renderer_id, .. } => {
                        println!("Our device is now active! renderer_id={}", renderer_id);
                    }
                    SessionEvent::DeviceDeactivated { renderer_id, new_active_renderer_id, .. } => {
                        println!(
                            "Our device deactivated (was renderer_id={}), new active={}",
                            renderer_id, new_active_renderer_id
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
                    SessionEvent::QueueUpdated { tracks, version } => {
                        println!(
                            "Queue updated: {} tracks, version={}.{}",
                            tracks.len(),
                            version.0,
                            version.1
                        );
                    }
                    SessionEvent::PlaybackCommand { renderer_id, state, position_ms } => {
                        println!(
                            "Playback command: renderer={} state={:?} position={:?}",
                            renderer_id, state, position_ms
                        );
                        // TODO: Actually control playback
                    }
                    SessionEvent::VolumeCommand { renderer_id, volume } => {
                        println!("Volume command: renderer={} volume={}", renderer_id, volume);
                    }
                    SessionEvent::LoopModeChanged { mode } => {
                        println!("Loop mode changed: {:?}", mode);
                    }
                    SessionEvent::ShuffleModeChanged { enabled } => {
                        println!("Shuffle mode changed: {}", enabled);
                    }
                }
            }
            _ = signal::ctrl_c() => {
                println!("\nShutting down...");
                break;
            }
        }
    }
}
