//! Area registry, install, and removal.
//!
//! An *area registry* is a JSON file (typically served over HTTPS) listing
//! the areas that can be installed. Each entry points at a `.tar.gz` archive
//! of the area package and carries a SHA-256 checksum:
//!
//! ```json
//! {
//!   "schema_version": 1,
//!   "areas": [
//!     {
//!       "name": "enor",
//!       "display_name": "Polaris / ENOR",
//!       "description": "Norway FIR runway selection logic",
//!       "version": "0.1.0",
//!       "download_url": "https://example.org/area_enor-0.1.0.tar.gz",
//!       "checksum_sha256": "<hex>",
//!       "maintainers": ["meltinglava"]
//!     }
//!   ]
//! }
//! ```
//!
//! [`install_area`] downloads the archive, verifies the hash, and untars it
//! into `<install_dir>/<name>/`. [`list_installed_areas`] enumerates the
//! `manifest.toml`s already on disk. [`remove_area`] deletes the directory.

use std::{
    collections::HashMap,
    fs,
    io::{self, Cursor},
    path::{Component, Path, PathBuf},
};

use flate2::read::GzDecoder;
use runway_selector_area_config::{
    AreaConfigError, AreaManifest, TopLevelConfig, load_area_manifest,
};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tar::{Archive, EntryType};
use tempfile::TempDir;
use thiserror::Error;
use tracing::{info, warn};

#[derive(Debug, Error)]
pub enum AreaRegistryError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("HTTP error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("Registry JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Unsupported registry schema_version {found}; this build supports {supported}")]
    UnsupportedSchema { found: u32, supported: u32 },
    #[error("No area named {name:?} found in any registered registry")]
    UnknownArea { name: String },
    #[error("Checksum mismatch for {name}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        name: String,
        expected: String,
        actual: String,
    },
    #[error("Failed to read installed area at {path}: {source}")]
    ManifestRead {
        path: PathBuf,
        #[source]
        source: AreaConfigError,
    },
    #[error("Refusing to extract unsafe tarball entry {entry:?}: {reason}")]
    UnsafeTarEntry { entry: String, reason: &'static str },
}

pub type AreaResult<T> = Result<T, AreaRegistryError>;

