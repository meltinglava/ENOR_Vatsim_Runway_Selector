//! Drive area plugins over HTTP/JSON.
//!
//! Multi-plugin: every installed area whose `manifest.toml supported_icaos`
//! (the single authoritative airport list) intersects the sector file gets
//! spawned via [`runway_selector_plugin_host`], receives one batch
//! `POST /runway-selections`, and has its selections written back onto
//! [`Airports`]. Ownership is disjoint — the first installed area to claim an
//! ICAO owns it and later claims are dropped with a warning.
//!
//! ATIS is applied by the host before this runs; airports that already have
//! an ATIS selection are *not* sent to plugins (no pointless round-trip, no
//! double handling).
//!
//! A missing, crashed, or erroring plugin never breaks the run: the failure
//! is logged, reported in the returned [`AreaRunStatus`], and the host falls
//! back to built-in defaults for that area's airports.

use std::collections::HashSet;

use indexmap::IndexMap;
use jiff::{Timestamp, Zoned, tz::TimeZone};
use runway_plugin_api::RunwaySelectionsRequest;
use runway_selector_core::{
    Airports, RunwayInUseSource,
    plugin_convert::{airport_to_request, runway_use_from_wire, selection_source_from_wire},
    runway::RunwayUse,
};
use runway_selector_plugin_host::spawn_plugin;
use self_update::cargo_crate_version;
use semver::Version;
use tracing::{info, warn};

use crate::area_runtime::InstalledArea;

/// Current host version, used for the plugin's `min_core_version` check.
fn host_version() -> Version {
    cargo_crate_version!()
        .parse()
        .expect("CARGO_PKG_VERSION is always a valid semver")
}

/// User-visible outcome of one area's selection run.
pub struct AreaRunStatus {
    pub area_name: String,
    pub outcome: AreaRunOutcome,
}

pub enum AreaRunOutcome {
    /// The plugin ran; `handled` airports got selections from it.
    Ok { handled: usize, deferred: usize },
    /// No airport in the sector file needed this area this run.
    NothingToDo,
    /// Spawn or request failed; built-in fallback covers its airports.
    Failed(String),
}

impl AreaRunStatus {
    pub fn user_message(&self) -> String {
        match &self.outcome {
            AreaRunOutcome::Ok { handled, deferred } => format!(
                "area {}: selected runways for {handled} airport(s), deferred {deferred} to defaults",
                self.area_name
            ),
            AreaRunOutcome::NothingToDo => {
                format!("area {}: no airports to decide this run", self.area_name)
            }
            AreaRunOutcome::Failed(e) => format!(
                "area {}: plugin failed ({e}); using built-in defaults for its airports",
                self.area_name
            ),
        }
    }
}

/// Run runway selection through every installed area plugin.
///
/// Returns one status per area so the caller can surface plugin failures to
/// the user. Never returns an error: plugin problems degrade to defaults.
pub async fn run_area_selections(
    airports: &mut Airports,
    areas: &[InstalledArea],
) -> Vec<AreaRunStatus> {
    let now_utc = Timestamp::now();
    let ownership = assign_airport_ownership(areas);

    let mut statuses = Vec::with_capacity(areas.len());
    for area in areas {
        let status = run_single_area(airports, area, &ownership, now_utc).await;
        info!("{}", status.user_message());
        statuses.push(status);
    }
    statuses
}

/// Disjoint ICAO ownership across areas: first installed area to claim an
/// ICAO in its manifest wins; duplicate claims are logged and dropped.
fn assign_airport_ownership(areas: &[InstalledArea]) -> IndexMap<String, String> {
    let mut owner_by_icao: IndexMap<String, String> = IndexMap::new();
    for area in areas {
        for icao in &area.manifest.supported_icaos {
            match owner_by_icao.entry(icao.clone()) {
                indexmap::map::Entry::Vacant(v) => {
                    v.insert(area.manifest.name.clone());
                }
                indexmap::map::Entry::Occupied(o) => {
                    warn!(
                        icao = %icao,
                        owner = %o.get(),
                        also_claimed_by = %area.manifest.name,
                        "Multiple areas claim the same airport; keeping the first owner"
                    );
                }
            }
        }
    }
    owner_by_icao
}

async fn run_single_area(
    airports: &mut Airports,
    area: &InstalledArea,
    ownership: &IndexMap<String, String>,
    now_utc: Timestamp,
) -> AreaRunStatus {
    let name = area.manifest.name.clone();

    // Airports this area owns, present in the sector file, and not already
    // decided by ATIS (the host applies ATIS itself — F6).
    let eligible: Vec<String> = area
        .manifest
        .supported_icaos
        .iter()
        .filter(|icao| ownership.get(*icao) == Some(&name))
        .filter(|icao| {
            airports.airports.get(*icao).is_some_and(|airport| {
                !airport
                    .runways_in_use
                    .contains_key(&RunwayInUseSource::Atis)
            })
        })
        .cloned()
        .collect();

    if eligible.is_empty() {
        return AreaRunStatus {
            area_name: name,
            outcome: AreaRunOutcome::NothingToDo,
        };
    }

    match drive_plugin(airports, area, &eligible, now_utc).await {
        Ok((handled, deferred)) => AreaRunStatus {
            area_name: name,
            outcome: AreaRunOutcome::Ok { handled, deferred },
        },
        Err(e) => {
            warn!(area = %name, error = %e, "Area plugin failed; falling back to defaults");
            AreaRunStatus {
                area_name: name,
                outcome: AreaRunOutcome::Failed(e),
            }
        }
    }
}

