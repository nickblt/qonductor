# Qonductor Architecture

This document describes the async communication patterns and message flow in Qonductor.

## Overview

Qonductor has three main async components that communicate via channels.
Each device gets its own event channel for clean isolation:

```
┌───────────────────────────────────────────────────────────────────────────┐
│                          User Application                                 │
│                                                                           │
│   // Each device has its own event stream                                │
│   let events = manager.add_device(config).await?;                        │
│   while let Some(event) = events.recv().await {                          │
│       match event {                                                       │
│           SessionEvent::PlaybackCommand { respond, .. } => { ... }       │
│           SessionEvent::QueueUpdated { .. } => { ... }                   │
│       }                                                                   │
│   }                                                                       │
└───────────────────────────────────────────────────────────────────────────┘
          ▲
          │ event_tx (per device)
          │
┌─────────┴─────────────────────────┐    ┌───────────────────────────────────┐
│     SessionRunner (per device)    │    │       DeviceRegistry              │
│                                   │    │                                   │
│   tokio::select! {                │    │  HTTP Server (Axum)               │
│       ws.recv() => send event     │    │  mDNS Broadcast (zeroconf)        │
│       heartbeat => send Heartbeat │    │  Stores event_tx per device       │
│   }                               │    │                                   │
│                                   │    │  POST /devices/{uuid}/connect     │
│                                   │    │    => device_tx.send(selected)    │
└───────────────────────────────────┘    └───────────────────────────────────┘
          │                                           ▲
          │ WebSocket                                 │ device_rx
          ▼                                           │
┌─────────────────────────┐              ┌───────────┴───────────────────────┐
│     Qobuz Server        │              │       SessionManager.run()        │
│     (qobuz.com)         │              │                                   │
└─────────────────────────┘              │   device_rx.recv() =>             │
                                         │     spawn_session(device.event_tx)│
                                         └───────────────────────────────────┘
```

## Channels

### 1. Device Selection Channel

**Type:** `mpsc::channel<DeviceSelected>`
**Capacity:** 16
**Direction:** DeviceRegistry (HTTP handler) → SessionManager

When a user selects a device in the Qobuz app, the app makes an HTTP POST to our server. The HTTP handler extracts credentials and sends a `DeviceSelected` event.

```rust
struct DeviceSelected {
    device_uuid: [u8; 16],
    session_info: SessionInfo,  // Contains JWT, WebSocket endpoint
}
```

### 2. Per-Device Event Channel

**Type:** `mpsc::channel<SessionEvent>`
**Capacity:** 100
**Direction:** SessionRunner → User Application (one per device)

Each device gets its own event channel, created when `add_device()` is called.
The sender is stored in `DeviceRegistry` and passed to `SessionRunner` when a session starts.
The receiver is returned to the user for handling that device's events.

This design enables:
- Multiple devices with different Qobuz accounts
- Clean event isolation per device
- Independent event handling per device

## Event Types

### Commands (require response via Responder)

| Event | Response Type | When Sent |
|-------|---------------|-----------|
| `PlaybackCommand` | `PlaybackResponse` | Server commands play/pause/seek |
| `Activate` | `ActivationState` | Device becomes active renderer |
| `Heartbeat` | `Option<PlaybackResponse>` | Every 10 seconds while active |

### Events (no response needed)

| Event | When Sent |
|-------|-----------|
| `Deactivated` | Device was deactivated by server |
| `QueueUpdated` | Queue changed (tracks added/removed/reordered) |
| `LoopModeChanged` | Loop mode changed (off/one/all) |
| `ShuffleModeChanged` | Shuffle toggled |
| `RestoreState` | Receiving state from previous active renderer |

### Broadcasts (informational)

| Event | When Sent |
|-------|-----------|
| `Connected` | WebSocket connection established |
| `Disconnected` | WebSocket closed or error |
| `DeviceRegistered` | Our device registered with server |
| `RendererAdded` | Another device joined session |
| `RendererRemoved` | A device left session |
| `ActiveRendererChanged` | Active renderer changed |
| `RendererStateUpdated` | Playback state broadcast from any renderer |
| `VolumeBroadcast` | Volume changed |
| `VolumeMutedBroadcast` | Mute state changed |
| `MaxAudioQualityBroadcast` | Max quality capability changed |
| `FileAudioQualityBroadcast` | Current file's sample rate |
| `SessionClosed` | Session ended |

## Message Flow Examples

### Device Selection Flow

```
1. User opens Qobuz app, sees device in Connect menu
2. User taps device

3. Qobuz app → HTTP Server
   POST /devices/{uuid}/connect-to-qconnect
   Body: { session_id, jwt, ws_endpoint }

4. HTTP Handler → SessionManager (via device_tx)
   DeviceSelected { device_uuid, session_info }

5. SessionManager spawns session
   - Connects WebSocket to Qobuz server
   - Subscribes to session channel
   - Joins session with device info
   - Spawns SessionRunner task

6. SessionRunner → User (via event channels)
   SessionEvent::Connected

7. Server → SessionRunner (WebSocket)
   SrvrCtrlAddRenderer (our device registered)

8. SessionRunner → User
   SessionEvent::DeviceRegistered { device_uuid, renderer_id }
```

