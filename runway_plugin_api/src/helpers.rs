//! Ready-made runway selection helpers.
//!
//! These cover the most common selection patterns so plugin authors don't
//! have to re-implement the same logic.  All helpers operate on slices of
//! [`RunwayInfo`] values from [`AirportSelectionRequest::runways`].
//!
//! # Quick reference
//!
//! | Helper | Use when… |
//! |--------|-----------|
//! | [`best_headwind`] | pick the runway with the most headwind from a set |
//! | [`prefer_unless_tailwind`] | use a fixed preferred runway unless tailwind exceeds a limit |
//! | [`prefer_unless_crosswind`] | use a fixed preferred runway unless crosswind exceeds a limit |
//! | [`min_crosswind`] | pick the runway with the smallest crosswind |
//! | [`within_crosswind_limit`] | filter runways by a crosswind ceiling |

use crate::RunwayInfo;

/// Return the runway with the greatest headwind from `runways`.
///
/// `advantage_threshold_kt` controls how decisive the winner must be:
/// the leader must beat every other runway by **strictly more than** this
/// value.  Use `0` to always pick the leader when there is any headwind
/// advantage at all.
///
/// Returns `None` when:
/// - no runway has METAR wind data, or
/// - there is only one candidate and `advantage_threshold_kt < 0` (unusual),
///   or
/// - the leader's advantage over the runner-up is ≤ `advantage_threshold_kt`.
///
/// # Example
/// ```
/// use runway_plugin_api::{RunwayInfo, helpers::best_headwind};
///
/// let runways = vec![
///     RunwayInfo { identifier: "18".into(), heading: 180,
///                  headwind_kt: Some(8), tailwind_kt: Some(0),
///                  crosswind_kt: Some(2), crosswind_direction: None },
///     RunwayInfo { identifier: "36".into(), heading: 360,
///                  headwind_kt: Some(-8), tailwind_kt: Some(8),
///                  crosswind_kt: Some(2), crosswind_direction: None },
/// ];
/// // 8 − (−8) = 16 > 2  →  "18" wins
/// assert_eq!(best_headwind(&runways, 2).map(|r| r.identifier.as_str()), Some("18"));
/// ```
pub fn best_headwind(runways: &[RunwayInfo], advantage_threshold_kt: i32) -> Option<&RunwayInfo> {
    let mut candidates: Vec<(&RunwayInfo, i32)> = runways
        .iter()
        .filter_map(|r| r.headwind_kt.map(|hw| (r, hw)))
        .collect();

    if candidates.is_empty() {
        return None;
    }

    candidates.sort_by_key(|a| std::cmp::Reverse(a.1));
    let (best_rwy, best_hw) = candidates[0];

    // Single candidate: it is the only option regardless of threshold.
    if candidates.len() == 1 {
        return Some(best_rwy);
    }

    let runner_up_hw = candidates[1].1;
    if best_hw - runner_up_hw > advantage_threshold_kt {
        Some(best_rwy)
    } else {
        None
    }
}

/// Use `preferred_id` unless its tailwind component exceeds `max_tailwind_kt`.
///
/// When the tailwind on the preferred runway is within the limit (or wind
/// data is absent), the preferred runway is returned unchanged.  When the
/// tailwind is over the limit, the function falls back to the runway with
/// the best headwind in `runways` (ties broken by order); if no other runway
/// has wind data the preferred runway is still returned.
///
/// Returns `None` only when `preferred_id` is not found in `runways`.
///
/// # Example
/// ```
/// use runway_plugin_api::{RunwayInfo, helpers::prefer_unless_tailwind};
///
/// let runways = vec![
///     RunwayInfo { identifier: "18".into(), heading: 180,
///                  headwind_kt: Some(-6), tailwind_kt: Some(6),
///                  crosswind_kt: Some(1), crosswind_direction: None },
///     RunwayInfo { identifier: "36".into(), heading: 360,
///                  headwind_kt: Some(6), tailwind_kt: Some(0),
///                  crosswind_kt: Some(1), crosswind_direction: None },
/// ];
/// // Tailwind on "18" is 6 kt > 5 kt limit → switch to "36"
/// assert_eq!(
///     prefer_unless_tailwind(&runways, "18", 5).map(|r| r.identifier.as_str()),
///     Some("36"),
/// );
/// ```
pub fn prefer_unless_tailwind<'a>(
    runways: &'a [RunwayInfo],
    preferred_id: &str,
    max_tailwind_kt: i32,
) -> Option<&'a RunwayInfo> {
    let preferred = runways.iter().find(|r| r.identifier == preferred_id)?;

    let tailwind = preferred.tailwind_kt.unwrap_or(0);
    if tailwind <= max_tailwind_kt {
        return Some(preferred);
    }

    // Tailwind limit exceeded: switch to the runway with the best headwind.
    // Fall back to the preferred runway if no alternative has wind data.
    Some(best_headwind(runways, 0).unwrap_or(preferred))
}

