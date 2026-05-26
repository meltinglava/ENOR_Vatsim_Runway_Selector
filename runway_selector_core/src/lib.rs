//! Area-agnostic runway-selection logic for the `es_runway_selector` tool.
//!
//! Core owns everything that does not depend on a specific FIR (Polaris,
//! Stockholm, etc.):
//!
//! - sector file decoding ([`sector_file`])
//! - METAR fetching ([`metar`]) — VATSIM URL list passed by the caller
//! - ATIS regex parsing ([`atis`])
//! - runway wind component math ([`airport`])
//! - the runway-source priority model ([`airport::RunwayInUseSource`])
//! - the `.rwy` output writer ([`output`])
//! - the HTML runway report (rendered from [`airports::Airports`])
//!
//! Area-specific selection logic for airports with custom rules (currently
//! ENGM and ENZV; planned to move out to a separate plugin crate in a later
//! refactor phase) lives temporarily on [`airport::Airport`].

pub mod airport;
pub mod airports;
pub mod atis;
pub mod error;
pub mod metar;
pub mod output;
pub mod runway;
pub mod sector_file;
pub mod util;

pub use airport::{Airport, CrosswindDirection, RunwayInUseSource, RunwayWindComponents};
pub use airports::Airports;
pub use error::{CoreError, CoreResult};
pub use runway::{Runway, RunwayDirection, RunwayUse};
