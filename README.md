# Qonductor

## UNDER ACTIVE DEVELOPMENT, EVERYTHING WILL BREAK

Rust implementation of the Qobuz Connect protocol.

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                            SessionManager                                   │
│  - Main entry point                                                         │
│  - Owns DeviceRegistry and SessionHandles                                   │
│  - Routes events to user via mpsc channel                                   │
└─────────────────────────────────────────────────────────────────────────────┘
         │ owns                                              │ owns
         ▼                                                   ▼
┌─────────────────────────────┐                 ┌──────────────────────────────┐
│     DeviceRegistry          │                 │   SessionHandle (per session)│
│  - 1 HTTP server (axum)     │                 │  - Lightweight handle        │
│  - N mDNS announcements     │                 │  - Sends commands via channel│
│  - Emits DeviceSelected     │                 │  - Spawns SessionRunner task │
└─────────────────────────────┘                 └──────────────────────────────┘
         │                                                   │
         ▼                                                   ▼
┌─────────────────────────────┐                 ┌──────────────────────────────┐
│   Per-Device mDNS Service   │                 │   SessionRunner (spawned)    │
│  _qobuz-connect._tcp.local  │                 │  - Owns WebSocket connection │
│  TXT: path, device_uuid     │                 │  - tokio::select! loop       │
└─────────────────────────────┘                 │  - Handles WS + commands     │
                                                └──────────────────────────────┘
```

## Device Discovery Flow

### 1. Device Registration

When you call `manager.add_device(config)`:

```
┌──────────┐         ┌────────────────┐         ┌─────────────────┐
│   User   │         │ DeviceRegistry │         │  mDNS (Avahi)   │
└────┬─────┘         └───────┬────────┘         └────────┬────────┘
     │                       │                           │
     │  add_device(config)   │                           │
     │──────────────────────>│                           │
     │                       │                           │
     │                       │  Register service         │
     │                       │  "_qobuz-connect._tcp"    │
     │                       │──────────────────────────>│
     │                       │                           │
     │                       │  TXT records:             │
     │                       │  - path=/devices/{uuid}   │
     │                       │  - device_uuid={uuid}     │
     │                       │  - type=SPEAKER           │
     │                       │──────────────────────────>│
     │                       │                           │
     │       Ok(())          │                           │
     │<──────────────────────│                           │
```

The device is now discoverable on the local network via mDNS/Zeroconf.

### 2. Device Selection (Qobuz App → Your Device)

When a user selects your device in the Qobuz app:

```
┌────────────┐      ┌────────────────┐      ┌────────────────┐      ┌─────────────┐
│ Qobuz App  │      │  HTTP Server   │      │ DeviceRegistry │      │   Manager   │
└─────┬──────┘      └───────┬────────┘      └───────┬────────┘      └──────┬──────┘
      │                     │                       │                      │
      │ GET /devices/{uuid}/get-display-info        │                      │
      │────────────────────>│                       │                      │
      │    { name, type }   │                       │                      │
      │<────────────────────│                       │                      │
      │                     │                       │                      │
      │ GET /devices/{uuid}/get-connect-info        │                      │
      │────────────────────>│                       │                      │
      │   { app_id }        │                       │                      │
      │<────────────────────│                       │                      │
      │                     │                       │                      │
      │ POST /devices/{uuid}/connect-to-qconnect    │                      │
      │ { session_id, jwt_qconnect, jwt_api }       │                      │
      │────────────────────>│                       │                      │
      │                     │  DeviceSelected       │                      │
      │                     │──────────────────────>│                      │
      │                     │                       │  DeviceSelected      │
      │                     │                       │─────────────────────>│
      │    { success }      │                       │                      │
      │<────────────────────│                       │                      │
