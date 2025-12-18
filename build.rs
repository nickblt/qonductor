use std::io::Result;

fn main() -> Result<()> {
    prost_build::compile_protos(
        &[
            "proto/qconnect_common.proto",
            "proto/qconnect_envelope.proto",
            "proto/qconnect_payload.proto",
            "proto/qconnect_queue.proto",
            "proto/ws.proto",
        ],
        &["proto/"],
    )?;
    Ok(())
}
