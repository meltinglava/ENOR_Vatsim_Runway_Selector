//! First-run wizard: prints guidance when the user starts the binary without
//! having installed any area packages, or with an area installed but no
//! profiles configured.
//!
//! Non-interactive — we only emit messages. Interactive prompts would need
//! `rfd` (GUI) or a stdin reader, both of which would block CI / non-TTY
//! users. The intent is to point the user at the right `area …` subcommand.

use std::path::{Path, PathBuf};

use runway_selector_areas::list_installed_areas;
use runway_selector_core::{
    AreaManifest,
    area_config::{ProfileConfig, load_profile_config},
};
use tracing::info;

use crate::error::ApplicationResult;

/// Returned by [`detect_setup_state`] — what the user needs to do next.
#[derive(Debug, PartialEq, Eq)]
pub enum SetupState {
    /// No area packages installed at all.
    NoAreasInstalled { suggested: Option<&'static str> },
    /// At least one area installed, but it has no profiles defined.
    AreaInstalledNoProfiles { area_name: String },
    /// Ready — at least one area with profiles is installed.
    Ready { area_count: usize },
}

/// Walk the install directory, look at the sector file the host detected,
/// and report which setup state we're in. Pure — no I/O outside reading the
/// area dirs and profile files.
pub fn detect_setup_state(
    install_dir: &Path,
    sector_file_prefix: Option<&str>,
) -> ApplicationResult<SetupState> {
    let installed = list_installed_areas(install_dir)?;

    if installed.is_empty() {
        let suggested = suggested_area_for_prefix(sector_file_prefix);
        return Ok(SetupState::NoAreasInstalled { suggested });
    }

    let mut area_count = 0usize;
    for (path, manifest) in &installed {
        if profiles_in_area(path).is_empty() {
            return Ok(SetupState::AreaInstalledNoProfiles {
                area_name: manifest.name.clone(),
            });
        }
        area_count += 1;
    }
    Ok(SetupState::Ready { area_count })
}

/// Best-effort mapping from a sector file prefix (e.g. `ENOR-Norway-NC`) to
/// the area name that almost certainly handles it. Used for the
/// "we suggest installing X" message — adding entries is cheap and
/// non-binding.
fn suggested_area_for_prefix(prefix: Option<&str>) -> Option<&'static str> {
    let prefix = prefix?;
    if prefix.starts_with("ENOR") {
        Some("enor")
    } else if prefix.starts_with("ESAA") || prefix.starts_with("ESOS") {
        Some("esos")
    } else if prefix.starts_with("EGTT") {
        Some("egtt")
    } else {
        None
    }
}

/// Enumerate profile files inside an installed area's `profiles/` directory.
/// Empty when the directory is missing or contains no `.toml` files.
pub fn profiles_in_area(area_dir: &Path) -> Vec<PathBuf> {
    let dir = area_dir.join("profiles");
    let Ok(read_dir) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    read_dir
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()) == Some("toml")
                && !p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(".local.toml"))
        })
        .collect()
}

/// Load a profile by `(area_name, profile_name)` from `install_dir`.
pub fn load_profile_in_area(
    install_dir: &Path,
    area_name: &str,
    profile_name: &str,
) -> ApplicationResult<Option<ProfileConfig>> {
    let path = install_dir
        .join(area_name)
        .join("profiles")
        .join(format!("{profile_name}.toml"));
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(load_profile_config(&path)?))
}

/// Print the appropriate first-run message for the given state. The host
/// then proceeds to its normal flow — the wizard never blocks.
pub fn print_setup_state(state: &SetupState) {
    match state {
        SetupState::NoAreasInstalled {
            suggested: Some(area),
        } => {
            println!(
                "No area plugins installed. The detected sector file looks like {area}; \
                 run `es_runway_selector area install {area}` to install it."
            );
        }
        SetupState::NoAreasInstalled { suggested: None } => {
            println!(
                "No area plugins installed. Run `es_runway_selector area available` to see \
                 installable areas."
            );
        }
        SetupState::AreaInstalledNoProfiles { area_name } => {
            println!(
                "Area `{area_name}` is installed but has no profiles. Add profile files in \
                 `<area>/profiles/<name>.toml` before launching."
            );
        }
        SetupState::Ready { area_count } => {
            info!(area_count, "Area plugins installed");
        }
    }
}

