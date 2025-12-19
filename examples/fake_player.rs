//! Fake player example - simulates a Qobuz Connect player.
//!
//! This example registers as a device, takes over as active renderer,
//! and simulates playback - giving feedback to the server so the iOS app
//! sees it as a real speaker playing music.
//!
//! Run with: RUST_LOG=info cargo run --example fake_player
//! Run with debug: RUST_LOG=qonductor=debug,fake_player=debug cargo run --example fake_player

use qonductor::{AudioQuality, DeviceConfig, LoopMode, PlayState, QueueTrack, SessionEvent, SessionManager};
use rand::Rng;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::time::Duration;
use tokio::signal;
use tokio::time::{interval, Interval};
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

// ============================================================================
// Track Duration Provider
// ============================================================================

/// Trait for getting track durations.
///
/// This allows swapping in different duration sources:
/// - RandomDurationProvider for testing (current implementation)
/// - API-based provider that fetches real durations from Qobuz
trait TrackDurationProvider: Send + Sync {
    /// Get the duration for a track in milliseconds.
    fn get_duration_ms(&self, track_id: u64) -> u32;
}

/// Provides random track durations between 30 seconds and 5 minutes.
struct RandomDurationProvider {
    /// Cache of track_id -> duration_ms to keep consistent durations per track.
    cache: std::sync::Mutex<HashMap<u64, u32>>,
}

impl RandomDurationProvider {
    fn new() -> Self {
        Self {
            cache: std::sync::Mutex::new(HashMap::new()),
        }
    }
}

impl TrackDurationProvider for RandomDurationProvider {
    fn get_duration_ms(&self, track_id: u64) -> u32 {
        let mut cache = self.cache.lock().unwrap();
        *cache.entry(track_id).or_insert_with(|| {
            // Random duration between 30 seconds and 5 minutes
            let mut rng = rand::thread_rng();
            rng.gen_range(30_000..300_000)
        })
    }
}

// ============================================================================
// Fake Player
// ============================================================================

/// Simulates a Qobuz Connect player.
struct FakePlayer {
    // Playback state
    state: PlayState,
    position_ms: u32,
    current_track_index: Option<usize>,

    // Queue
    queue: Vec<QueueTrack>,
    queue_version: (u64, i32),

    // Settings
    loop_mode: LoopMode,
    shuffle_enabled: bool,
    volume: u32,
    muted: bool,

    // Config
    update_interval: Duration,

    // Identity
    renderer_id: u64,

    // Duration provider
    duration_provider: Box<dyn TrackDurationProvider>,
}

impl FakePlayer {
    /// Create a new fake player with configurable update interval.
    fn new(update_interval: Duration) -> Self {
        Self {
            state: PlayState::Stopped,
            position_ms: 0,
            current_track_index: None,
            queue: Vec::new(),
            queue_version: (0, 0),
            loop_mode: LoopMode::Off,
            shuffle_enabled: false,
            volume: 100,
            muted: false,
            update_interval,
            renderer_id: 0,
            duration_provider: Box::new(RandomDurationProvider::new()),
        }
    }

    /// Get the current track being played, if any.
    fn current_track(&self) -> Option<&QueueTrack> {
        self.current_track_index
            .and_then(|idx| self.queue.get(idx))
    }

    /// Get the current queue item ID for reporting state.
    fn current_queue_item_id(&self) -> Option<i32> {
        self.current_track().map(|t| t.queue_item_id as i32)
    }

    /// Get the duration of the current track in milliseconds.
    fn current_track_duration_ms(&self) -> Option<u32> {
        self.current_track()
            .map(|t| self.duration_provider.get_duration_ms(t.track_id))
    }

