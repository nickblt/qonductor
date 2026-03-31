//! Connect directly to Qobuz Connect using a WebSocket JWT.
//!
//! This example bypasses mDNS/HTTP discovery and connects a device
//! directly via WebSocket. Useful when you already have credentials
//! from the Qobuz API.
//!
//! Run with: QOBUZ_WS_JWT=xxx cargo run --example direct_auth
//! Run with debug: RUST_LOG=qonductor=debug QOBUZ_WS_JWT=xxx cargo run --example direct_auth

use qonductor::{
    ActivationState, BufferState, Command, DeviceConfig, Notification, PlayingState, SessionEvent,
    msg,
    msg::{LoopModeSetExt, PositionExt, QueueRendererStateExt, SetStateExt, report::VolumeChanged},
};
use std::env;
use tokio::signal;
use tracing::debug;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let ws_jwt = match env::var("QOBUZ_WS_JWT") {
        Ok(jwt) => jwt,
        Err(_) => {
            eprintln!("QOBUZ_WS_JWT environment variable is not set");
            std::process::exit(1);
        }
    };

    let device_name = "Direct Auth Example";
    let config = DeviceConfig::new(device_name).auto_activate(false);

    let ws_endpoint = env::var("QOBUZ_WS_ENDPOINT")
        .unwrap_or_else(|_| "wss://qws-us-prod.qobuz.com/ws".to_string());

    println!("Connecting directly to Qobuz Connect at {ws_endpoint}...");

    let mut session = match qonductor::connect(&ws_endpoint, &ws_jwt, &config).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to connect: {e}");
            std::process::exit(1);
        }
    };

    println!("Connected! Waiting for events. Press Ctrl+C to stop.\n");

    tokio::select! {
        _ = async {
            while let Some(event) = session.recv().await {
                match event {
                    SessionEvent::Command(cmd) => match cmd {
                        Command::SetState { cmd, respond } => {
                            let position_ms = cmd.current_position;
                            let queue_item_id = cmd.current_queue_item.as_ref().map(|q| q.queue_item_id);

                            println!(
                                "[{}] Playback command: state={:?} position={:?} queue_item={:?}",
                                device_name,
                                cmd.state(),
                                position_ms,
                                queue_item_id
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
                            playback
                                .set_state(PlayingState::Stopped)
                                .set_buffer(BufferState::Ok);
                            respond.send(ActivationState {
                                muted: false,
                                volume: 100,
                                max_quality: 4,
                                playback,
                            });
                        }

                        Command::SetVolume { cmd, respond } => {
                            println!("[{}] Volume changed: {:?}", device_name, cmd.volume);
                            respond.send(VolumeChanged { volume: cmd.volume });
                        }

                        Command::Heartbeat { respond, .. } => {
                            respond.send(None);
                        }
                    },

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
                            let queue_index = rsu.state.as_ref().and_then(|s| s.current_queue_index);
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
        } => {}
        _ = signal::ctrl_c() => {
            println!("\nShutting down...");
        }
    }
}
