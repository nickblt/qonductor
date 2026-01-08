//! Re-exports of protobuf types with friendlier names.
//!
//! Types are organized by communication direction:
//!
//! - [`cmd`] - Commands from server TO this renderer (SrvrRndr*)
//! - [`report`] - Renderer reports TO server (RndrSrvr*)
//! - [`ctrl`] - Controller requests TO server (CtrlSrvr*)
//! - [`notify`] - Server broadcasts/notifications (SrvrCtrl*)
//!
//! Common types (enums, shared structures) are re-exported at the module root.

/// Commands from server TO this renderer (SrvrRndr* messages).
///
/// These are received when the Qobuz app or another controller
/// wants to control playback on this device.
pub mod cmd {
    pub use crate::proto::qconnect::SrvrRndrSetActive as SetActive;
    pub use crate::proto::qconnect::SrvrRndrSetAutoplayMode as SetAutoplayMode;
    pub use crate::proto::qconnect::SrvrRndrSetLoopMode as SetLoopMode;
    pub use crate::proto::qconnect::SrvrRndrSetMaxAudioQuality as SetMaxAudioQuality;
    pub use crate::proto::qconnect::SrvrRndrSetShuffleMode as SetShuffleMode;
    pub use crate::proto::qconnect::SrvrRndrSetState as SetState;
    pub use crate::proto::qconnect::SrvrRndrSetVolume as SetVolume;
}

/// Our reports TO server (RndrSrvr* messages).
///
/// These are sent to report our current state back to the server,
/// which then broadcasts to all connected controllers.
pub mod report {
    // Session
    pub use crate::proto::qconnect::RndrSrvrJoinSession as JoinSession;
    pub use crate::proto::qconnect::RndrSrvrDeviceInfoUpdated as DeviceInfoUpdated;

    // Playback state
    pub use crate::proto::qconnect::RndrSrvrStateUpdated as StateUpdated;
    pub use crate::proto::qconnect::RndrSrvrRendererAction as RendererAction;

    // Volume
    pub use crate::proto::qconnect::RndrSrvrVolumeChanged as VolumeChanged;
    pub use crate::proto::qconnect::RndrSrvrVolumeMuted as VolumeMuted;

    // Audio quality
    pub use crate::proto::qconnect::RndrSrvrFileAudioQualityChanged as FileAudioQualityChanged;
    pub use crate::proto::qconnect::RndrSrvrDeviceAudioQualityChanged as DeviceAudioQualityChanged;
    pub use crate::proto::qconnect::RndrSrvrMaxAudioQualityChanged as MaxAudioQualityChanged;
}

/// Controller requests TO server (CtrlSrvr* messages).
///
/// These are sent by controllers (apps) to request changes.
/// The server then forwards appropriate commands to renderers.
pub mod ctrl {
    // Session
    pub use crate::proto::qconnect::CtrlSrvrJoinSession as JoinSession;

    // Playback control
    pub use crate::proto::qconnect::CtrlSrvrSetPlayerState as SetPlayerState;
    pub use crate::proto::qconnect::CtrlSrvrSetActiveRenderer as SetActiveRenderer;
    pub use crate::proto::qconnect::CtrlSrvrSetVolume as SetVolume;
    pub use crate::proto::qconnect::CtrlSrvrMuteVolume as MuteVolume;

    // Queue management
    pub use crate::proto::qconnect::CtrlSrvrClearQueue as ClearQueue;
    pub use crate::proto::qconnect::CtrlSrvrQueueLoadTracks as QueueLoadTracks;
    pub use crate::proto::qconnect::CtrlSrvrQueueInsertTracks as QueueInsertTracks;
    pub use crate::proto::qconnect::CtrlSrvrQueueAddTracks as QueueAddTracks;
    pub use crate::proto::qconnect::CtrlSrvrQueueRemoveTracks as QueueRemoveTracks;
    pub use crate::proto::qconnect::CtrlSrvrQueueReorderTracks as QueueReorderTracks;
    pub use crate::proto::qconnect::CtrlSrvrSetQueueState as SetQueueState;

    // Mode settings
    pub use crate::proto::qconnect::CtrlSrvrSetShuffleMode as SetShuffleMode;
    pub use crate::proto::qconnect::CtrlSrvrSetLoopMode as SetLoopMode;
    pub use crate::proto::qconnect::CtrlSrvrSetMaxAudioQuality as SetMaxAudioQuality;
    pub use crate::proto::qconnect::CtrlSrvrSetAutoplayMode as SetAutoplayMode;

    // State requests
    pub use crate::proto::qconnect::CtrlSrvrAskForQueueState as AskForQueueState;
    pub use crate::proto::qconnect::CtrlSrvrAskForRendererState as AskForRendererState;

    // Autoplay/radio
    pub use crate::proto::qconnect::CtrlSrvrAutoplayLoadTracks as AutoplayLoadTracks;
    pub use crate::proto::qconnect::CtrlSrvrAutoplayRemoveTracks as AutoplayRemoveTracks;
}

/// Notifications/broadcasts from server (SrvrCtrl* messages).
///
/// These inform us about queue state, mode changes, and other renderers.
pub mod notify {
    // Session
    pub use crate::proto::qconnect::SrvrCtrlSessionState as SessionState;

