fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The iOS Simulator backend is macOS-only; skip protobuf codegen (and the
    // protoc requirement) on other targets so the workspace builds cross-platform.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return Ok(());
    }
    tonic_build::configure()
        .build_server(false)
        .compile_protos(&["proto/idb.proto"], &["proto"])?;
    println!("cargo:rerun-if-changed=proto/idb.proto");
    Ok(())
}
