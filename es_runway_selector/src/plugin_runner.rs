//! Drive an area plugin over gRPC.
//!
//! Looks up the requested installed area, spawns its subprocess via
//! [`runway_selector_plugin_host`], calls `SelectRunways` for every airport
//! the plugin claims, and writes the returned selections back onto
//! [`Airports`] using the appropriate [`RunwayInUseSource`].

use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

use indexmap::IndexMap;
use runway_selector_areas::list_installed_areas;
use runway_selector_core::{
    Airports, RunwayInUseSource,
    area_config::{AreaManifest, load_area_config},
    proto_convert::{airport_to_request, runway_use_from_proto, selection_source_from_proto},
    runway::RunwayUse,
};
use runway_selector_plugin_host::{PluginHandle, spawn_plugin};
use runway_selector_protocol::v1::{
    AirportSelection, RunwayUse as ProtoRunwayUse, SelectRunwaysRequest, SelectionSource,
    runway_selector_client::RunwaySelectorClient,
};
use tracing::{info, warn};

use crate::error::ApplicationResult;

/// Find an installed area by name. Returns `Ok(None)` if no area with that
/// name is installed.
pub fn find_installed_area(
    install_dir: &Path,
    name: &str,
) -> ApplicationResult<Option<(PathBuf, AreaManifest)>> {
    let installed = list_installed_areas(install_dir)?;
    Ok(installed.into_iter().find(|(_, m)| m.name == name))
}

/// Spawn the area's plugin, ask it which ICAOs it owns, send a
/// `SelectRunways` request for those airports, and write the returned
/// selections into `airports.runways_in_use`. The plugin handle is shut
/// down before returning on the happy path.
pub async fn run_area_selection(
    airports: &mut Airports,
    area_dir: &Path,
    manifest: &AreaManifest,
) -> ApplicationResult<()> {
    let area_config = load_area_config(area_dir)?;

    info!(name = %manifest.name, version = %manifest.version, "Spawning area plugin");
    let handle = spawn_plugin(manifest, area_dir).await?;

    drive_plugin(airports, handle, area_config.time_zone.as_deref()).await
}

async fn drive_plugin(
    airports: &mut Airports,
    handle: PluginHandle,
    area_time_zone: Option<&str>,
) -> ApplicationResult<()> {
    let mut client = RunwaySelectorClient::new(handle.channel.clone());

    let claimed: Vec<String> = client.get_airports(()).await?.into_inner().icaos;
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

    let response = client.select_runways(req).await?.into_inner();
    apply_selections(airports, response.selections);

    let _ = handle.shutdown().await;
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