    // Queue state
    pub use crate::proto::qconnect::SrvrCtrlQueueState as QueueState;
    pub use crate::proto::qconnect::SrvrCtrlQueueCleared as QueueCleared;
    pub use crate::proto::qconnect::SrvrCtrlQueueLoadTracks as QueueLoadTracks;
    pub use crate::proto::qconnect::SrvrCtrlQueueTracksAdded as QueueTracksAdded;
    pub use crate::proto::qconnect::SrvrCtrlQueueTracksInserted as QueueTracksInserted;
    pub use crate::proto::qconnect::SrvrCtrlQueueTracksRemoved as QueueTracksRemoved;
    pub use crate::proto::qconnect::SrvrCtrlQueueTracksReordered as QueueTracksReordered;
    pub use crate::proto::qconnect::SrvrCtrlQueueVersionChanged as QueueVersionChanged;
    pub use crate::proto::qconnect::SrvrCtrlQueueErrorMessage as QueueErrorMessage;

    // Autoplay
    pub use crate::proto::qconnect::SrvrCtrlAutoplayModeSet as AutoplayModeSet;
    pub use crate::proto::qconnect::SrvrCtrlAutoplayTracksLoaded as AutoplayTracksLoaded;

    // Mode changes
    pub use crate::proto::qconnect::SrvrCtrlLoopModeSet as LoopModeSet;
    pub use crate::proto::qconnect::SrvrCtrlShuffleModeSet as ShuffleModeSet;

    // Renderer presence
    pub use crate::proto::qconnect::SrvrCtrlActiveRendererChanged as ActiveRendererChanged;
    pub use crate::proto::qconnect::SrvrCtrlAddRenderer as AddRenderer;
    pub use crate::proto::qconnect::SrvrCtrlUpdateRenderer as UpdateRenderer;
    pub use crate::proto::qconnect::SrvrCtrlRemoveRenderer as RemoveRenderer;

    // Renderer state broadcasts
    pub use crate::proto::qconnect::SrvrCtrlRendererStateUpdated as RendererStateUpdated;
    pub use crate::proto::qconnect::SrvrCtrlVolumeChanged as VolumeChanged;
    pub use crate::proto::qconnect::SrvrCtrlVolumeMuted as VolumeMuted;
    pub use crate::proto::qconnect::SrvrCtrlMaxAudioQualityChanged as MaxAudioQualityChanged;
    pub use crate::proto::qconnect::SrvrCtrlFileAudioQualityChanged as FileAudioQualityChanged;
    pub use crate::proto::qconnect::SrvrCtrlDeviceAudioQualityChanged as DeviceAudioQualityChanged;
}

// Common types re-exported at module root
pub use crate::proto::qconnect::{
    // Enums
    BufferState, DeviceType, LoopMode, PlayingState,
    // Shared structures
    DeviceCapabilities, DeviceInfo, Position, QueueItemRef, QueueRendererState, QueueTrackRef,
    QueueVersion, RendererState,
};

// ============================================================================
// Typed Enum Accessors
// ============================================================================

/// Extension trait for [`QueueRendererState`] with typed enum accessors.
pub trait QueueRendererStateExt {
    fn state(&self) -> Option<PlayingState>;
    fn set_state(&mut self, state: PlayingState) -> &mut Self;
    fn buffer(&self) -> Option<BufferState>;
    fn set_buffer(&mut self, state: BufferState) -> &mut Self;
}

impl QueueRendererStateExt for QueueRendererState {
    fn state(&self) -> Option<PlayingState> {
        self.playing_state.and_then(|i| PlayingState::try_from(i).ok())
    }
    fn set_state(&mut self, state: PlayingState) -> &mut Self {
        self.playing_state = Some(state.into());
        self
    }
    fn buffer(&self) -> Option<BufferState> {
        self.buffer_state.and_then(|i| BufferState::try_from(i).ok())
    }
    fn set_buffer(&mut self, state: BufferState) -> &mut Self {
        self.buffer_state = Some(state.into());
        self
    }
}

/// Extension trait for [`cmd::SetState`] with typed enum accessors.
pub trait SetStateExt {
    fn state(&self) -> Option<PlayingState>;
}

impl SetStateExt for cmd::SetState {
    fn state(&self) -> Option<PlayingState> {
        self.playing_state.and_then(|i| PlayingState::try_from(i).ok())
    }
}

/// Extension trait for [`notify::LoopModeSet`] with typed enum accessor.
pub trait LoopModeSetExt {
    fn loop_mode(&self) -> Option<LoopMode>;
}

impl LoopModeSetExt for notify::LoopModeSet {
    fn loop_mode(&self) -> Option<LoopMode> {
        self.mode.and_then(|i| LoopMode::try_from(i).ok())
    }
}

// ============================================================================
// Position Helpers
// ============================================================================

/// Extension trait for [`Position`] providing convenience constructors.
pub trait PositionExt {
    /// Create a Position with the current timestamp and given value.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use qonductor::msg::{Position, PositionExt};
    ///
    /// let pos = Position::now(12345); // 12.345 seconds into the track
    /// ```
    fn now(value: u32) -> Position;
}

impl PositionExt for Position {
    fn now(value: u32) -> Position {
        Position {
            timestamp: Some(now_ms()),
            value: Some(value),
        }
    }
}

/// Get current time in milliseconds since Unix epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time before Unix epoch")
        .as_millis() as u64
}