    /// Handle a playback command from the server.
    fn handle_playback_command(&mut self, state: PlayState, position_ms: Option<u32>) {
        let old_state = self.state;
        self.state = state;

        if let Some(pos) = position_ms {
            self.position_ms = pos;
        }

        // If we're starting playback and have no current track, start from beginning
        if state == PlayState::Playing && self.current_track_index.is_none() && !self.queue.is_empty()
        {
            self.current_track_index = Some(0);
            self.position_ms = position_ms.unwrap_or(0);
        }

        info!(
            "Playback: {:?} -> {:?} at {}ms (track index: {:?})",
            old_state, state, self.position_ms, self.current_track_index
        );

        if let Some(track) = self.current_track() {
            let duration_ms = self.duration_provider.get_duration_ms(track.track_id);
            info!(
                "  Current track: id={} queue_item={} duration={}s",
                track.track_id,
                track.queue_item_id,
                duration_ms / 1000
            );
        }
    }

    /// Handle a queue update from the server.
    fn handle_queue_update(&mut self, tracks: Vec<QueueTrack>, version: (u64, i32)) {
        info!(
            "Queue updated: {} tracks (version {}.{})",
            tracks.len(),
            version.0,
            version.1
        );

        for (i, track) in tracks.iter().enumerate() {
            let duration_ms = self.duration_provider.get_duration_ms(track.track_id);
            debug!(
                "  [{}] track_id={} queue_item={} duration={}s",
                i,
                track.track_id,
                track.queue_item_id,
                duration_ms / 1000
            );
        }

        // If we had a current track, try to find it in the new queue
        if let Some(current) = self.current_track() {
            let current_queue_item_id = current.queue_item_id;
            self.current_track_index = tracks
                .iter()
                .position(|t| t.queue_item_id == current_queue_item_id);

            if self.current_track_index.is_none() {
                info!("  Current track no longer in queue, will stop");
                self.state = PlayState::Stopped;
                self.position_ms = 0;
            }
        }

        self.queue = tracks;
        self.queue_version = version;
    }

    /// Handle volume command from the server.
    fn handle_volume_command(&mut self, volume: u32) {
        info!("Volume: {} -> {}", self.volume, volume);
        self.volume = volume;
    }

    /// Handle loop mode change.
    fn handle_loop_mode_change(&mut self, mode: LoopMode) {
        info!("Loop mode: {:?} -> {:?}", self.loop_mode, mode);
        self.loop_mode = mode;
    }

    /// Handle shuffle mode change.
    fn handle_shuffle_mode_change(&mut self, enabled: bool) {
        info!("Shuffle: {} -> {}", self.shuffle_enabled, enabled);
        self.shuffle_enabled = enabled;
    }

    /// Advance playback position by the update interval.
    /// Returns true if state report should be sent.
    fn tick(&mut self) -> bool {
        if self.state != PlayState::Playing {
            return false;
        }

        let interval_ms = self.update_interval.as_millis() as u32;
        self.position_ms += interval_ms;

        // Check if track has ended
        if let Some(duration_ms) = self.current_track_duration_ms()
            && self.position_ms >= duration_ms
        {
            info!(
                "Track ended at {}ms (duration was {}ms)",
                self.position_ms, duration_ms
            );
            self.advance_track();
        }

        // Report position update
        if let Some(track) = self.current_track() {
            debug!(
                "Position: {}ms / {}ms (track {})",
                self.position_ms,
                self.duration_provider.get_duration_ms(track.track_id),
                track.track_id
            );
        }

        true
    }

