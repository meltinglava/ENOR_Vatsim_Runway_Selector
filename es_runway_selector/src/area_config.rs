//! Download and install area-specific configuration packages.
//!
//! Supports two source types:
//! - `github`: latest release assets from a per-FIR GitHub repository.
//! - `manifest`: a central JSON registry that maps FIR keys to download URLs.
//!
//! Downloaded packages are extracted to
//! `<config_dir>/areas/<area_name>/`.

use std::{
    fs,
    io::{self, Cursor},
    path::Path,
};

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::error::{ApplicationError, ApplicationResult};

// ─── Config types (areas.toml) ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum AreaSource {
    /// Download from a GitHub repository's latest release.
    Github {
        /// `owner/repo` e.g. `"meltinglava/es-enor-area-plugin"`.
        repo: String,
    },
    /// Fetch a central manifest JSON and look up the area by key.
    Manifest {
        /// URL that returns a `ManifestFile` JSON document.
        url: String,
        /// Key inside the manifest's `areas` map.
        key: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AreaEntry {
    pub name: String,
    #[serde(flatten)]
    pub source: AreaSource,
}

#[derive(Debug, Deserialize, Default)]
struct AreasFile {
    #[serde(default)]
    areas: Vec<AreaEntry>,
}

pub(crate) fn load_area_entries(config_dir: &Path) -> Vec<AreaEntry> {
    let path = config_dir.join("areas.toml");
    if !path.exists() {
        return Vec::new();
    }
    let raw = fs::read_to_string(&path).unwrap_or_default();
    toml::from_str::<AreasFile>(&raw)
        .map(|f| f.areas)
        .unwrap_or_default()
}

// ─── Manifest format ──────────────────────────────────────────────────────────

/// Central manifest format served at `AreaSource::Manifest::url`.
#[derive(Debug, Deserialize)]
pub(crate) struct ManifestFile {
    pub areas: std::collections::HashMap<String, ManifestArea>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ManifestArea {
    pub name: String,
    /// Direct download URL for the `.tar.gz` or `.zip` package.
    pub download_url: String,
    /// Optional human-readable description shown in `--list-areas`.
    pub description: Option<String>,
}

// ─── GitHub release API ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

async fn latest_github_release(client: &Client, repo: &str) -> ApplicationResult<GithubRelease> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let resp = client
        .get(&url)
        .header("User-Agent", "es_runway_selector")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(ApplicationError::AreaConfigError(format!(
            "GitHub API returned {} for {url}",
            resp.status()
        )));
    }
    Ok(resp.json().await?)
}

// ─── Download & extract ───────────────────────────────────────────────────────

async fn download_bytes(client: &Client, url: &str) -> ApplicationResult<Vec<u8>> {
    debug!(url, "Downloading area config package");
    let resp = client
        .get(url)
        .header("User-Agent", "es_runway_selector")
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(ApplicationError::AreaConfigError(format!(
            "Download failed with status {} for {url}",
            resp.status()
        )));
    }
    Ok(resp.bytes().await?.to_vec())
}

fn extract_archive(bytes: &[u8], dest: &Path) -> io::Result<()> {
    fs::create_dir_all(dest)?;

    // Try tar.gz first; fall back to zip.
    if bytes.starts_with(&[0x1f, 0x8b]) {
        // gzip magic bytes
        let gz = flate2::read::GzDecoder::new(Cursor::new(bytes));
        let mut archive = tar::Archive::new(gz);
        archive.unpack(dest)?;
    } else if bytes.starts_with(&[0x50, 0x4b]) {
        // PK zip magic bytes
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        archive.extract(dest).map_err(io::Error::other)?;
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Unrecognised archive format (expected .tar.gz or .zip)",
        ));
    }
    Ok(())
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Download and install an area config package into `config_dir/areas/<name>/`.
pub(crate) async fn download_area(entry: &AreaEntry, config_dir: &Path) -> ApplicationResult<()> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()?;

    let dest = config_dir.join("areas").join(&entry.name);

    let download_url = match &entry.source {
        AreaSource::Github { repo } => {
            let release = latest_github_release(&client, repo).await?;
            info!(area = %entry.name, version = %release.tag_name, "Found GitHub release");
            // Pick the first .tar.gz or .zip asset.
            release
                .assets
                .iter()
                .find(|a| a.name.ends_with(".tar.gz") || a.name.ends_with(".zip"))
                .map(|a| a.browser_download_url.clone())
                .ok_or_else(|| {
                    ApplicationError::AreaConfigError(format!(
                        "No .tar.gz or .zip asset found in latest release of {repo}"
                    ))
                })?
        }
        AreaSource::Manifest { url, key } => {
            let resp = client
                .get(url)
                .header("User-Agent", "es_runway_selector")
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(ApplicationError::AreaConfigError(format!(
                    "Manifest fetch failed with status {}",
                    resp.status()
                )));
            }
            let manifest: ManifestFile = resp.json().await?;
            manifest
                .areas
                .get(key)
                .map(|a| a.download_url.clone())
                .ok_or_else(|| {
                    ApplicationError::AreaConfigError(format!(
                        "Key '{key}' not found in manifest at {url}"
                    ))
                })?
        }
    };

    let bytes = download_bytes(&client, &download_url).await?;
    extract_archive(&bytes, &dest).map_err(ApplicationError::from)?;

    info!(area = %entry.name, dest = %dest.display(), "Area config installed");
    Ok(())
}

/// Fetch and print all areas listed in a manifest (for `--list-areas`).
pub(crate) async fn list_manifest_areas(url: &str) -> ApplicationResult<()> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let resp = client
        .get(url)
        .header("User-Agent", "es_runway_selector")
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(ApplicationError::AreaConfigError(format!(
            "Manifest fetch failed with status {}",
            resp.status()
        )));
    }
    let manifest: ManifestFile = resp.json().await?;
    println!("Available areas:");
    let mut keys: Vec<&String> = manifest.areas.keys().collect();
    keys.sort();
    for key in keys {
        let area = &manifest.areas[key];
        if let Some(desc) = &area.description {
            println!("  {key} – {} ({})", area.name, desc);
        } else {
            println!("  {key} – {}", area.name);
        }
    }
    Ok(())
}
