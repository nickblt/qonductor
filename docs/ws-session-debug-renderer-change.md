# QConnect WebSocket Session Debug Log - Renderer Change

Captured session showing what happens when selecting a new renderer as active.

**Setup:** Session already running with renderer ID 7 active. User selects our fake renderer (ID 9) as the new active renderer.

## Message Timeline

### 1. RX: Server Notifies Renderer Activation Pending

```
Raw: 06240819150aaa f9e6aab333180122010 23a1309aabc59359b0100001093151a0508 2bda0200
Base64: BiQI2RUQqvnmqrMzGAEiAQI6EwmqvFk1mwEAABCTFRoFCCvaAgA=
```

| Field         | Value                  |
| ------------- | ---------------------- |
| Envelope type | 6 (PAYLOAD)            |
| msg_id        | 2777                   |
| msg_date      | 1766126632106          |
| Message type  | 43 (SrvrRndrSetActive) |
| active        | **None** (pending)     |

**Analysis:** Server notifies us that we're about to be made active. `active: None` indicates pending state.

---

### 2. RX: Server Broadcasts Active Renderer Changed

```
Base64: BiYI2xUQqvnmqrMzGAEiAQI6FQmqvFk1mwEAABCVFRoHCFayBQIIBw==
```

| Field       | Value                              |
| ----------- | ---------------------------------- |
| msg_id      | 2779                               |
| msg_date    | 1766126632106 (+0ms)               |
| Message type| 86 (SrvrCtrlActiveRendererChanged) |
| renderer_id | **7**                              |