pub const REGISTRY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Registry {
    pub schema_version: u32,
    pub areas: Vec<RegistryEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RegistryEntry {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub version: Version,
    pub download_url: String,
    pub checksum_sha256: String,
    #[serde(default)]
    pub maintainers: Vec<String>,
}

/// Fetch a registry from `url` and parse it. Pure HTTP+JSON — no caching;
/// callers decide if/where to cache.
pub async fn fetch_registry(url: &str) -> AreaResult<Registry> {
    let body = reqwest::Client::builder()
        .build()?
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    let registry: Registry = serde_json::from_str(&body)?;
    if registry.schema_version != REGISTRY_SCHEMA_VERSION {
        return Err(AreaRegistryError::UnsupportedSchema {
            found: registry.schema_version,
            supported: REGISTRY_SCHEMA_VERSION,
        });
    }
    Ok(registry)
}

/// Fetch the primary registry plus any additional `extra_registries` from
/// the top-level config. Returns entries deduplicated by name, with later
/// registries winning ties.
pub async fn fetch_combined_registry(config: &TopLevelConfig) -> AreaResult<Registry> {
    let mut entries: Vec<RegistryEntry> = Vec::new();
    let mut sources = vec![config.area_registry_url.clone()];
    sources.extend(config.extra_registries.iter().cloned());

    for url in sources {
        match fetch_registry(&url).await {
            Ok(reg) => entries.extend(reg.areas),
            Err(e) => warn!(%url, error = ?e, "Failed to fetch registry, continuing"),
        }
    }

    // dedupe by name, later wins
    let mut by_name: HashMap<String, RegistryEntry> = HashMap::new();
    for entry in entries {
        by_name.insert(entry.name.clone(), entry);
    }

    let mut deduped: Vec<RegistryEntry> = by_name.into_values().collect();
    deduped.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(Registry {
        schema_version: REGISTRY_SCHEMA_VERSION,
        areas: deduped,
    })
}

/// Download `entry`'s tarball, verify the SHA-256, and untar it into
/// `<install_dir>/<entry.name>/`, replacing any existing directory there.
/// Returns the install path.
///
/// The extraction is atomic: the tarball is staged in a sibling temp dir under
/// `install_dir` and `rename`d over the target once every entry has been
/// verified and written successfully. A failure midway through extraction (or
/// a malicious entry) leaves the previously installed area untouched.
pub async fn install_area(entry: &RegistryEntry, install_dir: &Path) -> AreaResult<PathBuf> {
    let bytes = reqwest::Client::builder()
        .build()?
        .get(&entry.download_url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    verify_checksum(&entry.name, &entry.checksum_sha256, &bytes)?;

    fs::create_dir_all(install_dir)?;

    let staging = TempDir::new_in(install_dir)?;
    extract_tar_gz_safe(&bytes, staging.path())?;

    let target_dir = install_dir.join(&entry.name);
    if target_dir.exists() {
        fs::remove_dir_all(&target_dir)?;
    }
    let staged_path = staging.keep();
    fs::rename(&staged_path, &target_dir)?;

    info!(name = %entry.name, version = %entry.version, dest = %target_dir.display(), "Installed area");
    Ok(target_dir)
}

/// Enumerate installed areas under `install_dir`. Subdirectories that are
/// missing a `manifest.toml` are skipped with a warning.
pub fn list_installed_areas(install_dir: &Path) -> AreaResult<Vec<(PathBuf, AreaManifest)>> {
    if !install_dir.exists() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    for entry in fs::read_dir(install_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest_path = path.join("manifest.toml");
        if !manifest_path.exists() {
            warn!(path = %path.display(), "Skipping directory without manifest.toml");
            continue;
        }
        match load_area_manifest(&path) {
            Ok(m) => out.push((path, m)),
            Err(source) => {
                return Err(AreaRegistryError::ManifestRead { path, source });
            }
        }
    }
    Ok(out)
}

/// Remove an installed area by name. No-op if it isn't installed.
pub fn remove_area(install_dir: &Path, name: &str) -> AreaResult<()> {
    let path = install_dir.join(name);
    if path.exists() {
        fs::remove_dir_all(&path)?;
        info!(%name, "Removed area");
    }
    Ok(())
}

fn verify_checksum(name: &str, expected_hex: &str, bytes: &[u8]) -> AreaResult<()> {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let actual = hex::encode(hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected_hex) {
        return Err(AreaRegistryError::ChecksumMismatch {
            name: name.to_string(),
            expected: expected_hex.to_string(),
            actual,
        });
    }
    Ok(())
}

/// Extract a `.tar.gz` into `dest` after validating every entry.
///
/// Rejects absolute paths, parent (`..`) components, and non-regular-file /
/// non-directory entries (notably symlinks and hardlinks), all of which can
/// be used to escape `dest`. `dest` itself is *not* canonicalized — callers
/// pass a freshly created directory and the per-entry checks make any
/// resulting path stay underneath it.
fn extract_tar_gz_safe(bytes: &[u8], dest: &Path) -> AreaResult<()> {
    let gz = GzDecoder::new(Cursor::new(bytes));
    let mut archive = Archive::new(gz);
    archive.set_overwrite(false);
    archive.set_preserve_permissions(false);
    archive.set_unpack_xattrs(false);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_path = entry.path()?.into_owned();
        validate_entry_path(&entry_path)?;
        validate_entry_type(&entry_path, entry.header().entry_type())?;
        entry.unpack_in(dest)?;
    }
    Ok(())
}

fn validate_entry_path(entry_path: &Path) -> AreaResult<()> {
    let entry_str = entry_path.display().to_string();
    for component in entry_path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => {
                return Err(AreaRegistryError::UnsafeTarEntry {
                    entry: entry_str,
                    reason: "contains a parent-directory component",
                });
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(AreaRegistryError::UnsafeTarEntry {
                    entry: entry_str,
                    reason: "is an absolute path",
                });
            }
        }
    }
    Ok(())
}