    /// Advance to the next track.
    fn advance_track(&mut self) {
        let current_idx = self.current_track_index.unwrap_or(0);
        let queue_len = self.queue.len();

        if queue_len == 0 {
            info!("Queue empty, stopping");
            self.state = PlayState::Stopped;
            self.position_ms = 0;
            self.current_track_index = None;
            return;
        }

        match self.loop_mode {
            LoopMode::One => {
                // Repeat current track
                info!("Repeating track (loop mode: One)");
                self.position_ms = 0;
            }
            LoopMode::All => {
                // Move to next, wrap around
                let next_idx = (current_idx + 1) % queue_len;
                info!(
                    "Advancing track: {} -> {} (loop mode: All, {} tracks)",
                    current_idx, next_idx, queue_len
                );
                self.current_track_index = Some(next_idx);
                self.position_ms = 0;

                if let Some(track) = self.current_track() {
                    let duration_ms = self.duration_provider.get_duration_ms(track.track_id);
                    info!(
                        "  Now playing: track_id={} queue_item={} duration={}s",
                        track.track_id,
                        track.queue_item_id,
                        duration_ms / 1000
                    );
                }
            }
            LoopMode::Off => {
                // Move to next, stop at end
                let next_idx = current_idx + 1;
                if next_idx >= queue_len {
                    info!("Reached end of queue, stopping");
                    self.state = PlayState::Stopped;
                    self.position_ms = 0;
                    // Keep current_track_index at last track
                } else {
                    info!(
                        "Advancing track: {} -> {} ({} tracks)",
                        current_idx, next_idx, queue_len
                    );
                    self.current_track_index = Some(next_idx);
                    self.position_ms = 0;

                    if let Some(track) = self.current_track() {
                        let duration_ms = self.duration_provider.get_duration_ms(track.track_id);
                        info!(
                            "  Now playing: track_id={} queue_item={} duration={}s",
                            track.track_id,
                            track.queue_item_id,
                            duration_ms / 1000
                        );
                    }
                }
            }
        }
    }

    /// Create a tick interval for playback updates.
    fn create_tick_interval(&self) -> Interval {
        interval(self.update_interval)
    }
}

// ============================================================================
// Main
// ============================================================================

