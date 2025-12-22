//! Fake player example.
//!
//! Simulates a Qobuz Connect player using the stream-based event API.
//!
//! Run with: RUST_LOG=info cargo run --example fake_player
//! Run with debug: RUST_LOG=qonductor=debug,fake_player=debug cargo run --example fake_player

use qonductor::{
    ActivationState, BufferState, DeviceConfig, LoopMode, PlaybackCommand, PlaybackResponse,
    PlayingState, QueueTrack, SessionEvent, SessionManager,
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
    queue: Vec<QueueTrack>,

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

    fn current_track(&self) -> Option<&QueueTrack> {
        self.current_track_index.and_then(|i| self.queue.get(i))
    }

    fn current_queue_item_id(&self) -> Option<i32> {
        self.current_track().map(|t| t.queue_item_id as i32)
    }

    fn current_duration(&mut self) -> Option<u32> {
        let track_id = self.current_track()?.track_id;
        Some(self.duration_provider.get_duration(track_id))
    }

    fn next_queue_item_id(&self) -> Option<i32> {
        let current_idx = self.current_track_index?;
        let next_idx = current_idx + 1;
        self.queue.get(next_idx).map(|t| t.queue_item_id as i32)
    }

    fn playback_response(&mut self) -> PlaybackResponse {
        PlaybackResponse {
            state: self.state,
            buffer_state: BufferState::Ok,
            position_ms: self.current_position_ms(),
            duration_ms: self.current_duration(),
            queue_item_id: self.current_queue_item_id(),
            next_queue_item_id: self.next_queue_item_id(),
        }
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
                track_id = track.track_id,
                queue_item_id = track.queue_item_id,
                "Playing next track"
            );
        }

        true
    }

    fn find_track_index(&self, queue_item_id: i32) -> Option<usize> {
        self.queue
            .iter()
            .position(|t| t.queue_item_id == queue_item_id as u64)
    }

    // === Event handlers ===

    fn handle_playback_command(&mut self, cmd: PlaybackCommand) -> PlaybackResponse {
        info!(?cmd.state, ?cmd.position_ms, ?cmd.queue_item_id, "Playback command received");

        let was_playing = self.state == PlayingState::Playing;
        let new_state = cmd.state.unwrap_or(self.state); // None means keep current state

        // Handle position: if specified, use it; if transitioning, snapshot current
        if let Some(pos) = cmd.position_ms {
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
        if let Some(queue_item_id) = cmd.queue_item_id
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

        self.playback_response()
    }

    fn handle_activate(&mut self, renderer_id: u64) -> ActivationState {
        info!(renderer_id, "Device activated");
        self.renderer_id = renderer_id;

        ActivationState {
            muted: self.muted,
            volume: self.volume,
            max_quality: 4, // HiRes 192kHz capability level
            playback: self.playback_response(),
        }
    }

    fn handle_deactivate(&mut self, renderer_id: u64) {
        warn!(renderer_id, "Device deactivated");
        self.renderer_id = 0;
        self.state = PlayingState::Stopped;
    }

    fn handle_queue_update(&mut self, tracks: Vec<QueueTrack>, version: (u64, i32)) {
        info!(
            track_count = tracks.len(),
            version = ?version,
            "Queue updated"
        );

        // Remember current track ID if playing
        let current_id = self.current_queue_item_id();

        self.queue = tracks;

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

    fn handle_heartbeat(&mut self) -> Option<PlaybackResponse> {
        if self.tick() && self.state == PlayingState::Playing {
            debug!(position_ms = self.current_position_ms(), "Heartbeat");
            Some(self.playback_response())
        } else {
            None
        }
    }

    fn handle_restore_state(&mut self, position_ms: u32, queue_index: Option<u32>) {
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

                    SessionEvent::PlaybackCommand { renderer_id: _, cmd, respond } => {
                        let response = player.handle_playback_command(cmd);
                        respond.send(response);
                    }

                    SessionEvent::Activate { renderer_id, respond } => {
                        let response = player.handle_activate(renderer_id);
                        respond.send(response);
                    }

                    SessionEvent::Heartbeat { renderer_id: _, respond } => {
                        let response = player.handle_heartbeat();
                        respond.send(response);
                    }

                    // === Events (no response needed) ===

                    SessionEvent::Deactivated { renderer_id } => {
                        player.handle_deactivate(renderer_id);
                    }

                    SessionEvent::QueueUpdated { tracks, version } => {
                        player.handle_queue_update(tracks, version);
                    }

                    SessionEvent::LoopModeChanged { mode } => {
                        info!(?mode, "Loop mode changed");
                        player.loop_mode = mode;
                    }

                    SessionEvent::ShuffleModeChanged { enabled } => {
                        info!(enabled, "Shuffle mode changed");
                        player.shuffle_enabled = enabled;
                    }

                    SessionEvent::RestoreState { position_ms, queue_index } => {
                        player.handle_restore_state(position_ms, queue_index);
                    }

                    // === Broadcasts (informational) ===

                    SessionEvent::Connected => {
                        info!("WebSocket connected");
                    }

                    SessionEvent::Disconnected { reason, .. } => {
                        info!("Disconnected: {:?}", reason);
                    }

                    SessionEvent::DeviceRegistered { renderer_id, .. } => {
                        info!("Device registered with renderer_id={}", renderer_id);
                    }

                    SessionEvent::RendererAdded { renderer_id, name } => {
                        debug!("Other renderer added: {} (id={})", name, renderer_id);
                    }

                    SessionEvent::RendererRemoved { renderer_id } => {
                        debug!("Renderer removed: id={}", renderer_id);
                    }

                    SessionEvent::ActiveRendererChanged { renderer_id } => {
                        debug!("Active renderer changed to: {}", renderer_id);
                    }

                    SessionEvent::RendererStateUpdated { renderer_id, state, position_ms, .. } => {
                        debug!("Renderer {} state broadcast: {:?} at {}ms", renderer_id, state, position_ms);
                    }

                    SessionEvent::VolumeBroadcast { renderer_id, volume } => {
                        debug!("Volume broadcast: renderer {} -> {}", renderer_id, volume);
                    }

                    SessionEvent::VolumeMutedBroadcast { renderer_id, muted } => {
                        debug!("Mute broadcast: renderer {} -> {}", renderer_id, muted);
                    }

                    SessionEvent::MaxAudioQualityBroadcast { renderer_id, quality } => {
                        debug!("Max quality broadcast: renderer {} -> {:?}", renderer_id, quality);
                    }

                    SessionEvent::FileAudioQualityBroadcast { renderer_id, sample_rate_hz } => {
                        debug!("File quality broadcast: renderer {} -> {}Hz", renderer_id, sample_rate_hz);
                    }

                    SessionEvent::SessionClosed { device_uuid: _ } => {
                        info!("Session closed");
                    }
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
