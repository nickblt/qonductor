//! Fake player example.
//!
//! Simulates a Qobuz Connect player using the stream-based event API.
//!
//! Run with: RUST_LOG=info cargo run --example fake_player
//! Run with debug: RUST_LOG=qonductor=debug,fake_player=debug cargo run --example fake_player

use qonductor::{
    msg, ActivationState, BufferState, Command, DeviceConfig, LoopMode, Notification, PlayingState,
    SessionEvent, SessionManager,
    msg::{PositionExt, QueueRendererStateExt, SetStateExt, LoopModeSetExt},
};
use rand::{thread_rng, Rng};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::time::Instant;
use tokio::signal;
use tracing::{debug, info, warn};

// ============================================================================
// Configuration
// ============================================================================

#[derive(Deserialize)]
struct Config {
    app_id: String,
}

fn load_config() -> Config {
    let config_str =
        fs::read_to_string("credentials.toml").expect("Failed to read credentials.toml");
    toml::from_str(&config_str).expect("Failed to parse credentials.toml")
}

// ============================================================================
// Track Duration Provider
// ============================================================================

trait TrackDurationProvider: Send {
    fn get_duration(&mut self, track_id: u64) -> u32;
}

struct RandomDurationProvider {
    cache: HashMap<u64, u32>,
}

impl RandomDurationProvider {
    fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }
}

impl TrackDurationProvider for RandomDurationProvider {
    fn get_duration(&mut self, track_id: u64) -> u32 {
        *self.cache.entry(track_id).or_insert_with(|| {
            let mut rng = thread_rng();
            rng.gen_range(60_000..360_000) // 1-6 minutes
        })
    }
}

// ============================================================================
// Fake Player
// ============================================================================

/// Simulates a Qobuz Connect player.
struct FakePlayer {
    // Playback state
    state: PlayingState,
    /// Position in ms at the time of `position_timestamp`
    position_at_timestamp: u32,
    /// When position_at_timestamp was recorded
    position_timestamp: Instant,
    current_track_index: Option<usize>,

    // Queue
    queue: Vec<msg::QueueTrackRef>,

    // Settings
    loop_mode: LoopMode,
    shuffle_enabled: bool,
    volume: u32,
    muted: bool,

    // Identity
    renderer_id: u64,

    // Duration provider
    duration_provider: Box<dyn TrackDurationProvider>,
}

impl FakePlayer {
    fn new() -> Self {
        Self {
            state: PlayingState::Stopped,
            position_at_timestamp: 0,
            position_timestamp: Instant::now(),
            current_track_index: None,
            queue: Vec::new(),
            loop_mode: LoopMode::Off,
            shuffle_enabled: false,
            volume: 100,
            muted: false,
            renderer_id: 0,
            duration_provider: Box::new(RandomDurationProvider::new()),
        }
    }

    /// Calculate current position based on elapsed time since last update.
    fn current_position_ms(&self) -> u32 {
        if self.state == PlayingState::Playing {
            let elapsed = self.position_timestamp.elapsed().as_millis() as u32;
            self.position_at_timestamp.saturating_add(elapsed)
        } else {
            self.position_at_timestamp
        }
    }

    /// Set position and record the timestamp.
    fn set_position(&mut self, position_ms: u32) {
        self.position_at_timestamp = position_ms;
        self.position_timestamp = Instant::now();
    }

    fn current_track(&self) -> Option<&msg::QueueTrackRef> {
        self.current_track_index.and_then(|i| self.queue.get(i))
    }

    fn current_queue_item_id(&self) -> Option<i32> {
        self.current_track()
            .and_then(|t| t.queue_item_id)
            .map(|id| id as i32)
    }

    fn current_duration(&mut self) -> Option<u32> {
        let track_id = self.current_track()?.track_id? as u64;
        Some(self.duration_provider.get_duration(track_id))
    }

    fn next_queue_item_id(&self) -> Option<i32> {
        let current_idx = self.current_track_index?;
        let next_idx = current_idx + 1;
        self.queue
            .get(next_idx)
            .and_then(|t| t.queue_item_id)
            .map(|id| id as i32)
    }

    fn renderer_state(&mut self) -> msg::QueueRendererState {
        let mut state = msg::QueueRendererState {
            current_position: Some(msg::Position::now(self.current_position_ms())),
            duration: self.current_duration(),
            current_queue_item_id: self.current_queue_item_id(),
            next_queue_item_id: self.next_queue_item_id(),
            ..Default::default()
        };
        state.set_state(self.state).set_buffer(BufferState::Ok);
        state
    }

