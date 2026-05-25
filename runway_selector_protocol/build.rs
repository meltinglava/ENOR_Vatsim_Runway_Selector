fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Prefer the user's installed protoc (required on musl, since the
    // vendored binaries are glibc-linked). Fall back to the vendored binary
    // so contributors on glibc Linux / macOS / Windows don't need to install
    // anything.
    if std::env::var_os("PROTOC").is_none()
        && which::which("protoc").is_err()
        && let Ok(vendored) = protoc_bin_vendored::protoc_bin_path()
    {
        // SAFETY: build.rs runs single-threaded, so mutating the environment is safe here.
        unsafe {
            std::env::set_var("PROTOC", vendored);
        }
    }

    let proto = "proto/runway_selector.proto";
    println!("cargo:rerun-if-changed={proto}");
    println!("cargo:rerun-if-changed=build.rs");

    tonic_prost_build::configure().compile_protos(&[proto], &["proto"])?;

    Ok(())
}