```

### 3. WebSocket Session Creation

When SessionManager receives DeviceSelected:

```
┌─────────────┐      ┌───────────────┐      ┌────────────────┐      ┌─────────────┐
│   Manager   │      │ SessionHandle │      │ SessionRunner  │      │ Qobuz WS    │
└──────┬──────┘      └───────┬───────┘      └───────┬────────┘      └──────┬──────┘
       │                     │                      │                      │
       │ SessionHandle::connect(session_info, device_config)               │
       │────────────────────>│                      │                      │
       │                     │                      │                      │
       │                     │  Connect WebSocket   │                      │
       │                     │────────────────────────────────────────────>│
       │                     │                      │                      │
       │                     │  Subscribe + Join    │                      │
       │                     │────────────────────────────────────────────>│
       │                     │                      │                      │
       │                     │  Spawn runner task   │                      │
       │                     │─────────────────────>│                      │
       │                     │                      │                      │
       │    SessionHandle    │                      │   tokio::select! {   │
       │<────────────────────│                      │     ws.recv()        │
       │                     │                      │     cmd_rx.recv()    │
       │                     │                      │   }                  │
       │                     │                      │<────────────────────>│
```

### 4. Event Flow

Events from Qobuz server flow to user code:

```
┌─────────────┐      ┌───────────────┐      ┌─────────────┐       ┌──────────┐
│  Qobuz WS   │      │ SessionRunner │      │   Manager   │       │   User   │
└──────┬──────┘      └───────┬───────┘      └──────┬──────┘       └────┬─────┘
       │                     │                     │                   │
       │  PlaybackCommand    │                     │                   │
       │────────────────────>│                     │                   │
       │                     │                     │                   │
       │                     │  event_tx.send()    │                   │
       │                     │────────────────────>│                   │
       │                     │                     │                   │
       │                     │                     │  events.recv()    │
       │                     │                     │──────────────────>│
       │                     │                     │                   │
       │                     │                     │  SessionEvent::   │
       │                     │                     │  PlaybackCommand  │
       │                     │                     │──────────────────>│
```

## Usage

```rust
use qonductor::{SessionManager, DeviceConfig, SessionEvent};

#[tokio::main]
async fn main() -> qonductor::Result<()> {
    // Start the session manager (HTTP server + mDNS)
    let (mut manager, mut events) = SessionManager::start(7864).await?;

    // Register devices for discovery
    manager.add_device(DeviceConfig::new("Living Room Speaker", "your_app_id")).await?;
    manager.add_device(DeviceConfig::new("Kitchen Speaker", "your_app_id")).await?;

    // Spawn manager to handle device selections
    tokio::spawn(async move { manager.run().await });

    // Handle events
    while let Some(event) = events.recv().await {
        match event {
            SessionEvent::Connected => println!("Connected!"),
            SessionEvent::DeviceRegistered { renderer_id, .. } => {
                println!("Registered as renderer {}", renderer_id);
            }
            SessionEvent::PlaybackCommand { state, position_ms, .. } => {
                println!("Play {:?} at {:?}ms", state, position_ms);
            }
            SessionEvent::QueueUpdated { tracks, .. } => {
                println!("Queue has {} tracks", tracks.len());
            }
            _ => {}
        }
    }

    Ok(())
}
```

## Key Types

| Type             | Description                                                |
| ---------------- | ---------------------------------------------------------- |
| `SessionManager` | Main entry point. Manages devices and sessions.            |
| `DeviceConfig`   | Configuration for a discoverable device.                   |
| `SessionEvent`   | Events from Qobuz (playback commands, queue updates, etc.) |
| `PlayingState`   | Playback state: `Playing`, `Paused`, `Stopped`             |

## How It Works

1. **mDNS Advertisement**: Each device is advertised via `_qobuz-connect._tcp` with a unique path in the TXT record.

2. **HTTP Endpoints**: A single HTTP server handles all devices via parameterized routes (`/devices/{uuid}/*`). Qobuz apps hit these endpoints when the device is selected.

3. **1:1 Device-Session Mapping**: When a device is selected in the Qobuz app, a dedicated session is created for that device between Qonductor and the Qobuz servers.

4. **Actor Pattern**: Each WebSocket session runs in its own spawned task, communicating with the manager via channels.

## Building

```bash
cargo build
cargo run --example discovery_server
```

## License

MIT
