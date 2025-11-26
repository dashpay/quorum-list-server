fn main() -> Result<(), Box<dyn std::error::Error>> {
    // proto3 optional fields require this flag on older protoc builds
    tonic_build::configure()
        .protoc_arg("--experimental_allow_proto3_optional")
        .compile_protos(&["proto/platform.proto"], &["proto"])?;
    Ok(())
}
