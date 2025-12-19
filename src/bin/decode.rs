//! Decode QConnect protobuf messages from hex or base64.
//!
//! Usage: cargo run --bin decode <hex_or_base64_string>

use base64::Engine;
use prost::Message;
use prost::encoding::decode_varint;
use qonductor::proto::qconnect::{Payload, QConnectBatch, QConnectMessage};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <hex_or_base64_string>", args[0]);
        std::process::exit(1);
    }

    let input = &args[1];
    let data = decode_input(input);

    println!("Raw bytes ({} bytes): {:02x?}", data.len(), data);
    println!();

    decode_envelope(&data);
}

fn decode_input(input: &str) -> Vec<u8> {
    // Try hex first (all chars are 0-9, a-f, A-F)
    if input.chars().all(|c| c.is_ascii_hexdigit())
        && input.len().is_multiple_of(2)
        && let Ok(d) = (0..input.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&input[i..i + 2], 16))
            .collect()
    {
        return d;
    }

    // Try base64
    if let Ok(d) = base64::engine::general_purpose::STANDARD.decode(input) {
        return d;
    }

    // Try base64 URL-safe variant
    if let Ok(d) = base64::engine::general_purpose::URL_SAFE.decode(input) {
        return d;
    }

    eprintln!("Failed to decode as hex or base64");
    std::process::exit(1);
}

fn decode_envelope(data: &[u8]) {
    if data.is_empty() {
        println!("Empty data");
        return;
    }

    let msg_type = data[0] as i32;
    println!(
        "Envelope type: {msg_type} ({})",
        envelope_type_name(msg_type)
    );

    // Read varint length
    let mut cursor = &data[1..];
    let payload_len = match decode_varint(&mut cursor) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Failed to decode varint: {e}");
            return;
        }
    };
    let varint_bytes = data.len() - 1 - cursor.len();
    let payload_start = 1 + varint_bytes;

    println!("Payload length: {payload_len} bytes (varint used {varint_bytes} bytes)");
    println!();

    if data.len() < payload_start + payload_len as usize {
        eprintln!(
            "Incomplete: expected {} bytes, got {}",
            payload_start + payload_len as usize,
            data.len()
        );
        return;
    }

    let payload = &data[payload_start..payload_start + payload_len as usize];

    match msg_type {
        6 => decode_payload(payload),
        _ => {
            println!("Payload hex: {:02x?}", payload);
        }
    }
}

fn decode_payload(data: &[u8]) {
    match Payload::decode(data) {
        Ok(p) => {
            println!("Payload:");
            println!("  msg_id: {:?}", p.msg_id);
            println!("  msg_date: {:?}", p.msg_date);
            println!("  proto: {:?}", p.proto);

            if let Some(inner) = &p.payload {
                println!();
                decode_batch(inner);
            }
        }
        Err(e) => {
            eprintln!("Failed to decode Payload: {e}");
            println!("Raw: {:02x?}", data);
        }
    }
}

fn decode_batch(data: &[u8]) {
    match QConnectBatch::decode(data) {
        Ok(batch) => {
            println!("QConnectBatch:");
            println!("  messages_time: {:?}", batch.messages_time);
            println!("  messages_id: {:?}", batch.messages_id);
            println!("  messages: {} message(s)", batch.messages.len());

            for (i, msg) in batch.messages.iter().enumerate() {
                println!();
                println!("  Message[{i}]:");
                decode_message(msg);
            }
        }
        Err(e) => {
            eprintln!("Failed to decode QConnectBatch: {e}");
            println!("Raw: {:02x?}", data);
        }
    }
}