/// Use `preferred_id` unless its crosswind component exceeds `max_crosswind_kt`.
///
/// When the crosswind on the preferred runway is within the limit (or wind
/// data is absent), the preferred runway is returned unchanged.  When the
/// limit is exceeded, the function returns the runway with the smallest
/// crosswind from `runways`; if that runway happens to be the preferred one
/// again (i.e., all options are over the limit), the preferred runway is
/// still returned.
///
/// Returns `None` only when `preferred_id` is not found in `runways`.
pub fn prefer_unless_crosswind<'a>(
    runways: &'a [RunwayInfo],
    preferred_id: &str,
    max_crosswind_kt: i32,
) -> Option<&'a RunwayInfo> {
    let preferred = runways.iter().find(|r| r.identifier == preferred_id)?;

    let crosswind = preferred.crosswind_kt.unwrap_or(0);
    if crosswind <= max_crosswind_kt {
        return Some(preferred);
    }

    Some(min_crosswind(runways).unwrap_or(preferred))
}

/// Return the runway with the smallest crosswind component.
///
/// When multiple runways tie, the first in slice order is returned.
/// Returns `None` only when no runway has wind data.
///
/// # Example
/// ```
/// use runway_plugin_api::{RunwayInfo, helpers::min_crosswind};
///
/// let runways = vec![
///     RunwayInfo { identifier: "18".into(), heading: 180,
///                  headwind_kt: Some(0), tailwind_kt: Some(0),
///                  crosswind_kt: Some(18), crosswind_direction: None },
///     RunwayInfo { identifier: "28".into(), heading: 280,
///                  headwind_kt: Some(4), tailwind_kt: Some(0),
///                  crosswind_kt: Some(4), crosswind_direction: None },
/// ];
/// assert_eq!(min_crosswind(&runways).map(|r| r.identifier.as_str()), Some("28"));
/// ```
pub fn min_crosswind(runways: &[RunwayInfo]) -> Option<&RunwayInfo> {
    runways
        .iter()
        .filter(|r| r.crosswind_kt.is_some())
        .min_by_key(|r| r.crosswind_kt.unwrap())
}

