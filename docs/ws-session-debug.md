# QConnect WebSocket Session Debug Log

Captured session between Qobuz Desktop (renderer) and Qobuz Web (controller).

**Setup:** Web player selected as active renderer, then clicked play on Desktop.

## Message Timeline

### 1. RX: Server Command - Play

```
Raw: 062608cf1510e8ae8eaab33318012201023a1509689743359b0100001089151a070829ca02020802
```

| Field              | Value                 |
| ------------------ | --------------------- |
| Envelope type      | 6 (PAYLOAD)           |
| msg_id             | 2767                  |
| msg_date           | 1766125180776         |
| Message type       | 41 (SrvrRndrSetState) |
| playing_state      | **Playing**           |
| current_position   | None                  |
| queue_version      | None                  |
| current_queue_item | None                  |

**Analysis:** Server tells renderer to start playing. No position specified - resume from current.

---

### 2. TX: Renderer Reports State

```
Raw: 064408101091af8eaab33318012a01023a3409919743359b010000100e1a270817ba01220a20080210021a0c09839743359b01000010f82f20bdc4112a040802100130013802
```

| Field                 | Value                     |
| --------------------- | ------------------------- |
| Envelope type         | 6 (PAYLOAD)               |
| msg_id                | 16                        |
| msg_date              | 1766125180817 (+41ms)     |
| Message type          | 23 (RndrSrvrStateUpdated) |
| playing_state         | Playing                   |
| buffer_state          | Ok                        |
| position.timestamp    | 1766125180803             |
| position.value        | **6136ms**                |
| duration              | 287293ms (~4:47)          |
| queue_version         | major=2, minor=1          |
| current_queue_item_id | 1                         |
| next_queue_item_id    | 2                         |

**Analysis:** Renderer immediately reports current state after receiving play command.

---

### 3. TX: Renderer Reports State (duplicate)

```
Raw: 064408111096af8eaab33318012a01023a3409969743359b010000100f1a270817ba01220a20080210021a0c09959743359b01000010f82f20bdc4112a040802100130013802
```

| Field              | Value                              |
| ------------------ | ---------------------------------- |
| msg_id             | 17                                 |
| msg_date           | 1766125180822 (+5ms from previous) |
| position.timestamp | 1766125180821                      |
| position.value     | **6136ms** (same)                  |

**Analysis:** Second state report 5ms later. Possibly due to internal state machine or debouncing issue in real player.

---

### 4. RX: Server Broadcasts State

```
Raw: 064208d01510d0af8eaab33318012201023a3109d09743359b010000108a151a23085292051e080910011a18080210021a0c09839743359b01000010f82f20bdc4112801
```

| Field               | Value                             |
| ------------------- | --------------------------------- |
| msg_id              | 2768                              |
| msg_date            | 1766125180880 (+58ms from TX)     |
| Message type        | 82 (SrvrCtrlRendererStateUpdated) |
| renderer_id         | 9                                 |
| message_id          | 1                                 |
| playing_state       | Playing                           |
| buffer_state        | Ok                                |
| position.timestamp  | 1766125180803                     |
| position.value      | 6136ms                            |
| duration            | 287293ms                          |
| current_queue_index | 1                                 |

**Analysis:** Server broadcasts renderer state to all clients. This is an echo of TX #2.

---

### 5. RX: Server Broadcasts State (echo of TX #3)

```
Raw: 064208d11510d3af8eaab33318012201023a3109d39743359b010000108b151a23085292051e080910011a18080210021a0c09959743359b01000010f82f20bdc4112801
```

| Field              | Value                |
| ------------------ | -------------------- |
| msg_id             | 2769                 |
| msg_date           | 1766125180883 (+3ms) |
| position.timestamp | 1766125180821        |
| position.value     | 6136ms               |

**Analysis:** Server echoes the second TX.

---

### ⏱️ 10 SECOND DELAY

---

### 6. TX: Heartbeat #1

```
Raw: 0645081210eeff8eaab33318012a01023a3509eebf43359b01000010101a280817ba01230a21080210021a0d09ecbf43359b010000108c800120bdc4112a040802100130013802
```

| Field                 | Value                     |
| --------------------- | ------------------------- |
| msg_id                | 18                        |
| msg_date              | 1766125191150 (+10.3s)    |
| Message type          | 23 (RndrSrvrStateUpdated) |
| playing_state         | Playing                   |
| position.timestamp    | 1766125191148             |
| position.value        | **16396ms** (+10260ms)    |
| duration              | 287293ms                  |
| current_queue_item_id | 1                         |

**Analysis:** 10-second heartbeat. Position advanced by ~10.26 seconds.

---

### 7. RX: Server Confirms Heartbeat #1

```
Raw: 064308d21510aa808faab33318012201023a32092ac043359b010000108c151a24085292051f080910011a19080210021a0d09ecbf43359b010000108c800120bdc4112801
```

