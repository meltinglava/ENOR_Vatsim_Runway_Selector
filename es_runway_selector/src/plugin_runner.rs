//! Drive an area plugin over gRPC.
//!
//! Spawns the area's subprocess via [`runway_selector_plugin_host`], calls
//! `SelectRunways` for every airport the plugin claims, and writes the
//! returned selections back onto [`Airports`] using the appropriate
//! [`RunwayInUseSource`].

use std::time::SystemTime;

use anyhow::{Context, Result};
use indexmap::IndexMap;
use runway_selector_core::{
    Airports, RunwayInUseSource,
    proto_convert::{airport_to_request, runway_use_from_proto, selection_source_from_proto},
    runway::RunwayUse,
};
use runway_selector_plugin_host::{PluginHandle, spawn_plugin};
use runway_selector_protocol::v1::{
    AirportSelection, RunwayUse as ProtoRunwayUse, SelectRunwaysRequest, SelectionSource,
    runway_selector_client::RunwaySelectorClient,
};
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

/// Spawn `area`'s plugin, ask it which ICAOs it owns, send a `SelectRunways`
/// request for those airports, and write the returned selections into
/// `airports.runways_in_use`. The plugin handle is shut down before
/// returning on the happy path; on error the handle is dropped, which
/// SIGKILLs the child via `kill_on_drop`.
pub async fn run_area_selection(airports: &mut Airports, area: &InstalledArea) -> Result<()> {
    info!(name = %area.manifest.name, version = %area.manifest.version, "Spawning area plugin");
    let handle = spawn_plugin(&area.manifest, &area.area_dir, &host_version())
        .await
        .with_context(|| format!("Spawning area plugin {}", area.manifest.name))?;

    drive_plugin(
        airports,
        handle,
        area.config.time_zone.as_deref(),
        &area.manifest.name,
    )
    .await
}

async fn drive_plugin(
    airports: &mut Airports,
    handle: PluginHandle,
    area_time_zone: Option<&str>,
    area_name: &str,
) -> Result<()> {
    let mut client = RunwaySelectorClient::new(handle.channel.clone());

    let claimed: Vec<String> = client
        .get_airports(())
        .await
        .with_context(|| format!("Calling GetAirports on plugin {area_name}"))?
        .into_inner()
        .icaos;
    let area_timezone = area_time_zone.unwrap_or("UTC").to_string();

    let request_airports = claimed
        .iter()
        .filter_map(|icao| airports.airports.get(icao).map(|a| (icao.clone(), a)))
        .map(|(_, airport)| {
            let atis = airport
                .runways_in_use
                .get(&RunwayInUseSource::Atis)
                .cloned()
                .unwrap_or_default();
            airport_to_request(airport, &atis)
        })
        .collect::<Vec<_>>();

    if request_airports.is_empty() {
        info!("Area plugin claimed no airports that the sector file knows about");
        let _ = handle.shutdown().await;
        return Ok(());
    }

    let req = SelectRunwaysRequest {
        now_utc: Some(prost_timestamp_now()),
        area_timezone,
        airports: request_airports,
    };

    let response = client
        .select_runways(req)
        .await
        .with_context(|| format!("Calling SelectRunways on plugin {area_name}"))?
        .into_inner();
    apply_selections(airports, response.selections);

    if let Err(e) = handle.shutdown().await {
        warn!(area = %area_name, error = ?e, "Plugin shutdown returned an error");
    }
    Ok(())
}

fn apply_selections(airports: &mut Airports, selections: Vec<AirportSelection>) {
    for sel in selections {
        let Some(airport) = airports.airports.get_mut(&sel.icao) else {
            continue;
        };

        let Ok(proto_source) = SelectionSource::try_from(sel.source) else {
            warn!(icao = %sel.icao, source = sel.source, "Unknown selection source");
            continue;
        };
        let Some(source) = selection_source_from_proto(proto_source) else {
            warn!(icao = %sel.icao, "Plugin returned SELECTION_SOURCE_UNSPECIFIED");
            continue;
        };

        let mut map = IndexMap::new();
        for assignment in sel.runways {
            let Ok(proto_use) = ProtoRunwayUse::try_from(assignment.r#use) else {
                warn!(icao = %sel.icao, "Unknown runway use value");
                continue;
            };
            let Some(runway_use) = runway_use_from_proto(proto_use) else {
                continue;
            };
            map.entry(assignment.identifier)
                .and_modify(|existing: &mut RunwayUse| *existing = existing.merged_with(runway_use))
                .or_insert(runway_use);
        }

        airport.runways_in_use.insert(source, map);
    }
}

fn prost_timestamp_now() -> prost_types::Timestamp {
    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    prost_types::Timestamp {
        seconds: duration.as_secs() as i64,
        nanos: duration.subsec_nanos() as i32,
    }
}