/// List every installed area with the profiles it exposes. Drives
/// `es_runway_selector area profile list`.
pub fn list_areas_with_profiles(
    install_dir: &Path,
) -> ApplicationResult<Vec<(AreaManifest, Vec<ProfileConfig>)>> {
    let installed = list_installed_areas(install_dir)?;
    let mut out = Vec::new();
    for (path, manifest) in installed {
        let mut profiles = Vec::new();
        for profile_path in profiles_in_area(&path) {
            if let Ok(p) = load_profile_config(&profile_path) {
                profiles.push(p);
            }
        }
        out.push((manifest, profiles));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_manifest(area_dir: &Path, name: &str) {
        fs::create_dir_all(area_dir).unwrap();
        fs::write(
            area_dir.join("manifest.toml"),
            format!(
                r#"
name = "{name}"
version = "0.1.0"
display_name = "{name}"
runtime = "rust"
entry = "x"
"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn detect_returns_no_areas_when_install_dir_empty() {
        let dir = tempdir().unwrap();
        let state = detect_setup_state(dir.path(), Some("ENOR")).unwrap();
        assert_eq!(
            state,
            SetupState::NoAreasInstalled {
                suggested: Some("enor"),
            },
        );
    }

    #[test]
    fn detect_returns_no_areas_without_suggestion_for_unknown_prefix() {
        let dir = tempdir().unwrap();
        let state = detect_setup_state(dir.path(), Some("UNKNOWN")).unwrap();
        assert_eq!(state, SetupState::NoAreasInstalled { suggested: None });
    }

    #[test]
    fn detect_flags_installed_area_without_profiles() {
        let dir = tempdir().unwrap();
        let area = dir.path().join("enor");
        write_manifest(&area, "enor");
        let state = detect_setup_state(dir.path(), Some("ENOR")).unwrap();
        assert_eq!(
            state,
            SetupState::AreaInstalledNoProfiles {
                area_name: "enor".to_string(),
            },
        );
    }

    #[test]
    fn detect_returns_ready_when_area_has_profiles() {
        let dir = tempdir().unwrap();
        let area = dir.path().join("enor");
        write_manifest(&area, "enor");
        fs::create_dir_all(area.join("profiles")).unwrap();
        fs::write(
            area.join("profiles/twr.toml"),
            "name = \"twr\"\ndisplay_name = \"Tower\"\n",
        )
        .unwrap();

        let state = detect_setup_state(dir.path(), Some("ENOR")).unwrap();
        assert_eq!(state, SetupState::Ready { area_count: 1 });
    }

    #[test]
    fn profiles_in_area_skips_local_toml() {
        let dir = tempdir().unwrap();
        let area = dir.path().join("enor");
        let profiles = area.join("profiles");
        fs::create_dir_all(&profiles).unwrap();
        fs::write(profiles.join("twr.toml"), "").unwrap();
        fs::write(profiles.join("twr.local.toml"), "").unwrap();

        let listed = profiles_in_area(&area);
        assert_eq!(listed.len(), 1);
        assert!(listed[0].ends_with("twr.toml"));
    }

    #[test]
    fn suggested_area_prefix_matches_known_firs() {
        assert_eq!(suggested_area_for_prefix(Some("ENOR-Norway")), Some("enor"));
        assert_eq!(suggested_area_for_prefix(Some("ESAA-Sweden")), Some("esos"));
        assert_eq!(suggested_area_for_prefix(Some("EGTT-UK")), Some("egtt"));
        assert_eq!(suggested_area_for_prefix(Some("LFFF-France")), None);
        assert_eq!(suggested_area_for_prefix(None), None);
    }
}
