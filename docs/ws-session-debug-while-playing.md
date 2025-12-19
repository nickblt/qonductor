# QConnect WebSocket Session Debug Log - While Playing Handoff

Captured session showing the complete flow from loading the web player through handing off playback while a track is actively playing.

**Setup:**
1. Web player loaded (fresh session)
2. Desktop app (alnitak) already had a track queued at 32 seconds
3. User clicked play on desktop
4. While playing, user switched to web player
5. User paused on web player
6. User switched back to desktop

## Message Timeline

### Phase 1: Session Initialization

---

### 1. TX: Authenticate

```
Type: 1 (AUTHENTICATE)
```

JWT authentication with Qobuz server.

---

### 2. TX: Subscribe to Session

```
Type: 2 (SUBSCRIBE)
```

Join the QConnect session with device info: "Web Player Chrome" (wp-8.1.0-b019)

---

### 3. RX: Session State + Renderers

```
msg_id: 2804
Message types: 83, 83, 81 (batched)
```

| Message | Type | Content |
| ------- | ---- | ------- |
| 0 | SrvrCtrlAddRenderer | renderer_id=7, name="alnitak" (dsk-8.1.0-b019) |
| 1 | SrvrCtrlAddRenderer | renderer_id=10, name="Web Player Chrome" (wp-8.1.0-b019) |
| 2 | SrvrCtrlSessionState | session_id=**0xFFFFFFFFFFFFFFFF**, queue_version=2.1, track_index=3 |

**Analysis:** Server tells us about existing renderers and session state. The special session_id (max u64) indicates no renderer is currently active.

---

### 4. TX: Ask for Renderer State

```
msg_id: 4
Message type: 77 (CtrlSrvrAskForRendererState)
```

Client requests current renderer state.

---

### 5. RX: Current Renderer State (No Active Renderer)

```
msg_id: 2805
Message type: 82 (SrvrCtrlRendererStateUpdated)
```

| Field | Value |
| ----- | ----- |
| renderer_id | **18446744073709551615** (0xFFFFFFFFFFFFFFFF = no active) |
| playing_state | Paused |
| buffer_state | Ok |
| position.value | **32000ms** |
| duration | 287293ms (~4:47) |
| current_queue_index | 1 |

**Analysis:** Special renderer_id indicates no active renderer. Position is saved from previous session.

---

### Phase 2: Desktop Starts Playing (~28 seconds later)

---

### 6. RX: Desktop Becomes Active

```
msg_id: 2808
Message type: 86 (SrvrCtrlActiveRendererChanged)
renderer_id: 7
```

---

### 7-9. RX: Desktop Renderer Info

| msg_id | Type | Content |
| ------ | ---- | ------- |
| 2809 | SrvrCtrlVolumeMuted | renderer_id=7, not muted |
| 2810 | SrvrCtrlVolumeChanged | renderer_id=7, volume=100 |
| 2811 | SrvrCtrlMaxAudioQualityChanged | max_audio_quality=7 |

---

### 10. RX: Desktop Playing State

```
msg_id: 2812
Message type: 82 (SrvrCtrlRendererStateUpdated)
```

| Field | Value |
| ----- | ----- |
| renderer_id | 7 |
| playing_state | **Playing** |
| buffer_state | Ok |
| position.timestamp | 1766128213988 |
| position.value | **32000ms** |
| duration | 287293ms |

**Analysis:** Desktop started playing from 32 seconds.

---

### Phase 3: User Selects Web Player (~8 seconds later)

---

### 11. RX: Server Activates Us

```
msg_id: 2814
Message type: 43 (SrvrRndrSetActive)
active: true
```

**Analysis:** Server tells us we're now the active renderer!

---

### 12-14. TX: Report Our Capabilities

| msg_id | Type | Content |
| ------ | ---- | ------- |
| 5 | RndrSrvrVolumeMuted | not muted |
| 6 | RndrSrvrVolumeChanged | volume=100 |
| 7 | RndrSrvrMaxAudioQualityChanged | value=3 |

---

### 15. TX: Report Initial Playing State

```
msg_id: 8
Message type: 23 (RndrSrvrStateUpdated)
```

| Field | Value |
| ----- | ----- |
| playing_state | **Playing** |
| buffer_state | **Buffering** |
| position.timestamp | 1766128222902 |
| position.value | **40914ms** |
| queue_version | 2.1 |
| current_queue_item_id | 1 |
| next_queue_item_id | 2 |

**Analysis:** We pick up playback at 40914ms (32000 + ~9 seconds of playback). Buffer state is Buffering as we load the stream.

---

### 16. RX: Server Broadcasts Active Renderer Change

```
msg_id: 2815
Message type: 86 (SrvrCtrlActiveRendererChanged)
renderer_id: 10 (us!)
```

---

### 17-20. RX: Server Broadcasts Our Info

| msg_id | Type | Content |
| ------ | ---- | ------- |
| 2816 | SrvrCtrlVolumeMuted | renderer_id=10, not muted |
| 2817 | SrvrCtrlVolumeChanged | renderer_id=10, volume=100 |
| 2818 | SrvrCtrlMaxAudioQualityChanged | max_audio_quality=10 (?) |
| 2819 | SrvrCtrlRendererStateUpdated | Playing, Buffering, pos=40914ms |

---

### 21-22. TX: State Updates (with duration)

```
msg_id: 9, 10
Message type: 23 (RndrSrvrStateUpdated)
position.value: 40914ms
duration: 287293ms
buffer_state: Buffering
```

---

### 23-24. RX: Server Echoes State