    /// Check if track ended (called by heartbeat). Returns true if still playing.
    fn tick(&mut self) -> bool {
        if self.state != PlayingState::Playing {
            return false;
        }

        // Check if track ended using calculated position
        let current_pos = self.current_position_ms();
        if let Some(duration) = self.current_duration()
            && current_pos >= duration
        {
            return self.advance_track();
        }

        true // Still playing
    }

    fn advance_track(&mut self) -> bool {
        let queue_len = self.queue.len();
        if queue_len == 0 {
            self.state = PlayingState::Stopped;
            self.set_position(0);
            return true;
        }

        let current = self.current_track_index.unwrap_or(0);
        let next = match self.loop_mode {
            LoopMode::RepeatOne => current,
            LoopMode::RepeatAll => (current + 1) % queue_len,
            LoopMode::Off | LoopMode::Unknown => {
                if current + 1 >= queue_len {
                    self.state = PlayingState::Stopped;
                    self.set_position(0);
                    return true;
                }
                current + 1
            }
        };

        self.current_track_index = Some(next);
        self.set_position(0);

        if let Some(track) = self.queue.get(next) {
            info!(
                track_id = ?track.track_id,
                queue_item_id = ?track.queue_item_id,
                "Playing next track"
            );
        }

        true
    }

    fn find_track_index(&self, queue_item_id: i32) -> Option<usize> {
        let target = queue_item_id as u64;
        self.queue
            .iter()
            .position(|t| t.queue_item_id == Some(target))
    }

    // === Event handlers ===

    fn handle_playback_command(&mut self, cmd: &msg::cmd::SetState) -> msg::QueueRendererState {
        let new_state = cmd.state().unwrap_or(self.state); // None means keep current
        let position_ms = cmd.current_position;
        let queue_item_id = cmd.current_queue_item.as_ref().and_then(|q| q.queue_item_id);

        info!(state = ?cmd.state(), ?position_ms, ?queue_item_id, "Playback command received");

        let was_playing = self.state == PlayingState::Playing;

        // Handle position: if specified, use it; if transitioning, snapshot current
        if let Some(pos) = position_ms {
            self.set_position(pos);
        } else if !was_playing && new_state == PlayingState::Playing {
            // Starting playback - snapshot current position so elapsed time counts from now
            self.set_position(self.current_position_ms());
        } else if was_playing && new_state != PlayingState::Playing {
            // Stopping/pausing - snapshot the current calculated position
            self.set_position(self.current_position_ms());
        }

        self.state = new_state;

        // If server specified a queue item, switch to it
        if let Some(queue_item_id) = queue_item_id
            && let Some(idx) = self.find_track_index(queue_item_id as i32)
        {
            self.current_track_index = Some(idx);
        }

        // Start playing first track if we have a queue and not already on a track
        // Don't reset position - it may have been set by a prior seek command
        if self.state == PlayingState::Playing
            && self.current_track_index.is_none()
            && !self.queue.is_empty()
        {
            self.current_track_index = Some(0);
        }

        self.renderer_state()
    }

    fn handle_activate(&mut self) -> ActivationState {
        info!(renderer_id = self.renderer_id, "Device activated");

        ActivationState {
            muted: self.muted,
            volume: self.volume,
            max_quality: 4, // HiRes 192kHz capability level
            playback: self.renderer_state(),
        }
    }

    fn handle_deactivate(&mut self) {
        warn!(renderer_id = self.renderer_id, "Device deactivated");
        self.state = PlayingState::Stopped;
    }

    fn handle_queue_update(&mut self, queue: &msg::notify::QueueState) {
        let version = queue
            .queue_version
            .as_ref()
            .map(|v| (v.major.unwrap_or(0), v.minor.unwrap_or(0)))
            .unwrap_or((0, 0));

        info!(
            track_count = queue.tracks.len(),
            version = ?version,
            "Queue updated"
        );

        // Remember current track ID if playing
        let current_id = self.current_queue_item_id();

        self.queue = queue.tracks.clone();

        // Try to find the same track in the new queue
        if let Some(id) = current_id {
            self.current_track_index = self.find_track_index(id);
            if self.current_track_index.is_none() {
                // Track was removed, stop playback
                self.state = PlayingState::Stopped;
                self.set_position(0);
            }
        }
    }