/// Return all runways whose crosswind component is at most `max_kt`.
///
/// Runways without wind data are **included** in the result — their crosswind
/// is unknown, so they are not filtered out.  If you need to exclude
/// no-data entries, filter them yourself first.
///
/// Returns an empty `Vec` only when every runway with known wind data
/// exceeds `max_kt`.
pub fn within_crosswind_limit(runways: &[RunwayInfo], max_kt: i32) -> Vec<&RunwayInfo> {
    runways
        .iter()
        .filter(|r| r.crosswind_kt.is_none_or(|cw| cw <= max_kt))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rwy(
        id: &str,
        heading: u16,
        hw: Option<i32>,
        tw: Option<i32>,
        xw: Option<i32>,
    ) -> RunwayInfo {
        RunwayInfo {
            identifier: id.to_string(),
            heading,
            headwind_kt: hw,
            tailwind_kt: tw,
            crosswind_kt: xw,
            crosswind_direction: None,
        }
    }

    fn id(r: Option<&RunwayInfo>) -> Option<&str> {
        r.map(|r| r.identifier.as_str())
    }

    // ── best_headwind ─────────────────────────────────────────────────────────

    #[test]
    fn best_headwind_clear_winner() {
        let rwys = [
            rwy("18", 180, Some(10), Some(0), Some(1)),
            rwy("36", 360, Some(-10), Some(10), Some(1)),
        ];
        assert_eq!(id(best_headwind(&rwys, 2)), Some("18"));
    }

    #[test]
    fn best_headwind_no_clear_winner_within_threshold() {
        let rwys = [
            rwy("18", 180, Some(0), Some(0), Some(0)),
            rwy("36", 360, Some(0), Some(0), Some(0)),
        ];
        assert_eq!(id(best_headwind(&rwys, 2)), None);
    }

    #[test]
    fn best_headwind_no_metar() {
        let rwys = [
            rwy("18", 180, None, None, None),
            rwy("36", 360, None, None, None),
        ];
        assert_eq!(id(best_headwind(&rwys, 2)), None);
    }

    #[test]
    fn best_headwind_single_with_data() {
        let rwys = [rwy("18", 180, Some(5), Some(0), Some(0))];
        assert_eq!(id(best_headwind(&rwys, 2)), Some("18"));
    }

    #[test]
    fn best_headwind_threshold_zero_picks_any_advantage() {
        let rwys = [
            rwy("18", 180, Some(3), Some(0), Some(0)),
            rwy("36", 360, Some(2), Some(0), Some(0)),
        ];
        assert_eq!(id(best_headwind(&rwys, 0)), Some("18"));
    }

    // ── prefer_unless_tailwind ────────────────────────────────────────────────

    #[test]
    fn prefer_unless_tailwind_within_limit() {
        let rwys = [
            rwy("18", 180, Some(8), Some(0), Some(1)),
            rwy("36", 360, Some(-8), Some(8), Some(1)),
        ];
        assert_eq!(id(prefer_unless_tailwind(&rwys, "18", 5)), Some("18"));
    }

    #[test]
    fn prefer_unless_tailwind_exceeds_limit() {
        let rwys = [
            rwy("18", 180, Some(-6), Some(6), Some(1)),
            rwy("36", 360, Some(6), Some(0), Some(1)),
        ];
        assert_eq!(id(prefer_unless_tailwind(&rwys, "18", 5)), Some("36"));
    }

    #[test]
    fn prefer_unless_tailwind_no_metar_stays_preferred() {
        let rwys = [
            rwy("18", 180, None, None, None),
            rwy("36", 360, None, None, None),
        ];
        assert_eq!(id(prefer_unless_tailwind(&rwys, "18", 5)), Some("18"));
    }

    #[test]
    fn prefer_unless_tailwind_unknown_preferred_id() {
        let rwys = [rwy("18", 180, Some(5), Some(0), Some(0))];
        assert_eq!(id(prefer_unless_tailwind(&rwys, "99", 5)), None);
    }

    // ── prefer_unless_crosswind ───────────────────────────────────────────────

    #[test]
    fn prefer_unless_crosswind_within_limit() {
        let rwys = [
            rwy("18", 180, Some(0), Some(0), Some(10)),
            rwy("28", 280, Some(4), Some(0), Some(4)),
        ];
        assert_eq!(id(prefer_unless_crosswind(&rwys, "18", 15)), Some("18"));
    }

    #[test]
    fn prefer_unless_crosswind_exceeds_limit() {
        let rwys = [
            rwy("18", 180, Some(0), Some(0), Some(20)),
            rwy("28", 280, Some(4), Some(0), Some(4)),
        ];
        assert_eq!(id(prefer_unless_crosswind(&rwys, "18", 15)), Some("28"));
    }

    // ── min_crosswind ─────────────────────────────────────────────────────────

    #[test]
    fn min_crosswind_picks_smallest() {
        let rwys = [
            rwy("18", 180, Some(0), Some(0), Some(18)),
            rwy("28", 280, Some(4), Some(0), Some(4)),
        ];
        assert_eq!(id(min_crosswind(&rwys)), Some("28"));
    }

    #[test]
    fn min_crosswind_no_data_returns_none() {
        let rwys = [rwy("18", 180, None, None, None)];
        assert_eq!(id(min_crosswind(&rwys)), None);
    }

    // ── within_crosswind_limit ────────────────────────────────────────────────

    #[test]
    fn within_crosswind_limit_filters_correctly() {
        let rwys = [
            rwy("18", 180, Some(0), Some(0), Some(5)),
            rwy("36", 360, Some(0), Some(0), Some(20)),
            rwy("28", 280, Some(4), Some(0), None), // no data → included
        ];
        let ok = within_crosswind_limit(&rwys, 10);
        assert_eq!(ok.len(), 2);
        assert_eq!(ok[0].identifier, "18");
        assert_eq!(ok[1].identifier, "28");
    }
}
