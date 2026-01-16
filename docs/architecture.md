# Qonductor Architecture

This document describes the async communication patterns and message flow in Qonductor.

## Overview

Qonductor has three main async components that communicate via channels.
Each device gets bidirectional communication via a `DeviceSession` handle:

```
┌───────────────────────────────────────────────────────────────────────────┐
│                          User Application                                 │
│                                                                           │
│   let mut session = manager.add_device(config).await?;                    │
│   while let Some(event) = session.recv().await {                          │
│       match event {                                                       │
│           SessionEvent::Command(Command::SetState { cmd, respond }) => .. │
│           SessionEvent::Notification(Notification::QueueState(q)) => ..   │
│       }                                                                   │
│       // Player can also initiate state changes                           │
│       session.report_state(response).await?;                              │
│   }                                                                       │
└───────────────────────────────────────────────────────────────────────────┘
          ▲                                         │
          │ event_tx                                │ command_tx
          │ (per device)                            │ (via SharedCommandTx)
          │                                         ▼
┌─────────┴─────────────────────────────────────────┴───────────────────────┐
│                      SessionRunner (per device)                           │
│                                                                           │
│   tokio::select! {                                                        │
│       ws.recv() => send event to user                                     │
│       heartbeat => send Heartbeat event                                   │
│       command_rx.recv() => send command to WebSocket                      │
│   }                                                                       │
└───────────────────────────────────────────────────────────────────────────┘
          │
          │ WebSocket (bidirectional)
          ▼
┌─────────────────────────┐
│     Qobuz Server        │
│     (qobuz.com)         │
└─────────────────────────┘

┌───────────────────────────────────────────────────────────────────────────┐
│                         DeviceRegistry                                    │
│                                                                           │
│   HTTP Server (Axum)                                                      │
│   mDNS Broadcast (zeroconf)                                               │
│   Stores per device: event_tx, SharedCommandTx                            │
│                                                                           │
│   POST /devices/{uuid}/connect => device_tx.send(selected)                │
└───────────────────────────────────────────────────────────────────────────┘
                                         ▲
                                         │ device_rx
                                         │
┌────────────────────────────────────────┴──────────────────────────────────┐
│                       SessionManager.run()                                │
│                                                                           │
│   device_rx.recv() => {                                                   │
│       // Create command channel for this session                          │
│       let (command_tx, command_rx) = mpsc::channel();                     │
│       *shared_command_tx.write() = Some(command_tx);                      │
│       spawn_session(event_tx, command_rx)                                 │
│   }                                                                       │
└───────────────────────────────────────────────────────────────────────────┘
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

### 2. Per-Device Bidirectional Channels

Each device gets bidirectional communication via a `DeviceSession` handle:

**Event Channel (server → user):**

- **Type:** `mpsc::channel<SessionEvent>`
- **Capacity:** 100
- **Direction:** SessionRunner → User Application
- **Created:** When `add_device()` is called

**Command Channel (user → server):**

- **Type:** `mpsc::channel<SessionCommand>`
- **Capacity:** 100
- **Direction:** User Application → SessionRunner
- **Created:** When Qobuz app connects (session starts)

**SharedCommandTx:**

- **Type:** `Arc<RwLock<Option<mpsc::Sender<SessionCommand>>>>`
- Shared between `DeviceSession` and `DeviceRegistry`
- Initialized to `None` when `add_device()` is called
- Set to `Some(sender)` when session starts
- Allows `DeviceSession::report_*()` to send commands once connected

This design enables:

- Multiple devices with different Qobuz accounts
- Clean event isolation per device
- Bidirectional communication (player can initiate state changes)
- Multiple reconnects (new channel created each time)
- Clear error if sending before connected

## Event Types

Events are delivered via `SessionEvent`, which has two categories:
- `SessionEvent::Command(Command)` - Must respond via `Responder<T>`
- `SessionEvent::Notification(Notification)` - Informational (non-exhaustive, use `_ =>`)

### Commands (require response via Responder)

| Command     | Response Type                 | When Sent                       |
| ----------- | ----------------------------- | ------------------------------- |
| `SetState`  | `QueueRendererState`          | Server commands play/pause/seek |
| `SetActive` | `ActivationState`             | Device becomes active renderer  |
| `Heartbeat` | `Option<QueueRendererState>`  | Every 10 seconds while active   |

### Notifications (informational, non-exhaustive)

| Notification              | When Sent                                      |
| ------------------------- | ---------------------------------------------- |
| `Connected`               | WebSocket connection established               |
| `Disconnected`            | WebSocket closed or error                      |
| `DeviceRegistered`        | Our device registered with server              |
| `SessionClosed`           | Session ended                                  |
| `QueueState`              | Full queue snapshot received                   |
| `QueueTracksAdded`        | Tracks added to queue                          |
| `QueueTracksRemoved`      | Tracks removed from queue                      |
| `QueueTracksReordered`    | Tracks reordered in queue                      |
| `LoopModeSet`             | Loop mode changed (off/one/all)                |
| `ShuffleModeSet`          | Shuffle toggled                                |
| `Deactivated`             | Device was deactivated by server               |
| `RestoreState`            | Receiving state from previous active renderer  |
| `AddRenderer`             | Another device joined session                  |
| `RemoveRenderer`          | A device left session                          |
| `ActiveRendererChanged`   | Active renderer changed                        |
| `RendererStateUpdated`    | Playback state broadcast from any renderer     |
| `VolumeChanged`           | Volume changed                                 |
| `VolumeMuted`             | Mute state changed                             |
| `MaxAudioQualityChanged`  | Max quality capability changed                 |
| `FileAudioQualityChanged` | Current file's sample rate                     |

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
   SessionEvent::Notification(Notification::Connected)

7. Server → SessionRunner (WebSocket)
   SrvrCtrlAddRenderer (our device registered)

8. SessionRunner → User
   SessionEvent::Notification(Notification::DeviceRegistered { device_uuid, renderer_id, api_jwt })
```