#[derive(Deserialize)]
struct Credentials {
    app_id: String,
    #[allow(dead_code)]
    app_secret: String,
}

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    println!("=== Qonductor Fake Player ===");
    println!();

    // Load credentials
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

    // Start session manager
    let (mut manager, mut events) = match SessionManager::start(7864).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Failed to start session manager: {e}");
            std::process::exit(1);
        }
    };

    // Register device
    let device = DeviceConfig::new("Fake Player", &creds.app_id);
    println!("Device UUID: {}", device.uuid_formatted());

    if let Err(e) = manager.add_device(device).await {
        eprintln!("Failed to add device: {e}");
        std::process::exit(1);
    }

    // Get a handle before spawning run() - this can be used concurrently
    let handle = manager.handle();

    println!();
    println!("Fake player running!");
    println!("  HTTP endpoints: http://0.0.0.0:7864");
    println!("  mDNS service: Fake Player._qobuz-connect._tcp.local.");
    println!();
    println!("Waiting for controller to connect... (Press Ctrl+C to exit)");
    println!();

    // Spawn manager run loop (handles device selections)
    tokio::spawn(async move {
        if let Err(e) = manager.run().await {
            eprintln!("Session manager error: {e}");
        }
    });

    // Create fake player with 10 second heartbeat (matches C++ default)
    let mut player = FakePlayer::new(Duration::from_secs(10));
    let mut tick_interval = player.create_tick_interval();

    // Event loop
    loop {
        tokio::select! {
            // Handle session events
            Some(event) = events.recv() => {
                match event {
                    SessionEvent::Connected => {
                        info!("WebSocket connected");
                    }

                    SessionEvent::Disconnected { session_id, reason } => {
                        warn!("Disconnected (session {}): {:?}", session_id, reason);
                        // Reset player state
                        player.state = PlayState::Stopped;
                        player.renderer_id = 0;
                    }

                    SessionEvent::DeviceRegistered { device_uuid, renderer_id } => {
                        info!(
                            "Device registered: uuid={} renderer_id={}",
                            Uuid::from_bytes(device_uuid),
                            renderer_id
                        );
                        player.renderer_id = renderer_id;
                    }

                    SessionEvent::DeviceActive { renderer_id, .. } => {
                        info!("Device is now ACTIVE! renderer_id={}", renderer_id);
                        player.renderer_id = renderer_id;

                        // === Activation Handshake ===
                        // Report our initial state to the server immediately
                        // Order matters: mute, volume, max quality, playback state
                        info!("Sending activation handshake...");

                        if let Err(e) = handle.report_volume_muted(player.muted).await {
                            warn!("Failed to report mute state: {e}");
                        }

                        if let Err(e) = handle.report_volume(player.volume).await {
                            warn!("Failed to report volume: {e}");
                        }

                        if let Err(e) = handle.report_max_audio_quality(AudioQuality::FlacHiRes192).await {
                            warn!("Failed to report max audio quality: {e}");
                        }

                        if let Err(e) = handle.report_playback_state(
                            renderer_id,
                            player.state,
                            player.position_ms,
                            player.current_queue_item_id(),
                        ).await {
                            warn!("Failed to report playback state: {e}");
                        }

                        // Request queue and renderer state now that we're active
                        info!("Requesting queue state...");
                        if let Err(e) = handle.request_queue_state().await {
                            warn!("Failed to request queue state: {e}");
                        }

                        info!("Requesting renderer state...");
                        if let Err(e) = handle.request_renderer_state().await {
                            warn!("Failed to request renderer state: {e}");
                        }
                    }

                    SessionEvent::DeviceDeactivated { renderer_id, new_active_renderer_id, .. } => {
                        warn!(
                            "Device DEACTIVATED (was renderer_id={}), new active={}",
                            renderer_id, new_active_renderer_id
                        );
                        player.state = PlayState::Stopped;
                        player.renderer_id = 0;
                    }

                    SessionEvent::RendererAdded { renderer_id, name } => {
                        debug!("Other renderer added: {} (id={})", name, renderer_id);
                    }

                    SessionEvent::RendererRemoved { renderer_id } => {
                        debug!("Renderer removed: id={}", renderer_id);
                    }

                    SessionEvent::ActiveRendererChanged { renderer_id } => {
                        info!("Active renderer changed to id={}", renderer_id);
                    }

                    SessionEvent::QueueUpdated { tracks, version } => {
                        player.handle_queue_update(tracks, version);
                    }

                    SessionEvent::RendererStateUpdated { renderer_id, state, position_ms, duration_ms, queue_index } => {
                        if renderer_id == player.renderer_id || player.renderer_id == 0 {
                            debug!(
                                "Renderer state broadcast: {:?} at {}ms / {}ms, queue_index={}",
                                state, position_ms, duration_ms, queue_index
                            );
                            // This is an informational broadcast - we don't sync from it or respond
                            // We maintain our own playback state based on commands we receive
                        }
                    }

                    SessionEvent::PlaybackCommand { renderer_id, state, position_ms } => {
                        if renderer_id == player.renderer_id || player.renderer_id == 0 {
                            player.handle_playback_command(state, position_ms);
                            // Immediately report state back to server (protocol requirement)
                            if player.renderer_id != 0 {
                                if let Err(e) = handle.report_playback_state(
                                    player.renderer_id,
                                    player.state,
                                    player.position_ms,
                                    player.current_queue_item_id(),
                                ).await {
                                    warn!("Failed to report playback state: {e}");
                                }
                            }
                        } else {
                            debug!(
                                "Ignoring playback command for renderer {} (we are {})",
                                renderer_id, player.renderer_id
                            );
                        }
                    }

                    SessionEvent::VolumeCommand { renderer_id, volume } => {
                        if renderer_id == player.renderer_id || player.renderer_id == 0 {
                            player.handle_volume_command(volume);
                            // Immediately report volume back to server (protocol requirement)
                            if let Err(e) = handle.report_volume(player.volume).await {
                                warn!("Failed to report volume: {e}");
                            }
                        }
                    }

                    SessionEvent::LoopModeChanged { mode } => {
                        player.handle_loop_mode_change(mode);
                    }

                    SessionEvent::ShuffleModeChanged { enabled } => {
                        player.handle_shuffle_mode_change(enabled);
                    }

                    // Broadcast events - informational only, no response needed
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
                }
            }

            // Handle playback tick (only when playing)
            _ = tick_interval.tick() => {
                if player.tick() && player.renderer_id != 0 {
                    // Report state to server
                    if let Err(e) = handle.report_playback_state(
                        player.renderer_id,
                        player.state,
                        player.position_ms,
                        player.current_queue_item_id(),
                    ).await {
                        warn!("Failed to report playback state: {e}");
                    }
                }
            }

            // Handle Ctrl+C
            _ = signal::ctrl_c() => {
                println!("\nShutting down...");
                break;
            }
        }
    }
}
