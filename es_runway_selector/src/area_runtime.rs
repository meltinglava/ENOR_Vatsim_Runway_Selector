//! Host-side runtime view of installed areas.
//!
//! [`InstalledArea`] bundles an area's manifest, its merged
//! `area.toml`/`area.local.toml` config, and its on-disk path so the rest of
//! the host can drive runway selection without re-reading the same files at
//! every layer. [`load_installed_areas`] enumerates everything under the
//! install dir; [`match_area_for_prefix`] picks the one whose
//! `sector_file_prefix` matches a sector file the user opened.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use runway_selector_area_config::{AreaConfig, AreaManifest, load_area_config};
use runway_selector_areas::list_installed_areas;
use tracing::warn;

/// An installed area together with its loaded runtime config.
pub struct InstalledArea {
    pub area_dir: PathBuf,
    pub manifest: AreaManifest,
    pub config: AreaConfig,
}

/// Enumerate installed areas under `install_dir` and load each one's merged
/// `area.toml`/`area.local.toml`. Areas whose `area.toml` fails to parse are
/// skipped with a warning rather than aborting the whole host startup.
pub fn load_installed_areas(install_dir: &Path) -> Result<Vec<InstalledArea>> {
    let installed = list_installed_areas(install_dir)
        .with_context(|| format!("Enumerating installed areas in {}", install_dir.display()))?;
    let mut out = Vec::with_capacity(installed.len());
    for (area_dir, manifest) in installed {
        match load_area_config(&area_dir) {
            Ok(config) => out.push(InstalledArea {
                area_dir,
                manifest,
                config,
            }),
            Err(e) => warn!(
                area = %manifest.name,
                error = ?e,
                "Failed to load area.toml; skipping this area for the rest of startup"
            ),
        }
    }
    Ok(out)
}

/// Collect every `sector_file_prefix` declared by installed areas.
pub fn installed_sector_file_prefixes(areas: &[InstalledArea]) -> Vec<String> {
    areas
        .iter()
        .filter_map(|a| a.config.sector_file_prefix.clone())
        .collect()
}

/// Pick the area that owns a sector file with the given prefix. Matches when
/// `sct_prefix` starts with the area's declared `sector_file_prefix` — the
/// sct filename is typically `<prefix>-Region-Variant.sct`, so a prefix of
/// `ENOR` matches `ENOR-Norway-NC` etc.
pub fn match_area_for_prefix<'a>(
    areas: &'a [InstalledArea],
    sct_prefix: &str,
) -> Option<&'a InstalledArea> {
    areas.iter().find(|a| {
        a.config
            .sector_file_prefix
            .as_deref()
            .is_some_and(|p| sct_prefix.starts_with(p))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use runway_selector_area_config::Runtime;
    use semver::Version;

    fn fake_area(name: &str, sct_prefix: &str) -> InstalledArea {
        InstalledArea {
            area_dir: PathBuf::from("/nonexistent").join(name),
            manifest: AreaManifest {
                name: name.into(),
                version: Version::new(0, 1, 0),
                display_name: name.into(),
                description: None,
                runtime: Runtime::Rust,
                entry: name.into(),
                supported_icaos: vec![],
                min_core_version: None,
            },
            config: AreaConfig {
                metar_urls: vec![],
                ignore_airports: Default::default(),
                default_runways: IndexMap::new(),
                time_zone: None,
                sector_file_prefix: Some(sct_prefix.into()),
            },
        }
    }

    #[test]
    fn match_area_for_prefix_matches_on_starts_with() {
        let areas = vec![fake_area("enor", "ENOR"), fake_area("esos", "ESAA")];
        let m = match_area_for_prefix(&areas, "ENOR-Norway-NC").unwrap();
        assert_eq!(m.manifest.name, "enor");
    }

    #[test]
    fn match_area_for_prefix_returns_none_when_no_match() {
        let areas = vec![fake_area("enor", "ENOR")];
        assert!(match_area_for_prefix(&areas, "EGTT-UK").is_none());
    }

    #[test]
    fn installed_sector_file_prefixes_collects_all_declared() {
        let areas = vec![fake_area("a", "ENOR"), fake_area("b", "ESAA")];
        let mut prefixes = installed_sector_file_prefixes(&areas);
        prefixes.sort();
        assert_eq!(prefixes, vec!["ENOR", "ESAA"]);
    }
}