### Playback Command Flow

```
1. User presses Play in Qobuz app

2. Qobuz Server → SessionRunner (WebSocket)
   SrvrRndrSetState { playing_state: Playing, position: 0 }

3. SessionRunner creates oneshot channel for response
   let (tx, rx) = oneshot::channel()

4. SessionRunner → User (via event_tx)
   SessionEvent::Command(Command::SetState {
       cmd: msg::cmd::SetState { playing_state: Some(Playing), ... },
       respond: Responder { tx }
   })

5. User handles event, calls respond.send(response)
   respond.send(msg::QueueRendererState {
       playing_state: Some(Playing),
       current_position: Some(Position::now(0)),
       duration: Some(180000),
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
   SessionEvent::Command(Command::Heartbeat { respond })

3. User returns current state (or None to skip)
   respond.send(Some(msg::QueueRendererState { current_position: Some(Position::now(45000)), ... }))

4. SessionRunner → Qobuz Server
   RndrSrvrStateUpdated { position: 45000, ... }

5. Server broadcasts to all controllers
   SrvrCtrlRendererStateUpdated (position updated in all Qobuz apps)
```

### User-Initiated Command Flow

```
1. User pauses playback locally (not from Qobuz app)

2. User Application calls session.report_state()
   session.report_state(msg::QueueRendererState {
       playing_state: Some(Paused),
       current_position: Some(Position::now(45000)),
       ...
   }).await?

3. DeviceSession reads SharedCommandTx, sends command
   command_tx.send(SessionCommand::ReportState(response))

4. SessionRunner receives via command_rx.recv()

5. SessionRunner → Qobuz Server (WebSocket)
   RndrSrvrStateUpdated { playing_state: Paused, position: 45000 }

6. Server broadcasts to all controllers
   SrvrCtrlRendererStateUpdated (all Qobuz apps show paused)
```

### Disconnect Flow

```
1. Server deactivates our device (user selected different renderer)

2. Qobuz Server → SessionRunner (WebSocket)
   SrvrRndrSetActive { active: false }

3. SessionRunner → User
   SessionEvent::Notification(Notification::Deactivated)

4. SessionRunner closes WebSocket

5. SessionRunner → User
   SessionEvent::Notification(Notification::Disconnected { session_id, reason: Some("Server set inactive") })

6. SessionRunner exits, sends before terminating:
   SessionEvent::Notification(Notification::SessionClosed { device_uuid })

7. SessionManager forwards SessionClosed to user
```

## Spawned Tasks

| Task                 | Location       | Lifetime     | Purpose                                  |
| -------------------- | -------------- | ------------ | ---------------------------------------- |
| HTTP Server          | `discovery.rs` | App lifetime | Serves device discovery endpoints        |
| SessionManager.run() | `manager.rs`   | App lifetime | Routes events, handles device selections |
| SessionRunner        | `qconnect.rs`  | Per-session  | WebSocket handling, event generation     |

## Shared State

| State           | Type              | Location       | Purpose                       |
| --------------- | ----------------- | -------------- | ----------------------------- |
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

| Type | Name                            | Direction | Purpose                              |
| ---- | ------------------------------- | --------- | ------------------------------------ |
| 23   | RndrSrvrStateUpdated            | TX        | Report playback state                |
| 25   | RndrSrvrVolumeChanged           | TX        | Report volume                        |
| 26   | RndrSrvrFileAudioQualityChanged | TX        | Report sample rate                   |
| 28   | RndrSrvrMaxAudioQualityChanged  | TX        | Report max quality capability        |
| 29   | RndrSrvrVolumeMuted             | TX        | Report mute state                    |
| 41   | SrvrRndrSetState                | RX        | Server commands play/pause/seek      |
| 43   | SrvrRndrSetActive               | RX        | Server activates/deactivates us      |
| 77   | CtrlSrvrAskForRendererState     | TX        | Request current state                |
| 81   | SrvrCtrlSessionState            | RX        | Session info on connect              |
| 82   | SrvrCtrlRendererStateUpdated    | RX        | Broadcast state (ignore as renderer) |
| 83   | SrvrCtrlAddRenderer             | RX        | New renderer joined                  |
| 85   | SrvrCtrlRemoveRenderer          | RX        | Renderer left                        |
| 86   | SrvrCtrlActiveRendererChanged   | RX        | Active renderer changed              |
| 87   | SrvrCtrlVolumeChanged           | RX        | Broadcast volume                     |
| 90   | SrvrCtrlQueueState              | RX        | Full queue snapshot                  |
| 96   | SrvrCtrlShuffleModeSet          | RX        | Shuffle mode changed                 |
| 97   | SrvrCtrlLoopModeSet             | RX        | Loop mode changed                    |
| 98   | SrvrCtrlVolumeMuted             | RX        | Broadcast mute state                 |
| 99   | SrvrCtrlMaxAudioQualityChanged  | RX        | Broadcast max quality                |
| 100  | SrvrCtrlFileAudioQualityChanged | RX        | Broadcast file quality               |

TX = Sent by us (renderer)
RX = Received from server
