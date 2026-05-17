use std::{io, path::Path};

use serde::Deserialize;
use tracing::{debug, info};

use crate::config::{PluginUpdateConfig, UpdateBackend};

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("Archive error: {0}")]
    Archive(String),
    #[error("No release assets matching target '{0}' found")]
    NoMatchingAsset(String),
}

struct AssetInfo {
    version: String,
    download_url: String,
}

/// Returns `true` if the plugin was downloaded/updated.
pub async fn check_and_update_plugin(
    plugin_binary: &Path,
    config: &PluginUpdateConfig,
) -> Result<bool, UpdateError> {
    let current = read_version(plugin_binary);
    let target = platform_target();
    let asset = fetch_latest(config, target).await?;

    if current.as_deref() == Some(asset.version.as_str()) {
        debug!(
            "Plugin {} is up to date ({})",
            config.bin_name, asset.version
        );
        return Ok(false);
    }

    info!(
        "Updating plugin {} from {:?} to {}",
        config.bin_name, current, asset.version
    );

    let bytes = http_client()?
        .get(&asset.download_url)
        .send()
        .await?
        .bytes()
        .await?;

    if let Some(parent) = plugin_binary.parent() {
        std::fs::create_dir_all(parent)?;
    }

    install_binary(&bytes, plugin_binary, &config.bin_name, &asset.download_url)?;
    write_version(plugin_binary, &asset.version)?;

    info!("Plugin {} updated to {}", config.bin_name, asset.version);
    Ok(true)
}

fn platform_target() -> &'static str {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "windows-x64"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "macos-arm64"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "macos-x64"
    } else if cfg!(target_env = "musl") {
        "linux-musl-x64"
    } else {
        "linux-x64"
    }
}

fn http_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent("es-runway-selector")
        .build()
}

async fn fetch_latest(config: &PluginUpdateConfig, target: &str) -> Result<AssetInfo, UpdateError> {
    match &config.backend {
        UpdateBackend::Github => fetch_github(config, target).await,
        UpdateBackend::Gitlab => {
            let host = config.gitlab_host.as_deref().unwrap_or("gitlab.com");
            fetch_gitlab(config, target, host).await
        }
    }
}

async fn fetch_github(config: &PluginUpdateConfig, target: &str) -> Result<AssetInfo, UpdateError> {
    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
        assets: Vec<Asset>,
    }
    #[derive(Deserialize)]
    struct Asset {
        name: String,
        browser_download_url: String,
    }

    let release: Release = http_client()?
        .get(format!(
            "https://api.github.com/repos/{}/releases/latest",
            config.repo
        ))
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await?
        .json()
        .await?;

    let asset = release
        .assets
        .iter()
        .find(|a| a.name.contains(&config.bin_name) && a.name.contains(target))
        .ok_or_else(|| UpdateError::NoMatchingAsset(target.to_string()))?;

    Ok(AssetInfo {
        version: release.tag_name,
        download_url: asset.browser_download_url.clone(),
    })
}

async fn fetch_gitlab(
    config: &PluginUpdateConfig,
    target: &str,
    host: &str,
) -> Result<AssetInfo, UpdateError> {
    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
        assets: Assets,
    }
    #[derive(Deserialize)]
    struct Assets {
        links: Vec<Link>,
    }
    #[derive(Deserialize)]
    struct Link {
        name: String,
        url: String,
    }

    // GitLab requires the project path to be URL-encoded (slashes → %2F)
    let encoded_repo = config.repo.replace('/', "%2F");
    let releases: Vec<Release> = http_client()?
        .get(format!(
            "https://{host}/api/v4/projects/{encoded_repo}/releases"
        ))
        .send()
        .await?
        .json()
        .await?;

    let latest = releases
        .into_iter()
        .next()
        .ok_or_else(|| UpdateError::NoMatchingAsset(target.to_string()))?;

    let link = latest
        .assets
        .links
        .iter()
        .find(|l| l.name.contains(&config.bin_name) && l.name.contains(target))
        .ok_or_else(|| UpdateError::NoMatchingAsset(target.to_string()))?;

    Ok(AssetInfo {
        version: latest.tag_name,
        download_url: link.url.clone(),
    })
}

fn install_binary(
    bytes: &[u8],
    dest: &Path,
    bin_name: &str,
    source_url: &str,
) -> Result<(), UpdateError> {
    let lower = source_url.to_ascii_lowercase();
    if lower.ends_with(".zip") {
        extract_zip(bytes, dest, bin_name)
    } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        extract_targz(bytes, dest, bin_name)
    } else {
        write_raw(bytes, dest)
    }
}

#[cfg(windows)]
fn extract_zip(bytes: &[u8], dest: &Path, bin_name: &str) -> Result<(), UpdateError> {
    let cursor = std::io::Cursor::new(bytes);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| UpdateError::Archive(e.to_string()))?;

    let target_name = format!("{bin_name}.exe");
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| UpdateError::Archive(e.to_string()))?;
        let name = entry.name().to_owned();
        if name == target_name || name.ends_with(&format!("/{target_name}")) {
            let mut file = std::fs::File::create(dest)?;
            std::io::copy(&mut entry, &mut file)?;
            return Ok(());
        }
    }
    Err(UpdateError::Archive(format!(
        "{target_name} not found in zip archive"
    )))
}

#[cfg(not(windows))]
fn extract_zip(_bytes: &[u8], _dest: &Path, bin_name: &str) -> Result<(), UpdateError> {
    Err(UpdateError::Archive(format!(
        "zip extraction not supported on this platform; expected .tar.gz for {bin_name}"
    )))
}

#[cfg(not(windows))]
fn extract_targz(bytes: &[u8], dest: &Path, bin_name: &str) -> Result<(), UpdateError> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    let gz = GzDecoder::new(bytes);
    let mut archive = Archive::new(gz);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        let matches = path.file_name().and_then(|n| n.to_str()) == Some(bin_name);
        if matches {
            drop(path);
            entry.unpack(dest)?;
            #[cfg(unix)]
            set_executable(dest)?;
            return Ok(());
        }
    }
    Err(UpdateError::Archive(format!(
        "{bin_name} not found in tar.gz archive"
    )))
}

#[cfg(windows)]
fn extract_targz(_bytes: &[u8], _dest: &Path, bin_name: &str) -> Result<(), UpdateError> {
    Err(UpdateError::Archive(format!(
        "tar.gz extraction not supported on Windows; expected .zip for {bin_name}"
    )))
}

fn write_raw(bytes: &[u8], dest: &Path) -> Result<(), UpdateError> {
    std::fs::write(dest, bytes)?;
    #[cfg(unix)]
    set_executable(dest)?;
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), UpdateError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

fn version_path(plugin_binary: &Path) -> std::path::PathBuf {
    let stem = plugin_binary
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    plugin_binary.with_file_name(format!("{stem}.version"))
}

fn read_version(plugin_binary: &Path) -> Option<String> {
    std::fs::read_to_string(version_path(plugin_binary))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn write_version(plugin_binary: &Path, version: &str) -> io::Result<()> {
    std::fs::write(version_path(plugin_binary), version)
}
