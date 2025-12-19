//! Decode QConnect protobuf messages from hex or base64.
//!
//! Usage: cargo run --bin decode <hex_or_base64_string>

use base64::Engine;
use prost::Message;
use prost::encoding::decode_varint;
use qonductor::format_qconnect_message;
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
    println!("    {}", format_qconnect_message(msg));
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
