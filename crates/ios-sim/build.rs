fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(false)
        .compile_protos(&["proto/idb.proto"], &["proto"])?;
    println!("cargo:rerun-if-changed=proto/idb.proto");
    Ok(())
}