async fn drive_plugin(
    airports: &mut Airports,
    area: &InstalledArea,
    eligible: &[String],
    now_utc: Timestamp,
) -> Result<(usize, usize), String> {
    info!(
        name = %area.manifest.name,
        version = %area.manifest.version,
        airports = eligible.len(),
        "Spawning area plugin"
    );
    let handle = spawn_plugin(&area.manifest, &area.area_dir, &host_version())
        .await
        .map_err(|e| e.to_string())?;

    let request = RunwaySelectionsRequest {
        timestamp_utc: format_rfc3339_utc(now_utc),
        area_timezone: area
            .config
            .time_zone
            .clone()
            .unwrap_or_else(|| "UTC".to_string()),
        airports: eligible
            .iter()
            .filter_map(|icao| airports.airports.get(icao))
            .map(airport_to_request)
            .collect(),
    };

    let result = handle.select_runways(&request).await;

    if let Err(e) = handle.shutdown().await {
        warn!(area = %area.manifest.name, error = %e, "Plugin shutdown returned an error");
    }

    let response = result.map_err(|e| e.to_string())?;
    Ok(apply_results(
        airports,
        eligible,
        response.results,
        &area.manifest.name,
    ))
}

/// Write plugin selections back onto `airports`. Returns
/// `(handled, deferred)` counts. Results for airports the host never asked
/// about are ignored with a warning — a plugin cannot grab extra airports.
fn apply_results(
    airports: &mut Airports,
    eligible: &[String],
    results: Vec<runway_plugin_api::AirportSelectionResult>,
    area_name: &str,
) -> (usize, usize) {
    let asked: HashSet<&str> = eligible.iter().map(String::as_str).collect();
    let mut handled = 0usize;
    let mut deferred = 0usize;

    for result in results {
        if !asked.contains(result.icao.as_str()) {
            warn!(
                area = %area_name,
                icao = %result.icao,
                "Plugin answered for an airport it was not asked about; ignoring"
            );
            continue;
        }
        let Some(airport) = airports.airports.get_mut(&result.icao) else {
            continue;
        };

        if !result.handled {
            deferred += 1;
            continue;
        }
        handled += 1;

        let mut map: IndexMap<String, RunwayUse> = IndexMap::new();
        for entry in result.runway_uses {
            let runway_use = runway_use_from_wire(entry.use_);
            map.entry(entry.runway)
                .and_modify(|existing: &mut RunwayUse| *existing = existing.merged_with(runway_use))
                .or_insert(runway_use);
        }

        airport
            .runways_in_use
            .insert(selection_source_from_wire(result.source), map);
        airport.selection_tags = result.tags;
    }

    (handled, deferred)
}

/// RFC 3339 UTC with second precision, e.g. `2026-05-14T10:20:00Z`.
fn format_rfc3339_utc(ts: Timestamp) -> String {
    let zoned: Zoned = ts.to_zoned(TimeZone::UTC);
    zoned.strftime("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use runway_selector_area_config::{AreaConfig, AreaManifest, Runtime};
    use std::path::PathBuf;

    fn area(name: &str, icaos: &[&str]) -> InstalledArea {
        InstalledArea {
            area_dir: PathBuf::from("/nonexistent").join(name),
            manifest: AreaManifest {
                name: name.into(),
                version: Version::new(0, 1, 0),
                display_name: name.into(),
                description: None,
                runtime: Runtime::Rust,
                entry: name.into(),
                supported_icaos: icaos.iter().map(|s| s.to_string()).collect(),
                min_core_version: None,
            },
            config: AreaConfig::default(),
        }
    }

    #[test]
    fn ownership_is_first_claim_wins() {
        let areas = vec![
            area("enor", &["ENGM", "ENZV"]),
            area("esos", &["ENGM", "ESSA"]),
        ];
        let ownership = assign_airport_ownership(&areas);
        assert_eq!(ownership.get("ENGM").map(String::as_str), Some("enor"));
        assert_eq!(ownership.get("ESSA").map(String::as_str), Some("esos"));
        assert_eq!(ownership.get("ENZV").map(String::as_str), Some("enor"));
    }

    #[test]
    fn timestamp_formats_as_rfc3339_utc() {
        let ts: Timestamp = "2026-05-31T21:00:00Z".parse().unwrap();
        assert_eq!(format_rfc3339_utc(ts), "2026-05-31T21:00:00Z");
    }
}
