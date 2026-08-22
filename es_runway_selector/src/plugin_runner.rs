//! Drive the active area's plugin over HTTP/JSON.
//!
//! The area that owns the run is the one whose `sector_file_prefix` matches
//! the sector file the user opened. It owns every airport found in that
//! sector file's `[RUNWAY]` section — `area.toml`'s `ignore_airports` was
//! already applied when the sector file was loaded, so there is no separate
//! airport list to maintain. The plugin is spawned via
//! [`runway_selector_plugin_host`], receives one batch
//! `POST /runway-selections`, and has its selections written back onto
//! [`Airports`].
//!
//! ATIS is applied by the host before this runs; airports that already have
//! an ATIS selection are *not* sent to the plugin (no pointless round-trip,
//! no double handling).
//!
//! A missing, crashed, or erroring plugin never breaks the run: the failure
//! is logged, reported in the returned [`AreaRunStatus`], and the host falls
//! back to built-in defaults for the area's airports.

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

/// Run runway selection through the active area's plugin. `None` when no
/// installed area matched the sector file — nothing to run.
///
/// Never returns an error: plugin problems degrade to defaults.
pub async fn run_area_selections(
    airports: &mut Airports,
    active_area: Option<&InstalledArea>,
) -> Option<AreaRunStatus> {
    let area = active_area?;
    let now_utc = Timestamp::now();
    let status = run_single_area(airports, area, now_utc).await;
    info!("{}", status.user_message());
    Some(status)
}

/// Airports the plugin should decide: everything the sector file produced
/// (the ignore list was applied at load time) that ATIS has not already
/// decided — the host applies ATIS itself.
fn eligible_icaos(airports: &Airports) -> Vec<String> {
    airports
        .airports
        .iter()
        .filter(|(_, airport)| {
            !airport
                .runways_in_use
                .contains_key(&RunwayInUseSource::Atis)
        })
        .map(|(icao, _)| icao.clone())
        .collect()
}

async fn run_single_area(
    airports: &mut Airports,
    area: &InstalledArea,
    now_utc: Timestamp,
) -> AreaRunStatus {
    let name = area.manifest.name.clone();
    let eligible = eligible_icaos(airports);

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
    use runway_selector_core::airport::Airport;

    fn airport(icao: &str) -> Airport {
        Airport {
            icao: icao.into(),
            metar: None,
            runways: vec![],
            runways_in_use: IndexMap::new(),
            selection_tags: vec![],
        }
    }

    #[test]
    fn eligible_skips_atis_decided_airports() {
        let mut airports = Airports::new();
        airports
            .airports
            .insert("ENGM".to_string(), airport("ENGM"));
        let mut atis_decided = airport("ENBR");
        atis_decided
            .runways_in_use
            .insert(RunwayInUseSource::Atis, IndexMap::new());
        airports.airports.insert("ENBR".to_string(), atis_decided);

        assert_eq!(eligible_icaos(&airports), vec!["ENGM".to_string()]);
    }

    #[test]
    fn timestamp_formats_as_rfc3339_utc() {
        let ts: Timestamp = "2026-05-31T21:00:00Z".parse().unwrap();
        assert_eq!(format_rfc3339_utc(ts), "2026-05-31T21:00:00Z");
    }
}