```
msg_id: 2820, 2821
Message type: 82 (SrvrCtrlRendererStateUpdated)
```

---

### 25-26. TX: Buffering Complete

```
msg_id: 11, 12
Message type: 23 (RndrSrvrStateUpdated)
buffer_state: Ok
```

**Analysis:** We finished buffering and are ready to play.

---

### 27. TX: Report Sample Rate

```
msg_id: 13
Message type: 26 (RndrSrvrFileAudioQualityChanged)
value: 44100 (Hz)
```

---

### 28-30. RX: Server Confirms Buffered State

```
msg_id: 2822, 2823, 2824
Message types: 82, 82, 100
```

Server confirms our Ok buffer state and broadcasts file audio quality.

---

### Phase 4: User Clicks Pause (~8 seconds later)

---

### 31. RX: Server Commands Pause

```
msg_id: 2825
Message type: 41 (SrvrRndrSetState)
playing_state: Paused
```

**Analysis:** User clicked pause, server tells us to pause.

---

### Phase 5: User Switches Back to Desktop (~8 seconds later)

---

### 32. RX: Server Deactivates Us

```
msg_id: 2826
Message type: 43 (SrvrRndrSetActive)
active: None
```

**Analysis:** Server tells us we're no longer the active renderer.

---

### 33. RX: Desktop Becomes Active Again

```
msg_id: 2828
Message type: 86 (SrvrCtrlActiveRendererChanged)
renderer_id: 7
```

---

### 34-37. RX: Desktop Renderer Info

| msg_id | Type | Content |
| ------ | ---- | ------- |
| 2829 | SrvrCtrlVolumeMuted | renderer_id=7, not muted |
| 2830 | SrvrCtrlVolumeChanged | renderer_id=7, volume=100 |
| 2831 | SrvrCtrlMaxAudioQualityChanged | max_audio_quality=7 |
| 2832 | SrvrCtrlRendererStateUpdated | Paused, Buffering, pos=**49946ms** |

**Analysis:** Desktop shows position at 49946ms - approximately where we paused (40914 + ~9 seconds).

---

### 38-39. RX: Desktop State Updates

Desktop finishes buffering and reports ready state.

---

## Summary

### Position Tracking Through Handoffs

| Event | Position | Delta |
| ----- | -------- | ----- |
| Session load (no active renderer) | 32000ms | - |
| Desktop starts playing | 32000ms | +0ms |
| Web player takes over (~9s later) | 40914ms | +8914ms |
| User pauses | ~49000ms | +8086ms |
| Desktop takes over | 49946ms | ~same |

**Key insight:** Position is calculated as `now - timestamp + value`. Each renderer picks up where the previous left off.

### Activation Sequence Summary

```
No Active Renderer (renderer_id = 0xFFFFFFFFFFFFFFFF)
    │
    ▼ User clicks play on Desktop
Desktop Active (renderer_id = 7, Playing)
    │
    ▼ User selects Web Player
Server: SrvrRndrSetActive(active=true) to Web Player
    │
    ▼
Web Player reports state (Playing, Buffering, pos=40914ms)
    │
    ▼
Server: SrvrCtrlActiveRendererChanged(renderer_id=10)
    │
    ▼ User clicks pause
Server: SrvrRndrSetState(Paused) to Web Player
    │
    ▼ User selects Desktop
Server: SrvrRndrSetActive(active=None) to Web Player
Server: SrvrCtrlActiveRendererChanged(renderer_id=7)
```

### Key Findings

1. **Special "no active renderer" ID:** `0xFFFFFFFFFFFFFFFF` (max u64) indicates no renderer is active but position is still tracked.

2. **Seamless handoff:** When switching renderers while playing, position is calculated from the timestamp to determine where to resume.

3. **Position calculation:** `current_position = now - timestamp + value`
   - Desktop at 32000ms with timestamp T1
   - ~9 seconds later, web player calculates: `now - T1 + 32000 = 40914ms`

4. **Buffering during handoff:** New renderer reports `buffer_state=Buffering` while loading, then `Ok` when ready.

5. **Deactivation:** `SrvrRndrSetActive(active=None)` tells the current renderer it's being deactivated.

### Message Types Reference

| Type | Name | Direction | Purpose |
| ---- | ---- | --------- | ------- |
| 23 | RndrSrvrStateUpdated | TX | Report playback state |
| 25 | RndrSrvrVolumeChanged | TX | Report volume |
| 26 | RndrSrvrFileAudioQualityChanged | TX | Report sample rate |
| 28 | RndrSrvrMaxAudioQualityChanged | TX | Report max quality |
| 29 | RndrSrvrVolumeMuted | TX | Report mute state |
| 41 | SrvrRndrSetState | RX | Server commands play/pause |
| 43 | SrvrRndrSetActive | RX | Server activates/deactivates |
| 77 | CtrlSrvrAskForRendererState | TX | Request current state |
| 81 | SrvrCtrlSessionState | RX | Session info on connect |
| 82 | SrvrCtrlRendererStateUpdated | RX | Broadcast renderer state |
| 83 | SrvrCtrlAddRenderer | RX | New renderer joined |
| 86 | SrvrCtrlActiveRendererChanged | RX | Active renderer changed |
| 87 | SrvrCtrlVolumeChanged | RX | Broadcast volume |
| 98 | SrvrCtrlVolumeMuted | RX | Broadcast mute state |
| 99 | SrvrCtrlMaxAudioQualityChanged | RX | Broadcast max quality |
| 100 | SrvrCtrlFileAudioQualityChanged | RX | Broadcast file quality |
