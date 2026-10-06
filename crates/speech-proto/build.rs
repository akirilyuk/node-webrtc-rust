fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "regen-proto")]
    generate()?;
    Ok(())
}

#[cfg(feature = "regen-proto")]
fn generate() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let proto = manifest.join("proto/speech/v1/speech.proto");
    let includes = manifest.join("proto");
    let out = manifest.join("src/generated");
    std::fs::create_dir_all(&out)?;

    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    std::env::set_var("PROTOC", &protoc);

    println!("cargo:rerun-if-changed={}", proto.display());

    // tonic-build 0.12: `compile` (0.13 renamed this to `compile_protos`).
    tonic_build::configure()
        .build_client(true)
        .build_server(true)
        .server_mod_attribute("speech.v1", "#[cfg(feature = \"server\")]")
        .client_mod_attribute("speech.v1", "#[cfg(feature = \"client\")]")
        // `bytes` fields become `bytes::Bytes` (no Vec<u8> copy per audio message).
        .bytes(["."])
        .out_dir(&out)
        .compile(&[proto], &[includes])?;
    Ok(())
}