fn validate_entry_type(entry_path: &Path, entry_type: EntryType) -> AreaResult<()> {
    let entry_str = || entry_path.display().to_string();
    match entry_type {
        EntryType::Regular | EntryType::Directory | EntryType::GNUSparse => Ok(()),
        EntryType::Symlink | EntryType::Link => Err(AreaRegistryError::UnsafeTarEntry {
            entry: entry_str(),
            reason: "is a symlink or hardlink",
        }),
        other => Err(AreaRegistryError::UnsafeTarEntry {
            entry: entry_str(),
            reason: match other {
                EntryType::Char => "is a character device entry",
                EntryType::Block => "is a block device entry",
                EntryType::Fifo => "is a FIFO entry",
                _ => "has an unsupported entry type",
            },
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use tempfile::tempdir;

    fn make_tar_gz(files: &[(&str, &str)]) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let gz = GzEncoder::new(&mut tar_bytes, Compression::default());
            let mut builder = tar::Builder::new(gz);
            for (path, content) in files {
                let mut header = tar::Header::new_gnu();
                header.set_size(content.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder
                    .append_data(&mut header, path, content.as_bytes())
                    .unwrap();
            }
            builder.into_inner().unwrap().finish().unwrap();
        }
        tar_bytes
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(bytes);
        hex::encode(h.finalize())
    }

    #[test]
    fn verify_checksum_accepts_matching_hex() {
        let data = b"hello";
        let hex = sha256_hex(data);
        verify_checksum("x", &hex, data).unwrap();
    }

    #[test]
    fn verify_checksum_is_case_insensitive() {
        let data = b"hello";
        let hex = sha256_hex(data).to_uppercase();
        verify_checksum("x", &hex, data).unwrap();
    }

    #[test]
    fn verify_checksum_rejects_mismatch() {
        let err = verify_checksum("x", "deadbeef", b"hello").unwrap_err();
        assert!(matches!(err, AreaRegistryError::ChecksumMismatch { .. }));
    }

    #[test]
    fn extract_tar_gz_writes_files_to_destination() {
        let bytes = make_tar_gz(&[("manifest.toml", "name = \"x\"\n")]);
        let dir = tempdir().unwrap();
        extract_tar_gz_safe(&bytes, dir.path()).unwrap();
        assert!(dir.path().join("manifest.toml").exists());
    }

    fn make_tar_gz_with_symlink(link_name: &str, target: &str) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let gz = GzEncoder::new(&mut tar_bytes, Compression::default());
            let mut builder = tar::Builder::new(gz);
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_mode(0o777);
            builder.append_link(&mut header, link_name, target).unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        tar_bytes
    }

    #[test]
    fn extract_tar_gz_rejects_symlinks() {
        // The high-level `tar::Builder` rejects `..` / absolute paths at
        // append time, so the parent-dir / absolute-path branches are
        // covered by the `validate_entry_path` unit tests below. Symlinks
        // can still be constructed via `append_link`, so we keep the
        // integration test for that branch.
        let bytes = make_tar_gz_with_symlink("link", "/etc/passwd");
        let dir = tempdir().unwrap();
        let err = extract_tar_gz_safe(&bytes, dir.path()).unwrap_err();
        assert!(matches!(err, AreaRegistryError::UnsafeTarEntry { .. }));
    }

    #[test]
    fn validate_entry_path_rejects_parent_dir() {
        let err = validate_entry_path(Path::new("../escape.txt")).unwrap_err();
        assert!(matches!(err, AreaRegistryError::UnsafeTarEntry { .. }));
    }

    #[test]
    fn validate_entry_path_rejects_parent_dir_in_middle() {
        let err = validate_entry_path(Path::new("plugin/../../../etc/passwd")).unwrap_err();
        assert!(matches!(err, AreaRegistryError::UnsafeTarEntry { .. }));
    }

    #[test]
    fn validate_entry_path_rejects_absolute_paths() {
        let err = validate_entry_path(Path::new("/etc/passwd")).unwrap_err();
        assert!(matches!(err, AreaRegistryError::UnsafeTarEntry { .. }));
    }

    #[test]
    fn validate_entry_path_accepts_relative_paths() {
        validate_entry_path(Path::new("manifest.toml")).unwrap();
        validate_entry_path(Path::new("plugin/area_enor")).unwrap();
        validate_entry_path(Path::new("./profiles/twr.toml")).unwrap();
    }

    #[test]
    fn validate_entry_type_rejects_devices_and_fifos() {
        for ty in [
            tar::EntryType::Char,
            tar::EntryType::Block,
            tar::EntryType::Fifo,
        ] {
            let err = validate_entry_type(Path::new("x"), ty).unwrap_err();
            assert!(matches!(err, AreaRegistryError::UnsafeTarEntry { .. }));
        }
    }

    #[test]
    fn list_installed_returns_empty_when_dir_missing() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("does_not_exist");
        let result = list_installed_areas(&missing).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn list_installed_skips_dirs_without_manifest() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("no_manifest")).unwrap();
        let result = list_installed_areas(dir.path()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn list_installed_returns_manifest_when_present() {
        let dir = tempdir().unwrap();
        let area_dir = dir.path().join("enor");
        fs::create_dir_all(&area_dir).unwrap();
        fs::write(
            area_dir.join("manifest.toml"),
            r#"
name = "enor"
version = "0.1.0"
display_name = "Polaris / ENOR"
runtime = "rust"
entry = "area_enor"
"#,
        )
        .unwrap();

        let result = list_installed_areas(dir.path()).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].1.name, "enor");
    }

    #[test]
    fn remove_area_is_noop_when_not_installed() {
        let dir = tempdir().unwrap();
        remove_area(dir.path(), "missing").unwrap();
    }

    #[test]
    fn remove_area_deletes_existing_dir() {
        let dir = tempdir().unwrap();
        let area_dir = dir.path().join("enor");
        fs::create_dir_all(&area_dir).unwrap();
        fs::write(area_dir.join("file"), "data").unwrap();

        remove_area(dir.path(), "enor").unwrap();
        assert!(!area_dir.exists());
    }
}
