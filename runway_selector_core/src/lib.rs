//! Area-agnostic runway-selection support for the `es_runway_selector` tool.
//!
//! Core owns the host-side runtime concerns that do not depend on a specific
//! FIR (Polaris, Stockholm, …):
//!
//! - sector file decoding ([`sector_file`])
//! - METAR fetching ([`metar`]) — VATSIM URL list passed by the caller
//! - ATIS regex parsing ([`atis`])
//! - runway wind component math ([`airport`])
//! - the runway-source priority model ([`airport::RunwayInUseSource`])
//! - the host-side proto converter that lowers parsed METARs and runway
//!   state into the gRPC plugin contract ([`proto_convert`])
//! - the `.rwy` output writer ([`output`])
//! - the HTML runway report (rendered from [`airports::Airports`])
//!
//! Area-package configuration types (manifest, `area.toml`, profiles,
//! top-level config) live in the smaller [`runway_selector_area_config`]
//! crate so plugins can depend on them without pulling in core's HTTP /
//! HTML / VATSIM stack. Core re-exports the most common ones for
//! convenience, but new code should import from `runway_selector_area_config`
//! directly.
//!
//! Area-specific runway-selection logic (the ENGM time-of-day modes, the
//! ENZV crosswind switch, generic max-headwind) lives in the area plugin
//! crates (e.g. `area_enor`) and runs in a subprocess that talks to the
//! host over gRPC.

pub mod airport;
pub mod airports;
pub mod atis;
pub mod error;
pub mod metar;
pub mod output;
pub mod proto_convert;
pub mod runway;
pub mod sector_file;
pub mod util;

pub use airport::{Airport, CrosswindDirection, RunwayInUseSource, RunwayWindComponents};
pub use airports::Airports;
pub use error::{CoreError, CoreResult};
pub use runway::{Runway, RunwayDirection, RunwayUse};

/// Re-export of the area-config crate for callers that already depend on
/// `runway_selector_core`. New crates should depend on
/// `runway_selector_area_config` directly.
pub use runway_selector_area_config as area_config;
pub use runway_selector_area_config::{
    AreaConfig, AreaManifest, ProfileConfig, Runtime, TopLevelConfig, load_area_config,
    load_area_manifest, load_profile_config, merge_local_overrides,
};
