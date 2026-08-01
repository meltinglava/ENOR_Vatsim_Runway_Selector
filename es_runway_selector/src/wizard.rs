//! First-run wizard and startup profile selection.
//!
//! The setup-state part prints guidance when the user starts the binary
//! without having installed any area packages, or with an area installed but
//! no profiles configured — it only emits messages and never blocks.
//!
//! [`choose_profile`] is the one interactive piece: it picks which of the
//! active area's profiles to launch. It only opens a terminal dialog when
//! there is a real choice to make (two or more profiles on an attended
//! terminal); otherwise it degrades silently, so CI / non-TTY runs are safe.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use dialoguer::{FuzzySelect, console::user_attended};
use runway_selector_area_config::{AreaManifest, ProfileConfig, load_profile_config};
use runway_selector_areas::list_installed_areas;
use tracing::{info, warn};

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
) -> Result<SetupState> {
    let installed = list_installed_areas(install_dir)
        .with_context(|| format!("Listing installed areas in {}", install_dir.display()))?;

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

/// Decide which of the active area's profiles to launch with.
///
/// Zero profiles returns `None` — the host keeps its plain
/// `app_launchers.toml` behaviour. Exactly one profile is used without ever
/// showing a dialog. Two or more open a fuzzy-select prompt; cancelling it
/// (or running without a terminal) also falls back to `None`.
pub fn choose_profile(area_dir: &Path, area_display_name: &str) -> Option<ProfileConfig> {
    let profiles: Vec<ProfileConfig> = profiles_in_area(area_dir)
        .iter()
        .filter_map(|path| match load_profile_config(path) {
            Ok(profile) => Some(profile),
            Err(e) => {
                warn!(path = %path.display(), error = ?e, "Skipping unparsable profile");
                None
            }
        })
        .collect();
    let chosen = pick_profile(profiles, |ps| prompt_for_profile(ps, area_display_name));
    if let Some(profile) = &chosen {
        info!(profile = %profile.name, "Using profile");
    }
    chosen
}

/// Selection rule, separated from the terminal so it is testable: the
/// `chooser` (the dialog) is only ever invoked when there are at least two
/// options.
fn pick_profile(
    profiles: Vec<ProfileConfig>,
    chooser: impl FnOnce(&[ProfileConfig]) -> Option<usize>,
) -> Option<ProfileConfig> {
    match profiles.len() {
        0 => None,
        1 => profiles.into_iter().next(),
        _ => {
            let idx = chooser(&profiles)?;
            profiles.into_iter().nth(idx)
        }
    }
}

fn prompt_for_profile(profiles: &[ProfileConfig], area_display_name: &str) -> Option<usize> {
    if !user_attended() {
        warn!(
            "Multiple profiles available but no attended terminal; \
             falling back to app_launchers.toml"
        );
        return None;
    }
    let items: Vec<String> = profiles
        .iter()
        .map(|p| format!("{} ({})", p.display_name, p.name))
        .collect();
    match FuzzySelect::new()
        .with_prompt(format!("Select profile for {area_display_name}"))
        .items(&items)
        .default(0)
        .interact_opt()
    {
        Ok(Some(idx)) => Some(idx),
        Ok(None) => {
            info!("Profile selection cancelled; falling back to app_launchers.toml");
            None
        }
        Err(e) => {
            warn!(error = ?e, "Profile selection failed; falling back to app_launchers.toml");
            None
        }
    }
}

/// Load a profile by `(area_name, profile_name)` from `install_dir`.
pub fn load_profile_in_area(
    install_dir: &Path,
    area_name: &str,
    profile_name: &str,
) -> Result<Option<ProfileConfig>> {
    let path = install_dir
        .join(area_name)
        .join("profiles")
        .join(format!("{profile_name}.toml"));
    if !path.exists() {
        return Ok(None);
    }
    let profile = load_profile_config(&path)
        .with_context(|| format!("Loading profile config {}", path.display()))?;
    Ok(Some(profile))
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
) -> Result<Vec<(AreaManifest, Vec<ProfileConfig>)>> {
    let installed = list_installed_areas(install_dir)
        .with_context(|| format!("Listing installed areas in {}", install_dir.display()))?;
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

    fn profile(name: &str) -> ProfileConfig {
        ProfileConfig {
            name: name.to_string(),
            display_name: name.to_uppercase(),
            prf_files: Vec::new(),
            default_apps: Vec::new(),
        }
    }

    #[test]
    fn pick_profile_returns_none_without_profiles() {
        let picked = pick_profile(Vec::new(), |_| panic!("dialog must not open"));
        assert!(picked.is_none());
    }

    #[test]
    fn pick_profile_skips_dialog_for_single_profile() {
        let picked = pick_profile(vec![profile("rads")], |_| panic!("dialog must not open"));
        assert_eq!(picked.unwrap().name, "rads");
    }

    #[test]
    fn pick_profile_uses_chooser_for_multiple_profiles() {
        let picked = pick_profile(vec![profile("rads"), profile("twr")], |ps| {
            assert_eq!(ps.len(), 2);
            Some(1)
        });
        assert_eq!(picked.unwrap().name, "twr");
    }

    #[test]
    fn pick_profile_cancelled_chooser_returns_none() {
        let picked = pick_profile(vec![profile("rads"), profile("twr")], |_| None);
        assert!(picked.is_none());
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