### Playback Command Flow

```
1. User presses Play in Qobuz app

2. Qobuz Server → SessionRunner (WebSocket)
   SrvrRndrSetState { playing_state: Playing, position: 0 }

3. SessionRunner creates oneshot channel for response
   let (tx, rx) = oneshot::channel()

4. SessionRunner → User (via event_tx)
   SessionEvent::PlaybackCommand {
       renderer_id,
       cmd: PlaybackCommand { state: Playing, position: Some(0), ... },
       respond: Responder { tx }
   }

5. User handles event, calls respond.send(response)
   respond.send(PlaybackResponse {
       state: Playing,
       position_ms: 0,
       duration_ms: Some(180000),
       ...
   })

6. SessionRunner receives response via rx.await

7. SessionRunner → Qobuz Server (WebSocket)
   RndrSrvrStateUpdated { playing_state: Playing, position: 0, duration: 180000 }
```

### Heartbeat Flow

```
1. SessionRunner heartbeat timer fires (every 10 seconds)

2. SessionRunner → User
   SessionEvent::Heartbeat { renderer_id, respond }

3. User returns current state (or None to skip)
   respond.send(Some(PlaybackResponse { position_ms: 45000, ... }))

4. SessionRunner → Qobuz Server
   RndrSrvrStateUpdated { position: 45000, ... }

5. Server broadcasts to all controllers
   SrvrCtrlRendererStateUpdated (position updated in all Qobuz apps)
```

### Disconnect Flow

```
1. Server deactivates our device (user selected different renderer)

2. Qobuz Server → SessionRunner (WebSocket)
   SrvrRndrSetActive { active: false }

3. SessionRunner → User
   SessionEvent::Deactivated { renderer_id }

4. SessionRunner closes WebSocket

5. SessionRunner → User
   SessionEvent::Disconnected { reason: "Server set inactive" }

6. SessionRunner exits, sends before terminating:
   SessionEvent::SessionClosed { device_uuid }

7. SessionManager forwards SessionClosed to user
```

## Spawned Tasks

| Task | Location | Lifetime | Purpose |
|------|----------|----------|---------|
| HTTP Server | `discovery.rs` | App lifetime | Serves device discovery endpoints |
| SessionManager.run() | `manager.rs` | App lifetime | Routes events, handles device selections |
| SessionRunner | `qconnect.rs` | Per-session | WebSocket handling, event generation |

## Shared State

| State | Type | Location | Purpose |
|-------|------|----------|---------|
| Device Registry | `RwLock<HashMap>` | DeviceRegistry | Device configs + mDNS handles |

## Protocol Details

### WebSocket Envelope Format

Messages are wrapped in a protobuf envelope:

```
┌────────────────┬─────────────────┬──────────────────┐
│ Type (1 byte)  │ Length (varint) │ Payload (proto)  │
└────────────────┴─────────────────┴──────────────────┘

Type 6 = Payload (contains QConnectBatch)
Type 9 = Error
```

### QConnect Message Types

| Type | Name | Direction | Purpose |
|------|------|-----------|---------|
| 23 | RndrSrvrStateUpdated | TX | Report playback state |
| 25 | RndrSrvrVolumeChanged | TX | Report volume |
| 26 | RndrSrvrFileAudioQualityChanged | TX | Report sample rate |
| 28 | RndrSrvrMaxAudioQualityChanged | TX | Report max quality capability |
| 29 | RndrSrvrVolumeMuted | TX | Report mute state |
| 41 | SrvrRndrSetState | RX | Server commands play/pause/seek |
| 43 | SrvrRndrSetActive | RX | Server activates/deactivates us |
| 77 | CtrlSrvrAskForRendererState | TX | Request current state |
| 81 | SrvrCtrlSessionState | RX | Session info on connect |
| 82 | SrvrCtrlRendererStateUpdated | RX | Broadcast state (ignore as renderer) |
| 83 | SrvrCtrlAddRenderer | RX | New renderer joined |
| 85 | SrvrCtrlRemoveRenderer | RX | Renderer left |
| 86 | SrvrCtrlActiveRendererChanged | RX | Active renderer changed |
| 87 | SrvrCtrlVolumeChanged | RX | Broadcast volume |
| 90 | SrvrCtrlQueueState | RX | Full queue snapshot |
| 96 | SrvrCtrlShuffleModeSet | RX | Shuffle mode changed |
| 97 | SrvrCtrlLoopModeSet | RX | Loop mode changed |
| 98 | SrvrCtrlVolumeMuted | RX | Broadcast mute state |
| 99 | SrvrCtrlMaxAudioQualityChanged | RX | Broadcast max quality |
| 100 | SrvrCtrlFileAudioQualityChanged | RX | Broadcast file quality |

TX = Sent by us (renderer)
RX = Received from server