fn decode_message(msg: &QConnectMessage) {
    let msg_type = msg.message_type.unwrap_or(0);
    println!("    type: {msg_type} ({})", message_type_name(msg_type));

    // Renderer -> Server
    if msg.rndr_srvr_join_session.is_some() {
        println!(
            "    rndr_srvr_join_session: {:?}",
            msg.rndr_srvr_join_session
        );
    }
    if msg.rndr_srvr_device_info_updated.is_some() {
        println!(
            "    rndr_srvr_device_info_updated: {:?}",
            msg.rndr_srvr_device_info_updated
        );
    }
    if msg.rndr_srvr_state_updated.is_some() {
        println!(
            "    rndr_srvr_state_updated: {:?}",
            msg.rndr_srvr_state_updated
        );
    }
    if msg.rndr_srvr_renderer_action.is_some() {
        println!(
            "    rndr_srvr_renderer_action: {:?}",
            msg.rndr_srvr_renderer_action
        );
    }
    if msg.rndr_srvr_volume_changed.is_some() {
        println!(
            "    rndr_srvr_volume_changed: {:?}",
            msg.rndr_srvr_volume_changed
        );
    }
    if msg.rndr_srvr_file_audio_quality_changed.is_some() {
        println!(
            "    rndr_srvr_file_audio_quality_changed: {:?}",
            msg.rndr_srvr_file_audio_quality_changed
        );
    }
    if msg.rndr_srvr_device_audio_quality_changed.is_some() {
        println!(
            "    rndr_srvr_device_audio_quality_changed: {:?}",
            msg.rndr_srvr_device_audio_quality_changed
        );
    }
    if msg.rndr_srvr_max_audio_quality_changed.is_some() {
        println!(
            "    rndr_srvr_max_audio_quality_changed: {:?}",
            msg.rndr_srvr_max_audio_quality_changed
        );
    }
    if msg.rndr_srvr_volume_muted.is_some() {
        println!(
            "    rndr_srvr_volume_muted: {:?}",
            msg.rndr_srvr_volume_muted
        );
    }

    // Server -> Renderer
    if msg.srvr_rndr_set_state.is_some() {
        println!("    srvr_rndr_set_state: {:?}", msg.srvr_rndr_set_state);
    }
    if msg.srvr_rndr_set_volume.is_some() {
        println!("    srvr_rndr_set_volume: {:?}", msg.srvr_rndr_set_volume);
    }
    if msg.srvr_rndr_set_active.is_some() {
        println!("    srvr_rndr_set_active: {:?}", msg.srvr_rndr_set_active);
    }
    if msg.srvr_rndr_set_max_audio_quality.is_some() {
        println!(
            "    srvr_rndr_set_max_audio_quality: {:?}",
            msg.srvr_rndr_set_max_audio_quality
        );
    }
    if msg.srvr_rndr_set_loop_mode.is_some() {
        println!(
            "    srvr_rndr_set_loop_mode: {:?}",
            msg.srvr_rndr_set_loop_mode
        );
    }
    if msg.srvr_rndr_set_shuffle_mode.is_some() {
        println!(
            "    srvr_rndr_set_shuffle_mode: {:?}",
            msg.srvr_rndr_set_shuffle_mode
        );
    }
    if msg.srvr_rndr_set_autoplay_mode.is_some() {
        println!(
            "    srvr_rndr_set_autoplay_mode: {:?}",
            msg.srvr_rndr_set_autoplay_mode
        );
    }

    // Server -> Controllers
    if msg.srvr_ctrl_session_state.is_some() {
        println!(
            "    srvr_ctrl_session_state: {:?}",
            msg.srvr_ctrl_session_state
        );
    }
    if msg.srvr_ctrl_renderer_state_updated.is_some() {
        println!(
            "    srvr_ctrl_renderer_state_updated: {:?}",
            msg.srvr_ctrl_renderer_state_updated
        );
    }
    if msg.srvr_ctrl_add_renderer.is_some() {
        println!(
            "    srvr_ctrl_add_renderer: {:?}",
            msg.srvr_ctrl_add_renderer
        );
    }
    if msg.srvr_ctrl_update_renderer.is_some() {
        println!(
            "    srvr_ctrl_update_renderer: {:?}",
            msg.srvr_ctrl_update_renderer
        );
    }
    if msg.srvr_ctrl_remove_renderer.is_some() {
        println!(
            "    srvr_ctrl_remove_renderer: {:?}",
            msg.srvr_ctrl_remove_renderer
        );
    }
    if msg.srvr_ctrl_active_renderer_changed.is_some() {
        println!(
            "    srvr_ctrl_active_renderer_changed: {:?}",
            msg.srvr_ctrl_active_renderer_changed
        );
    }
    if msg.srvr_ctrl_volume_changed.is_some() {
        println!(
            "    srvr_ctrl_volume_changed: {:?}",
            msg.srvr_ctrl_volume_changed
        );
    }
    if msg.srvr_ctrl_queue_error_message.is_some() {
        println!(
            "    srvr_ctrl_queue_error_message: {:?}",
            msg.srvr_ctrl_queue_error_message
        );
    }
    if msg.srvr_ctrl_queue_cleared.is_some() {
        println!(
            "    srvr_ctrl_queue_cleared: {:?}",
            msg.srvr_ctrl_queue_cleared
        );
    }
    if msg.srvr_ctrl_queue_state.is_some() {
        let qs = msg.srvr_ctrl_queue_state.as_ref().unwrap();
        println!(
            "    srvr_ctrl_queue_state: {} tracks, version={:?}",
            qs.tracks.len(),
            qs.queue_version
        );
        //println!("    srvr_ctrl_queue_state: {:?}", msg.srvr_ctrl_queue_state);
    }
    if msg.srvr_ctrl_queue_tracks_loaded.is_some() {
        println!(
            "    srvr_ctrl_queue_tracks_loaded: {:?}",
            msg.srvr_ctrl_queue_tracks_loaded
        );
    }
    if msg.srvr_ctrl_queue_tracks_inserted.is_some() {
        println!(
            "    srvr_ctrl_queue_tracks_inserted: {:?}",
            msg.srvr_ctrl_queue_tracks_inserted
        );
    }
    if msg.srvr_ctrl_queue_tracks_added.is_some() {
        println!(
            "    srvr_ctrl_queue_tracks_added: {:?}",
            msg.srvr_ctrl_queue_tracks_added
        );
    }
    if msg.srvr_ctrl_queue_tracks_removed.is_some() {
        println!(
            "    srvr_ctrl_queue_tracks_removed: {:?}",
            msg.srvr_ctrl_queue_tracks_removed
        );
    }
    if msg.srvr_ctrl_queue_tracks_reordered.is_some() {
        println!(
            "    srvr_ctrl_queue_tracks_reordered: {:?}",
            msg.srvr_ctrl_queue_tracks_reordered
        );
    }
    if msg.srvr_ctrl_shuffle_mode_set.is_some() {
        println!(
            "    srvr_ctrl_shuffle_mode_set: {:?}",
            msg.srvr_ctrl_shuffle_mode_set
        );
    }
    if msg.srvr_ctrl_loop_mode_set.is_some() {
        println!(
            "    srvr_ctrl_loop_mode_set: {:?}",
            msg.srvr_ctrl_loop_mode_set
        );
    }
    if msg.srvr_ctrl_volume_muted.is_some() {
        println!(
            "    srvr_ctrl_volume_muted: {:?}",
            msg.srvr_ctrl_volume_muted
        );
    }
    if msg.srvr_ctrl_max_audio_quality_changed.is_some() {
        println!(
            "    srvr_ctrl_max_audio_quality_changed: {:?}",
            msg.srvr_ctrl_max_audio_quality_changed
        );
    }
    if msg.srvr_ctrl_file_audio_quality_changed.is_some() {
        println!(
            "    srvr_ctrl_file_audio_quality_changed: {:?}",
            msg.srvr_ctrl_file_audio_quality_changed
        );
    }
    if msg.srvr_ctrl_device_audio_quality_changed.is_some() {
        println!(
            "    srvr_ctrl_device_audio_quality_changed: {:?}",
            msg.srvr_ctrl_device_audio_quality_changed
        );
    }
    if msg.srvr_ctrl_autoplay_mode_set.is_some() {
        println!(
            "    srvr_ctrl_autoplay_mode_set: {:?}",
            msg.srvr_ctrl_autoplay_mode_set
        );
    }
    if msg.srvr_ctrl_autoplay_tracks_loaded.is_some() {
        println!(
            "    srvr_ctrl_autoplay_tracks_loaded: {:?}",
            msg.srvr_ctrl_autoplay_tracks_loaded
        );
    }
    if msg.srvr_ctrl_autoplay_tracks_removed.is_some() {
        println!(
            "    srvr_ctrl_autoplay_tracks_removed: {:?}",
            msg.srvr_ctrl_autoplay_tracks_removed
        );
    }
    if msg.srvr_ctrl_queue_version_changed.is_some() {
        println!(
            "    srvr_ctrl_queue_version_changed: {:?}",
            msg.srvr_ctrl_queue_version_changed
        );
    }
}