| Field          | Value                             |
| -------------- | --------------------------------- |
| msg_id         | 2770                              |
| msg_date       | 1766125191210 (+60ms)             |
| Message type   | 82 (SrvrCtrlRendererStateUpdated) |
| position.value | 16396ms                           |

---

### ⏱️ 10 SECOND DELAY

---

### 8. TX: Heartbeat #2

```
Raw: 0645081310d9ce8faab33318012a01023a350959e743359b01000010111a280817ba01230a21080210021a0d0958e743359b01000010f8ce0120bdc4112a040802100130013802
```

| Field              | Value                  |
| ------------------ | ---------------------- |
| msg_id             | 19                     |
| msg_date           | 1766125201241 (+10.1s) |
| position.timestamp | 1766125201240          |
| position.value     | **26488ms** (+10092ms) |

---

### 9. RX: Server Confirms Heartbeat #2

```
Raw: 064308d3151096cf8faab33318012201023a320996e743359b010000108d151a24085292051f080910011a19080210021a0d0958e743359b01000010f8ce0120bdc4112801
```

| Field          | Value                 |
| -------------- | --------------------- |
| msg_id         | 2771                  |
| msg_date       | 1766125201302 (+61ms) |
| position.value | 26488ms               |

---

### 🎵 USER CLICKED PAUSE

---

### 10. RX: Server Command - Pause

```
Raw: 062608d51510a2fa8faab33318012201023a150922fd43359b010000108f151a070829ca02020803
```

| Field            | Value                 |
| ---------------- | --------------------- |
| msg_id           | 2773                  |
| msg_date         | 1766125206818 (+5.5s) |
| Message type     | 41 (SrvrRndrSetState) |
| playing_state    | **Paused**            |
| current_position | None                  |

**Analysis:** Server tells renderer to pause.

---

### 11. TX: Renderer Reports Paused

```
Raw: 0645081410c5fa8faab33318012a01023a350945fd43359b01000010121a280817ba01230a21080310021a0d0936fd43359b01000010d6fa0120bdc4112a040802100130013802
```

| Field                 | Value                     |
| --------------------- | ------------------------- |
| msg_id                | 20                        |
| msg_date              | 1766125206853 (+35ms)     |
| Message type          | 23 (RndrSrvrStateUpdated) |
| playing_state         | **Paused**                |
| position.timestamp    | 1766125206838             |
| position.value        | **32086ms**               |
| duration              | 287293ms                  |
| current_queue_item_id | 1                         |

**Analysis:** Renderer immediately reports paused state.

---

### 12. RX: Server Confirms Pause

```
Raw: 064308d6151086fb8faab33318012201023a320986fd43359b0100001090151a24085292051f080910011a19080310021a0d0936fd43359b01000010d6fa0120bdc4112801
```

| Field          | Value                             |
| -------------- | --------------------------------- |
| msg_id         | 2774                              |
| msg_date       | 1766125206918 (+65ms)             |
| Message type   | 82 (SrvrCtrlRendererStateUpdated) |
| playing_state  | Paused                            |
| position.value | 32086ms                           |

---

## Summary

### Message Flow Pattern

```
Controller Action (e.g., click Play)
    │
    ▼
Server sends SrvrRndrSetState (type 41) to renderer
    │
    ▼
Renderer updates internal state
    │
    ▼
Renderer sends RndrSrvrStateUpdated (type 23) to server
    │
    ▼
Server broadcasts SrvrCtrlRendererStateUpdated (type 82) to ALL clients
    │
    ▼
(Renderer should IGNORE this broadcast - it's for other clients)
```

### Key Findings

1. **Renderer DOES respond to commands** - `SrvrRndrSetState` (type 41) triggers immediate state report.

2. **Renderer does NOT respond to broadcasts** - `SrvrCtrlRendererStateUpdated` (type 82) is ignored.

3. **Heartbeat interval** - 10 seconds (matches C++ default `Heartbeat(..., 10000)`).

4. **State report includes:**
   - `playing_state` (Playing/Paused/Stopped)
   - `buffer_state` (Ok)
   - `current_position` (timestamp + value in ms)
   - `duration` (track duration in ms)
   - `queue_version` (major.minor)
   - `current_queue_item_id`
   - `next_queue_item_id`

5. **Position calculation** - Renderer tracks position as timestamp + offset, calculates current position as: `now - timestamp + value`.

### Message Types Reference

| Type | Name                         | Direction | Purpose                          |
| ---- | ---------------------------- | --------- | -------------------------------- |
| 23   | RndrSrvrStateUpdated         | TX        | Renderer reports its state       |
| 41   | SrvrRndrSetState             | RX        | Server commands renderer         |
| 82   | SrvrCtrlRendererStateUpdated | RX        | Server broadcasts state (ignore) |
