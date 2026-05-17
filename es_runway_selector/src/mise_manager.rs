use std::path::PathBuf;

use serde::Deserialize;
use tracing::{debug, info};

use crate::{config::es_runway_selector_project_dir, plugin_client::PluginError};

/// Map a source file extension to the arguments needed by `mise exec`.
///
/// Returns `(tool@version, command_and_initial_args)` or `None` for native
/// binaries that are executed directly without mise.
pub fn mise_invocation_for_extension(ext: &str) -> Option<(&'static str, &'static [&'static str])> {
    match ext.to_ascii_lowercase().as_str() {
        // uv run handles PEP 723 inline script dependencies automatically.
        "py" => Some(("uv@latest", &["uv", "run"] as &[&str])),
        "js" | "mjs" | "cjs" => Some(("node@latest", &["node"])),
        // Deno runs TypeScript natively and needs net + read permissions.
        "ts" | "mts" => Some((
            "deno@latest",
            &["deno", "run", "--allow-net", "--allow-read"],
        )),
        "rb" => Some(("ruby@latest", &["ruby"])),
        _ => None,
    }
}

/// Return the path to a `mise` binary, downloading and caching it if needed.
pub async fn find_or_download_mise() -> Result<PathBuf, PluginError> {
    // 1. Check PATH.
    if tokio::process::Command::new("mise")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        debug!("Found mise in PATH");
        return Ok(PathBuf::from("mise"));
    }

    // 2. Check cached binary.
    let cached = cached_mise_path();
    if cached.exists() {
        debug!("Using cached mise at {:?}", cached);
        return Ok(cached);
    }

    // 3. Download and cache.
    info!("mise not found — fetching latest release info");
    let url = latest_download_url().await?;
    info!("Downloading mise from {}", url);

    let bytes = reqwest::Client::builder()
        .user_agent("es-runway-selector")
        .build()
        .map_err(PluginError::Http)?
        .get(&url)
        .send()
        .await
        .map_err(PluginError::Http)?
        .bytes()
        .await
        .map_err(PluginError::Http)?;

    std::fs::create_dir_all(cached.parent().expect("mise path has no parent"))
        .map_err(PluginError::Io)?;

    extract_mise(&bytes, &cached)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cached, std::fs::Permissions::from_mode(0o755))
            .map_err(PluginError::Io)?;
    }

    info!("mise downloaded to {:?}", cached);
    Ok(cached)
}

fn cached_mise_path() -> PathBuf {
    let dir = es_runway_selector_project_dir().data_dir().join("mise");
    if cfg!(target_os = "windows") {
        dir.join("mise.exe")
    } else {
        dir.join("mise")
    }
}

async fn latest_download_url() -> Result<String, PluginError> {
    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
    }

    let release: Release = reqwest::Client::builder()
        .user_agent("es-runway-selector")
        .build()
        .map_err(PluginError::Http)?
        .get("https://api.github.com/repos/jdx/mise/releases/latest")
        .send()
        .await
        .map_err(PluginError::Http)?
        .json()
        .await
        .map_err(PluginError::Http)?;

    let tag = &release.tag_name; // e.g. "v2025.5.10"

    let target = if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "windows-x64"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "macos-arm64"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "macos-x64"
    } else {
        "linux-x64"
    };

    let ext = if cfg!(target_os = "windows") {
        "zip"
    } else {
        "tar.gz"
    };

    Ok(format!(
        "https://github.com/jdx/mise/releases/download/{tag}/mise-{tag}-{target}.{ext}"
    ))
}

#[cfg(target_os = "windows")]
fn extract_mise(bytes: &[u8], target: &std::path::Path) -> Result<(), PluginError> {
    let cursor = std::io::Cursor::new(bytes);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| PluginError::Archive(e.to_string()))?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| PluginError::Archive(e.to_string()))?;
        let name = entry.name().to_owned();
        if name == "mise.exe" || name.ends_with("/mise.exe") {
            let mut out = std::fs::File::create(target).map_err(PluginError::Io)?;
            std::io::copy(&mut entry, &mut out).map_err(PluginError::Io)?;
            return Ok(());
        }
    }
    Err(PluginError::Archive(
        "mise.exe not found in zip archive".into(),
    ))
}

#[cfg(not(target_os = "windows"))]
fn extract_mise(bytes: &[u8], target: &std::path::Path) -> Result<(), PluginError> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    let gz = GzDecoder::new(bytes);
    let mut archive = Archive::new(gz);

    for entry in archive.entries().map_err(PluginError::Io)? {
        let mut entry = entry.map_err(PluginError::Io)?;
        let is_mise = entry
            .path()
            .map_err(PluginError::Io)?
            .file_name()
            .and_then(|n| n.to_str())
            == Some("mise");
        if is_mise {
            entry.unpack(target).map_err(PluginError::Io)?;
            return Ok(());
        }
    }
    Err(PluginError::Archive(
        "mise not found in tar.gz archive".into(),
    ))
}