    fn handle_heartbeat(&mut self) -> Option<msg::QueueRendererState> {
        if self.tick() && self.state == PlayingState::Playing {
            debug!(position_ms = self.current_position_ms(), "Heartbeat");
            Some(self.renderer_state())
        } else {
            None
        }
    }

    fn handle_restore_state(&mut self, rsu: &msg::notify::RendererStateUpdated) {
        let position_ms = rsu
            .state
            .as_ref()
            .and_then(|s| s.current_position.as_ref())
            .and_then(|p| p.value)
            .unwrap_or(0);
        let queue_index = rsu.state.as_ref().and_then(|s| s.current_queue_index);

        info!(position_ms, ?queue_index, "Restoring state from previous renderer");
        self.set_position(position_ms);
        if let Some(idx) = queue_index {
            self.current_track_index = Some(idx as usize);
        }
    }
}

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = load_config();

    // Create the player
    let mut player = FakePlayer::new();

    // Start the session manager
    let mut manager = SessionManager::start(7864).await?;

    // Register device and get its session handle
    let device_config = DeviceConfig::new("Fake Player", &config.app_id);
    let mut session = manager.add_device(device_config).await?;
    info!("Registered device: Fake Player");

    info!("Fake player running. Press Ctrl+C to stop.");
    info!("Select a device in the Qobuz app to connect.");

    // Spawn manager in background
    let manager_handle = tokio::spawn(async move {
        if let Err(e) = manager.run().await {
            warn!("Manager error: {e}");
        }
    });

    // Main loop: handle events and ctrl+c
    loop {
        tokio::select! {
            Some(event) = session.recv() => {
                match event {
                    // === Commands (require response) ===
                    SessionEvent::Command(cmd) => match cmd {
                        Command::SetState { cmd, respond, .. } => {
                            let response = player.handle_playback_command(&cmd);
                            respond.send(response);
                        }

                        Command::SetActive { respond, .. } => {
                            let response = player.handle_activate();
                            respond.send(response);
                        }

                        Command::Heartbeat { respond, .. } => {
                            let response = player.handle_heartbeat();
                            respond.send(response);
                        }
                    },

                    // === Notifications (informational) ===
                    SessionEvent::Notification(n) => match n {
                        Notification::QueueState(queue) => {
                            player.handle_queue_update(&queue);
                        }

                        Notification::QueueTracksAdded(added) => {
                            info!("Queue tracks added: {} tracks", added.tracks.len());
                            // In a real player, we'd insert these tracks
                        }

                        Notification::QueueTracksInserted(inserted) => {
                            info!("Queue tracks inserted: {} tracks", inserted.tracks.len());
                        }

                        Notification::QueueTracksRemoved(removed) => {
                            info!("Queue tracks removed: {} items", removed.queue_item_ids.len());
                        }

                        Notification::QueueTracksReordered(reordered) => {
                            info!("Queue tracks reordered: {} items", reordered.queue_item_ids.len());
                        }

                        Notification::LoopModeSet(lm) => {
                            let mode = lm.loop_mode().unwrap_or_default();
                            info!(?mode, "Loop mode changed");
                            player.loop_mode = mode;
                        }

                        Notification::ShuffleModeSet(sm) => {
                            let enabled = sm.shuffle_on.unwrap_or(false);
                            info!(enabled, "Shuffle mode changed");
                            player.shuffle_enabled = enabled;
                        }

                        Notification::Deactivated => {
                            player.handle_deactivate();
                        }

                        Notification::RestoreState(rsu) => {
                            player.handle_restore_state(&rsu);
                        }

                        Notification::Connected => {
                            info!("WebSocket connected");
                        }

                        Notification::Disconnected { reason, .. } => {
                            info!("Disconnected: {:?}", reason);
                        }

                        Notification::DeviceRegistered { renderer_id, .. } => {
                            info!("Device registered with renderer_id={}", renderer_id);
                            player.renderer_id = renderer_id;
                        }

                        Notification::SessionClosed { .. } => {
                            info!("Session closed");
                        }

                        other => {
                            debug!(?other, "Unhandled notification");
                        }
                    },
                }
            }
            _ = signal::ctrl_c() => {
                info!("Shutting down...");
                break;
            }
        }
    }

    manager_handle.abort();
    Ok(())
}