**Analysis:** Server broadcasts that renderer 7 is currently active (the one we're taking over from).

---

### 3. RX: Server Sends Volume Mute State

```
Base64: BiYI3BUQsPnmqrMzGAEiAQI6FQmwvFk1mwEAABCWFRoHCGKSBgIIBw==
```

| Field       | Value                     |
| ----------- | ------------------------- |
| msg_id      | 2780                      |
| msg_date    | 1766126632112 (+6ms)      |
| Message type| 98 (SrvrCtrlVolumeMuted)  |
| renderer_id | 7                         |
| value       | None (not muted)          |

**Analysis:** Server sends current mute state of renderer 7.

---

### 4. RX: Server Sends Volume Level

```
Base64: BigI3RUQsvnmqrMzGAEiAQI6FwmyvFk1mwEAABCXFRoJCFe6BQQIBxBk
```

| Field       | Value                       |
| ----------- | --------------------------- |
| msg_id      | 2781                        |
| msg_date    | 1766126632114 (+2ms)        |
| Message type| 87 (SrvrCtrlVolumeChanged)  |
| renderer_id | 7                           |
| volume      | **100**                     |

**Analysis:** Server sends current volume of renderer 7 (100%).

---

### 5. RX: Server Sends Max Audio Quality

```
Base64: BigI3hUQuvnmqrMzGAEiAQI6Fwm6vFk1mwEAABCYFRoJCGOaBgQIBxAE
```

| Field             | Value                                |
| ----------------- | ------------------------------------ |
| msg_id            | 2782                                 |
| msg_date          | 1766126632122 (+8ms)                 |
| Message type      | 99 (SrvrCtrlMaxAudioQualityChanged)  |
| max_audio_quality | 7 (renderer 7)                       |

**Analysis:** Server sends max audio quality setting.

---

### 6. RX: Server Sends Renderer State (Buffering)

```
Base64: Bj8I3xUQvPnmqrMzGAEiAQI6Lgm8vFk1mwEAABCZFRogCFKSBRsIBxABGhUIAxABGg0JeLxZNZsBAAAQ1voBKAE=
```

| Field               | Value                             |
| ------------------- | --------------------------------- |
| msg_id              | 2783                              |
| msg_date            | 1766126632124 (+2ms)              |
| Message type        | 82 (SrvrCtrlRendererStateUpdated) |
| renderer_id         | 7                                 |
| playing_state       | **Paused**                        |
| buffer_state        | **Buffering**                     |
| position.timestamp  | 1766126632056                     |
| position.value      | 32086ms                           |
| current_queue_index | 1                                 |

**Analysis:** Server sends renderer 7's current playback state. Position is 32086ms (~32 seconds into track).

---

### 7. RX: Server Sends Renderer State (with duration)

```
Base64: BkMI4BUQ5/vmqrMzGAEiAQI6MgnnvVk1mwEAABCaFRokCFKSBR8IBxABGhkIAxABGg0JeLxZNZsBAAAQ1voBIL3EESgB
```

| Field               | Value                             |
| ------------------- | --------------------------------- |
| msg_id              | 2784                              |
| msg_date            | 1766126632423 (+299ms)            |
| Message type        | 82 (SrvrCtrlRendererStateUpdated) |
| renderer_id         | 7                                 |
| playing_state       | Paused                            |
| buffer_state        | Buffering                         |
| position.value      | 32086ms                           |
| duration            | **287293ms** (~4:47)              |
| current_queue_index | 1                                 |

**Analysis:** Same state but now includes track duration.

---

### 8. RX: Server Sends File Audio Quality

```
Base64: BjAI4RUQ6fvmqrMzGAEiAQI6HwnpvVk1mwEAABCbFRoRCGSiBgwIBxDE2AIYECACKAI=
```

| Field              | Value                                |
| ------------------ | ------------------------------------ |
| msg_id             | 2785                                 |
| msg_date           | 1766126632425 (+2ms)                 |
| Message type       | 100 (SrvrCtrlFileAudioQualityChanged)|
| file_audio_quality | 7                                    |

**Analysis:** Server sends file's audio quality level.

---

### 9. RX: Server Sends Renderer State (Buffered)

```
Base64: BkMI4hUQ3/zmqrMzGAEiAQI6Mglfvlk1mwEAABCcFRokCFKSBR8IBxABGhkIAxACGg0JG75ZNZsBAAAQgPoBIL3EESgB
```

| Field               | Value                             |
| ------------------- | --------------------------------- |
| msg_id              | 2786                              |
| msg_date            | 1766126632543 (+118ms)            |
| Message type        | 82 (SrvrCtrlRendererStateUpdated) |
| renderer_id         | 7                                 |
| playing_state       | Paused                            |
| buffer_state        | **Ok** (buffered)                 |
| position.value      | 32000ms                           |
| duration            | 287293ms                          |

**Analysis:** Buffer state changed from Buffering to Ok.

---

### ⏱️ ~36 SECOND DELAY (Heartbeat/Keepalive)

---

### 10. RX: Heartbeat - Renderer State Repeat

```
Base64: BkMI4xUQupXpqrMzGAEiAQI6Mgm6Slo1mwEAABCdFRokCFKSBR8IBxABGhkIAxACGg0JG75ZNZsBAAAQgPoBIL3EESgB
```

| Field        | Value                  |
| ------------ | ---------------------- |
| msg_id       | 2787                   |
| msg_date     | 1766126668474 (+36s)   |
| Message type | 82                     |
| position     | 32000ms (unchanged)    |

---

### 11. RX: File Audio Quality Repeat

```
Base64: BjAI5BUQvZXpqrMzGAEiAQI6Hwm9Slo1mwEAABCeFRoRCGSiBgwIBxDE2AIYECACKAI=
```

| Field        | Value |
| ------------ | ----- |
| msg_id       | 2788  |
| msg_date     | 1766126668477 (+3ms) |
| Message type | 100   |

---

### 🎯 USER SELECTS US AS ACTIVE RENDERER

---

### 12. RX: Server Activates Us

```
Base64: BiYI5hUQmLPpqrMzGAEiAQI6FQmYWVo1mwEAABCgFRoHCCvaAgIIAQ==
```

| Field         | Value                  |
| ------------- | ---------------------- |
| msg_id        | 2790                   |
| msg_date      | 1766126672280 (+3.8s)  |
| Message type  | 43 (SrvrRndrSetActive) |
| active        | **true**               |

**Analysis:** Server tells us we are now the active renderer!

---

### 13. TX: Report Volume Mute State

```
Base64: BiIIFRDAs+mqszMYASoBAjoSCcBZWjWbAQAAEBMaBQgd6gEA
```

| Field        | Value                     |
| ------------ | ------------------------- |
| msg_id       | 21                        |
| msg_date     | 1766126672320 (+40ms)     |
| Message type | 29 (RndrSrvrVolumeMuted)  |
| value        | None (not muted)          |

**Analysis:** We report our mute state to server.

---

### 14. TX: Report Volume Level

```
Base64: BiQIFhDAs+mqszMYASoBAjoUCcBZWjWbAQAAEBQaBwgZygECCGQ=
```

| Field        | Value                      |
| ------------ | -------------------------- |
| msg_id       | 22                         |
| msg_date     | 1766126672320 (+0ms)       |
| Message type | 25 (RndrSrvrVolumeChanged) |
| volume       | **100**                    |

**Analysis:** We report volume level (matching what server told us).

---

### 15. TX: Report Max Audio Quality

```
Base64: BiQIFxDBs+mqszMYASoBAjoUCcFZWjWbAQAAEBUaBwgc4gECCAM=
```

| Field        | Value                              |
| ------------ | ---------------------------------- |
| msg_id       | 23                                 |
| msg_date     | 1766126672321 (+1ms)               |
| Message type | 28 (RndrSrvrMaxAudioQualityChanged)|
| value        | **3**                              |

**Analysis:** We report our max audio quality capability (3 = CD quality?).

---

### 16. TX: Report Playback State

```
Base64: BkEIGBDCs+mqszMYASoBAjoxCcJZWjWbAQAAEBYaJAgXugEfCh0IAxABGg0JwVlaNZsBAAAQgPoBKgQIAhABMAE4Ag==
```

| Field                 | Value                     |
| --------------------- | ------------------------- |
| msg_id                | 24                        |
| msg_date              | 1766126672322 (+1ms)      |
| Message type          | 23 (RndrSrvrStateUpdated) |
| playing_state         | **Paused**                |
| buffer_state          | **Buffering**             |
| position.timestamp    | 1766126672321             |
| position.value        | **32000ms**               |
| queue_version         | major=2, minor=1          |
| current_queue_item_id | 1                         |
| next_queue_item_id    | 2                         |

**Analysis:** We report our playback state, starting from where renderer 7 left off.

---

### 17. RX: Server Broadcasts Active Renderer Changed (to us)

```
Base64: BiYI5xUQmLPpqrMzGAEiAQI6FQmYWVo1mwEAABChFRoHCFayBQIICQ==
```

| Field        | Value                              |
| ------------ | ---------------------------------- |
| msg_id       | 2791                               |
| msg_date     | 1766126672280                      |
| Message type | 86 (SrvrCtrlActiveRendererChanged) |
| renderer_id  | **9** (us!)                        |

**Analysis:** Server broadcasts that renderer 9 (us) is now active.

---

### 18. RX: Server Broadcasts Our Mute State

```
Base64: BiYI6BUQ8rPpqrMzGAEiAQI6FQnyWVo1mwEAABCiFRoHCGKSBgIICQ==
```

| Field        | Value                    |
| ------------ | ------------------------ |
| msg_id       | 2792                     |
| msg_date     | 1766126672370 (+90ms)    |
| Message type | 98 (SrvrCtrlVolumeMuted) |
| renderer_id  | 9                        |
| value        | None (not muted)         |

---

### 19. RX: Server Broadcasts Our Volume

```
Base64: BigI6RUQ9bPpqrMzGAEiAQI6Fwn1WVo1mwEAABCjFRoJCFe6BQQICRBk
```

| Field        | Value                      |
| ------------ | -------------------------- |
| msg_id       | 2793                       |
| msg_date     | 1766126672373 (+3ms)       |
| Message type | 87 (SrvrCtrlVolumeChanged) |
| renderer_id  | 9                          |
| volume       | 100                        |

---

### 20. RX: Server Broadcasts Our Max Audio Quality

```
Base64: BigI6hUQ+rPpqrMzGAEiAQI6Fwn6WVo1mwEAABCkFRoJCGOaBgQICRAD
```

| Field             | Value                               |
| ----------------- | ----------------------------------- |
| msg_id            | 2794                                |
| msg_date          | 1766126672378 (+5ms)                |
| Message type      | 99 (SrvrCtrlMaxAudioQualityChanged) |
| max_audio_quality | 3                                   |

---

### 21. RX: Server Broadcasts Our State (Buffering)

```
Base64: Bj8I6xUQ/LPpqrMzGAEiAQI6Lgn8WVo1mwEAABClFRogCFKSBRsICRABGhUIAxABGg0JwVlaNZsBAAAQgPoBKAE=
```

| Field               | Value                             |
| ------------------- | --------------------------------- |
| msg_id              | 2795                              |
| msg_date            | 1766126672380 (+2ms)              |
| Message type        | 82 (SrvrCtrlRendererStateUpdated) |
| renderer_id         | 9                                 |
| playing_state       | Paused                            |
| buffer_state        | Buffering                         |
| position.value      | 32000ms                           |
| current_queue_index | 1                                 |

---

### 22. TX: Report State (with duration)

```
Base64: BkUIGRCVt+mqszMYASoBAjo1CZVbWjWbAQAAEBcaKAgXugEjCiEIAxABGg0JlFtaNZsBAAAQgPoBIL3EESoECAIQATABOAI=
```

| Field            | Value                     |
| ---------------- | ------------------------- |
| msg_id           | 25                        |
| msg_date         | 1766126672789 (+409ms)    |
| Message type     | 23 (RndrSrvrStateUpdated) |
| playing_state    | Paused                    |
| buffer_state     | Buffering                 |
| position.value   | 32000ms                   |
| duration         | **287293ms**              |

---

### 23. TX: Report State (duplicate)

```
Base64: BkUIGhDrt+mqszMYASoBAjo1CetbWjWbAQAAEBgaKAgXugEjCiEIAxABGg0J6ltaNZsBAAAQgPoBIL3EESoECAIQATABOAI=
```

| Field        | Value                  |
| ------------ | ---------------------- |
| msg_id       | 26                     |
| msg_date     | 1766126672875 (+86ms)  |
| Message type | 23                     |

---

### 24-25. RX: Server Confirms Our State Updates

```
Base64: BkMI7BUQzLfpqrMzGAEiAQI6MgnMW1o1mwEAABCmFRokCFKSBR8ICRABGhkIAxABGg0JlFtaNZsBAAAQgPoBIL3EESgB
Base64: BkMI7RUQmrjpqrMzGAEiAQI6MgkaXFo1mwEAABCnFRokCFKSBR8ICRABGhkIAxABGg0J6ltaNZsBAAAQgPoBIL3EESgB
```

| Field        | Value |
| ------------ | ----- |
| msg_id       | 2796, 2797 |
| Message type | 82 (SrvrCtrlRendererStateUpdated) |
| renderer_id  | 9 |
| buffer_state | Buffering |
| duration     | 287293ms |

---

### 26-27. TX: Report Buffered State

```
Base64: BkUIGxDEuOmqszMYASoBAjo1CURcWjWbAQAAEBkaKAgXugEjCiEIAxACGg0JQlxaNZsBAAAQgPoBIL3EESoECAIQATABOAI=
Base64: BkUIHBDEuOmqszMYASoBAjo1CURcWjWbAQAAEBoaKAgXugEjCiEIAxACGg0JQlxaNZsBAAAQgPoBIL3EESoECAIQATABOAI=
```

| Field            | Value                     |
| ---------------- | ------------------------- |
| msg_id           | 27, 28                    |
| msg_date         | 1766126672964             |
| Message type     | 23 (RndrSrvrStateUpdated) |
| playing_state    | Paused                    |
| buffer_state     | **Ok**                    |
| position.value   | 32000ms                   |
| duration         | 287293ms                  |

**Analysis:** We report buffer is now ready.

---

### 28. TX: Report File Audio Quality (Sample Rate)

```
Base64: BiwIHRDFuOmqszMYASoBAjocCUVcWjWbAQAAEBsaDwga0gEKCMTYAhAQGAIgAg==
```

| Field        | Value                                 |
| ------------ | ------------------------------------- |
| msg_id       | 29                                    |
| msg_date     | 1766126672965 (+1ms)                  |
| Message type | 26 (RndrSrvrFileAudioQualityChanged)  |
| value        | **44100** (sample rate in Hz!)        |

**Analysis:** We report the file's sample rate. This is NOT a quality enum - it's the actual sample rate!

---

### 29-30. RX: Server Confirms Buffered State

```
Base64: BkMI7hUQ9rjpqrMzGAEiAQI6Mgl2XFo1mwEAABCoFRokCFKSBR8ICRABGhkIAxACGg0JQlxaNZsBAAAQgPoBIL3EESgB
Base64: BkMI7xUQ+LjpqrMzGAEiAQI6Mgl4XFo1mwEAABCpFRokCFKSBR8ICRABGhkIAxACGg0JQlxaNZsBAAAQgPoBIL3EESgB
```

| Field        | Value |
| ------------ | ----- |
| msg_id       | 2798, 2799 |
| Message type | 82 |
| renderer_id  | 9 |
| buffer_state | Ok |

---

### 31. RX: Server Broadcasts File Audio Quality

```
Base64: BjAI8BUQ+rjpqrMzGAEiAQI6Hwl6XFo1mwEAABCqFRoRCGSiBgwICRDE2AIYECACKAI=
```

| Field              | Value |
| ------------------ | ----- |
| msg_id             | 2800 |
| msg_date           | 1766126673018 |
| Message type       | 100 (SrvrCtrlFileAudioQualityChanged) |
| file_audio_quality | 7 |

---

## Summary

### Renderer Activation Sequence

```
Server notifies pending activation (SrvrRndrSetActive: active=None)
    │
    ▼
Server sends current state info for existing renderer (7):
  - SrvrCtrlActiveRendererChanged (renderer_id=7)
  - SrvrCtrlVolumeMuted (not muted)
  - SrvrCtrlVolumeChanged (volume=100)
  - SrvrCtrlMaxAudioQualityChanged (quality=7)
  - SrvrCtrlRendererStateUpdated (state, position, duration)
  - SrvrCtrlFileAudioQualityChanged (quality=7)
    │
    ▼
User selects us as active
    │
    ▼
Server activates us (SrvrRndrSetActive: active=true)
    │
    ▼
We report our capabilities:
  - RndrSrvrVolumeMuted (not muted)
  - RndrSrvrVolumeChanged (volume=100)
  - RndrSrvrMaxAudioQualityChanged (quality=3)
  - RndrSrvrStateUpdated (Paused, Buffering, pos=32000ms)
    │
    ▼
Server broadcasts to all clients:
  - SrvrCtrlActiveRendererChanged (renderer_id=9)
  - SrvrCtrlVolumeMuted, VolumeChanged, MaxAudioQualityChanged
  - SrvrCtrlRendererStateUpdated (echoes our state)
    │
    ▼
We finish buffering and report:
  - RndrSrvrStateUpdated (buffer_state=Ok)
  - RndrSrvrFileAudioQualityChanged (value=44100 = sample rate!)
    │
    ▼
Server confirms and broadcasts final state
```

### Key Findings

1. **Activation is two-phase:**
   - First `active=None` (pending notification)
   - Then `active=true` (actual activation)

2. **State handoff:** We receive the previous renderer's position (32000ms) and continue from there.

3. **Capability negotiation:**
   - Server told us max_audio_quality=7 (from renderer 7)
   - We reported max_audio_quality=3 (our capability)
   - Server accepts our lower capability

4. **Sample rate reporting:** Type 26 (`RndrSrvrFileAudioQualityChanged`) value is the **actual sample rate in Hz** (44100), not a quality enum.

5. **Volume handling:** We echo back the volume (100) that the server told us about.

6. **Buffer state transitions:** Buffering -> Ok as we prepare to play.

### Message Types Reference

| Type | Name                              | Direction | Purpose                              |
| ---- | --------------------------------- | --------- | ------------------------------------ |
| 23   | RndrSrvrStateUpdated              | TX        | Renderer reports playback state      |
| 25   | RndrSrvrVolumeChanged             | TX        | Renderer reports volume              |
| 26   | RndrSrvrFileAudioQualityChanged   | TX        | Renderer reports sample rate         |
| 28   | RndrSrvrMaxAudioQualityChanged    | TX        | Renderer reports max quality cap     |
| 29   | RndrSrvrVolumeMuted               | TX        | Renderer reports mute state          |
| 43   | SrvrRndrSetActive                 | RX        | Server activates/deactivates us      |
| 82   | SrvrCtrlRendererStateUpdated      | RX        | Server broadcasts state (ignore)     |
| 86   | SrvrCtrlActiveRendererChanged     | RX        | Server broadcasts active renderer    |
| 87   | SrvrCtrlVolumeChanged             | RX        | Server broadcasts volume             |
| 98   | SrvrCtrlVolumeMuted               | RX        | Server broadcasts mute state         |
| 99   | SrvrCtrlMaxAudioQualityChanged    | RX        | Server broadcasts max quality        |
| 100  | SrvrCtrlFileAudioQualityChanged   | RX        | Server broadcasts file quality       |
