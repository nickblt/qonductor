use std::io::Result;

fn main() -> Result<()> {
    #[cfg(feature = "compile-protos")]
    {
        let out_dir = std::path::PathBuf::from("src/proto");
        std::fs::create_dir_all(&out_dir).unwrap();

        prost_build::Config::new()
            .out_dir(&out_dir)
            .compile_protos(
                &[
                    "proto/qconnect_common.proto",
                    "proto/qconnect_envelope.proto",
                    "proto/qconnect_payload.proto",
                    "proto/qconnect_queue.proto",
                    "proto/ws.proto",
                ],
                &["proto/"],
            )?;
    }
    Ok(())
}