fn envelope_type_name(t: i32) -> &'static str {
    match t {
        1 => "AUTHENTICATE",
        2 => "SUBSCRIBE",
        6 => "PAYLOAD",
        9 => "ERROR",
        10 => "PONG",
        _ => "UNKNOWN",
    }
}

fn message_type_name(t: i32) -> &'static str {
    match t {
        0 => "MessageTypeUnknown",
        1 => "MessageTypeError",
        // Renderer -> Server
        21 => "RndrSrvrJoinSession",
        22 => "RndrSrvrDeviceInfoUpdated",
        23 => "RndrSrvrStateUpdated",
        24 => "RndrSrvrRendererAction",
        25 => "RndrSrvrVolumeChanged",
        26 => "RndrSrvrFileAudioQualityChanged",
        27 => "RndrSrvrDeviceAudioQualityChanged",
        28 => "RndrSrvrMaxAudioQualityChanged",
        29 => "RndrSrvrVolumeMuted",
        // Server -> Renderer
        41 => "SrvrRndrSetState",
        42 => "SrvrRndrSetVolume",
        43 => "SrvrRndrSetActive",
        44 => "SrvrRndrSetMaxAudioQuality",
        45 => "SrvrRndrSetLoopMode",
        46 => "SrvrRndrSetShuffleMode",
        47 => "SrvrRndrSetAutoplayMode",
        // Controller -> Server
        61 => "CtrlSrvrJoinSession",
        62 => "CtrlSrvrSetPlayerState",
        63 => "CtrlSrvrSetActiveRenderer",
        64 => "CtrlSrvrSetVolume",
        65 => "CtrlSrvrClearQueue",
        66 => "CtrlSrvrQueueLoadTracks",
        67 => "CtrlSrvrQueueInsertTracks",
        68 => "CtrlSrvrQueueAddTracks",
        69 => "CtrlSrvrQueueRemoveTracks",
        70 => "CtrlSrvrQueueReorderTracks",
        71 => "CtrlSrvrSetShuffleMode",
        72 => "CtrlSrvrSetLoopMode",
        73 => "CtrlSrvrMuteVolume",
        74 => "CtrlSrvrSetMaxAudioQuality",
        75 => "CtrlSrvrSetQueueState",
        76 => "CtrlSrvrAskForQueueState",
        77 => "CtrlSrvrAskForRendererState",
        78 => "CtrlSrvrSetAutoplayMode",
        79 => "CtrlSrvrAutoplayAddTracks",
        80 => "CtrlSrvrAutoplayRemoveTracks",
        // Server -> Controllers
        81 => "SrvrCtrlSessionState",
        82 => "SrvrCtrlRendererStateUpdated",
        83 => "SrvrCtrlAddRenderer",
        84 => "SrvrCtrlUpdateRenderer",
        85 => "SrvrCtrlRemoveRenderer",
        86 => "SrvrCtrlActiveRendererChanged",
        87 => "SrvrCtrlVolumeChanged",
        88 => "SrvrCtrlQueueErrorMessage",
        89 => "SrvrCtrlQueueCleared",
        90 => "SrvrCtrlQueueState",
        91 => "SrvrCtrlQueueTracksLoaded",
        92 => "SrvrCtrlQueueTracksInserted",
        93 => "SrvrCtrlQueueTracksAdded",
        94 => "SrvrCtrlQueueTracksRemoved",
        95 => "SrvrCtrlQueueTracksReordered",
        96 => "SrvrCtrlShuffleModeSet",
        97 => "SrvrCtrlLoopModeSet",
        98 => "SrvrCtrlVolumeMuted",
        99 => "SrvrCtrlMaxAudioQualityChanged",
        100 => "SrvrCtrlFileAudioQualityChanged",
        101 => "SrvrCtrlDeviceAudioQualityChanged",
        102 => "SrvrCtrlAutoplayModeSet",
        103 => "SrvrCtrlAutoplayTracksLoaded",
        104 => "SrvrCtrlAutoplayTracksRemoved",
        105 => "SrvrCtrlQueueVersionChanged",
        _ => "UNKNOWN",
    }
}
