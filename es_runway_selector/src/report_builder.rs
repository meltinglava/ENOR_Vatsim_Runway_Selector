use std::io::{self, Write};

use askama::Template;
use indexmap::IndexMap;
use itertools::Itertools;
use runway_selector_protocol::SelectionTag;

use crate::{
    airport::{Airport, CrosswindDirection, RunwayInUseSource, RunwayWindComponents},
    runway::{RunwayDirection, RunwayUse},
};

pub(crate) type WindColumnParts = (String, String, String, String, String);

type AirportGroupData =
    IndexMap<Option<RunwayInUseSource>, Vec<(String, IndexMap<String, RunwayUse>)>>;

const CALM_THRESHOLD: i32 = 1;
const CALM_SYMBOL: &str = "○";
const HEADWIND_ARROW: &str = "↓";
const TAILWIND_ARROW: &str = "↑";
const CROSSWIND_FROM_LEFT_ARROW: &str = "→";
const CROSSWIND_FROM_RIGHT_ARROW: &str = "←";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LongitudinalWindDisplay {
    Calm,
    Headwind(i32),
    Tailwind(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CrosswindDisplay {
    Calm,
    FromLeft(i32),
    FromRight(i32),
    Variable(i32),
}

// ─── View types ───────────────────────────────────────────────────────────────

#[derive(Debug)]
pub(crate) struct RunwayReportView {
    pub groups: Vec<RunwaySourceGroupView>,
}

#[derive(Debug)]
pub(crate) struct RunwaySourceGroupView {
    pub source_label: String,
    pub source_class: String,
    pub airports: Vec<AirportRunwayView>,
}

#[derive(Debug)]
pub(crate) struct AirportRunwayView {
    pub icao: String,
    pub line_count: usize,
    pub lines: Vec<AirportRunwayLineView>,
    pub tags: Vec<SelectionTag>,
    pub metar: String,
}

#[derive(Debug)]
pub(crate) struct AirportRunwayLineView {
    pub runway_text: String,
    pub wind_head_arrow_text: String,
    pub wind_head_value_text: String,
    pub wind_cross_left_arrow_text: String,
    pub wind_cross_value_text: String,
    pub wind_cross_right_arrow_text: String,
}

#[derive(Template)]
#[template(path = "runway_report.html")]
struct RunwayReportTemplate<'a> {
    groups: &'a [RunwaySourceGroupView],
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Build the full report view from airport data.
pub(crate) fn build_report(airports: &IndexMap<String, Airport>) -> RunwayReportView {
    let data = group_airports(airports);
    build_view(airports, &data)
}

/// Render the report view to an HTML string.
pub(crate) fn render_html(view: &RunwayReportView) -> io::Result<String> {
    RunwayReportTemplate {
        groups: &view.groups,
    }
    .render()
    .map_err(io::Error::other)
}

/// Write the HTML report to a temp file and open it in the browser.
pub(crate) fn open_html_report(airports: &IndexMap<String, Airport>) -> io::Result<()> {
    let mut file = tempfile::Builder::new()
        .prefix("runways_")
        .suffix(".html")
        .rand_bytes(5)
        .tempfile()?;
    let html = render_html(&build_report(airports))?;
    file.write_all(html.as_bytes())?;
    open::that_detached(file.path())?;
    file.keep()?;
    Ok(())
}

// ─── Grouping / ordering ──────────────────────────────────────────────────────

fn group_airports(airports: &IndexMap<String, Airport>) -> AirportGroupData {
    let mut data = AirportGroupData::new();

    for airport in airports.values() {
        let preferred = RunwayInUseSource::default_sort_order()
            .into_iter()
            .find_map(|src| {
                airport
                    .runways_in_use
                    .get(&src)
                    .map(|sel| (src, sel.clone()))
            });

        match preferred {
            Some((src, sel)) => data
                .entry(Some(src))
                .or_default()
                .push((airport.icao.clone(), sel)),
            None => data
                .entry(None)
                .or_default()
                .push((airport.icao.clone(), IndexMap::new())),
        }
    }

    data.sort_unstable_by(|k1, _, k2, _| match (k1, k2) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (Some(a), Some(b)) => a.cmp(b),
    });
    data
}

fn build_view(airports: &IndexMap<String, Airport>, data: &AirportGroupData) -> RunwayReportView {
    let groups = data
        .iter()
        .map(|(source, configs)| {
            let airport_views = configs
                .iter()
                .map(|(icao, runways)| {
                    let airport = airports.get(icao);
                    let lines = build_report_lines_for_row(airport, runways);
                    let tags = source
                        .as_ref()
                        .and_then(|src| airport.and_then(|a| a.selection_tags.get(src).cloned()))
                        .unwrap_or_default();
                    AirportRunwayView {
                        icao: icao.clone(),
                        line_count: lines.len(),
                        lines,
                        tags,
                        metar: metar_text(airport, icao),
                    }
                })
                .collect();
            RunwaySourceGroupView {
                source_label: source_label(source.as_ref()).to_string(),
                source_class: source_class(source.as_ref()).to_string(),
                airports: airport_views,
            }
        })
        .collect();
    RunwayReportView { groups }
}

// ─── Display helpers ──────────────────────────────────────────────────────────

fn source_label(source: Option<&RunwayInUseSource>) -> &'static str {
    match source {
        Some(RunwayInUseSource::Atis) => "ATIS",
        Some(RunwayInUseSource::Metar) => "METAR",
        Some(RunwayInUseSource::Default) => "fallback",
        None => "No runway config",
    }
}

fn source_class(source: Option<&RunwayInUseSource>) -> &'static str {
    match source {
        Some(_) => "",
        None => "none",
    }
}

fn metar_text(airport: Option<&Airport>, icao: &str) -> String {
    airport
        .and_then(|a| a.metar.as_ref().map(|m| m.raw.clone()))
        .unwrap_or_else(|| format!("{icao} No METAR"))
}

fn runway_direction_for_identifier<'a>(
    airport: &'a Airport,
    runway_identifier: &str,
) -> Option<&'a RunwayDirection> {
    airport
        .runways
        .iter()
        .flat_map(|runway| runway.iter())
        .find(|dir| {
            dir.identifier == runway_identifier
                || (runway_identifier.len() == 2 && dir.identifier.starts_with(runway_identifier))
        })
}

fn selected_runways_are_parallel(airport: &Airport, runways: &IndexMap<String, RunwayUse>) -> bool {
    let directions: Vec<_> = runways
        .keys()
        .filter_map(|id| runway_direction_for_identifier(airport, id))
        .collect();

    directions.len() == runways.len()
        && directions
            .iter()
            .tuple_combinations()
            .all(|(a, b)| a.degrees % 180 == b.degrees % 180)
}

fn should_split_runway_lines(airport: &Airport, runways: &IndexMap<String, RunwayUse>) -> bool {
    runways.len() > 1 && !selected_runways_are_parallel(airport, runways)
}

fn format_runway_usage(runways: &IndexMap<String, RunwayUse>) -> Option<String> {
    if runways.is_empty() {
        None
    } else {
        Some(
            runways
                .iter()
                .map(|(rwy, use_)| format!("{rwy}{}", use_.report_suffix()))
                .join(" + "),
        )
    }
}

pub(crate) fn format_runway_usage_for_selection(
    airport: &Airport,
    runways: &IndexMap<String, RunwayUse>,
) -> String {
    if runways.is_empty() {
        return "(no selection)".to_string();
    }
    let parts: Vec<_> = runways
        .iter()
        .map(|(rwy, use_)| format!("{rwy}{}", use_.report_suffix()))
        .collect();
    if should_split_runway_lines(airport, runways) {
        parts.join("\n")
    } else {
        parts.join(" + ")
    }
}

fn wind_display_parts(c: &RunwayWindComponents) -> (LongitudinalWindDisplay, CrosswindDisplay) {
    let longitudinal = if c.headwind > CALM_THRESHOLD {
        LongitudinalWindDisplay::Headwind(c.headwind)
    } else if c.headwind < -CALM_THRESHOLD {
        LongitudinalWindDisplay::Tailwind(c.headwind.abs())
    } else {
        LongitudinalWindDisplay::Calm
    };

    let crosswind = if c.crosswind <= CALM_THRESHOLD {
        CrosswindDisplay::Calm
    } else {
        match c.crosswind_direction {
            CrosswindDirection::Left => CrosswindDisplay::FromLeft(c.crosswind),
            CrosswindDirection::Right => CrosswindDisplay::FromRight(c.crosswind),
            CrosswindDirection::Variable => CrosswindDisplay::Variable(c.crosswind),
        }
    };

    (longitudinal, crosswind)
}

pub(crate) fn format_wind_columns(c: &RunwayWindComponents) -> WindColumnParts {
    let (longitudinal, crosswind) = wind_display_parts(c);

    let (head_arrow, head_value) = match longitudinal {
        LongitudinalWindDisplay::Calm => (CALM_SYMBOL.to_string(), String::new()),
        LongitudinalWindDisplay::Headwind(v) => (HEADWIND_ARROW.to_string(), v.to_string()),
        LongitudinalWindDisplay::Tailwind(v) => (TAILWIND_ARROW.to_string(), v.to_string()),
    };

    let (cross_left, cross_value, cross_right) = match crosswind {
        CrosswindDisplay::Calm => (String::new(), CALM_SYMBOL.to_string(), String::new()),
        CrosswindDisplay::FromLeft(v) => (
            CROSSWIND_FROM_LEFT_ARROW.to_string(),
            v.to_string(),
            String::new(),
        ),
        CrosswindDisplay::FromRight(v) => (
            String::new(),
            v.to_string(),
            CROSSWIND_FROM_RIGHT_ARROW.to_string(),
        ),
        CrosswindDisplay::Variable(v) => (
            CROSSWIND_FROM_LEFT_ARROW.to_string(),
            v.to_string(),
            CROSSWIND_FROM_RIGHT_ARROW.to_string(),
        ),
    };

    (head_arrow, head_value, cross_left, cross_value, cross_right)
}

pub(crate) fn format_wind_component_columns_for_selection(
    airport: &Airport,
    runways: &IndexMap<String, RunwayUse>,
) -> WindColumnParts {
    if runways.is_empty() {
        return empty_wind_columns();
    }

    let split_lines = should_split_runway_lines(airport, runways);
    let mut values: Vec<WindColumnParts> = runways
        .keys()
        .map(|rwy| {
            runway_direction_for_identifier(airport, rwy)
                .and_then(|dir| airport.runway_wind_components(dir))
                .map_or_else(
                    || {
                        (
                            String::new(),
                            "n/a".to_string(),
                            String::new(),
                            "n/a".to_string(),
                            String::new(),
                        )
                    },
                    |c| format_wind_columns(&c),
                )
        })
        .collect();

    if !split_lines {
        values = values.into_iter().unique().collect();
    }

    let sep = if split_lines { "\n" } else { "  +  " };
    let (mut ha, mut hv, mut cla, mut cv, mut cra) = (
        Vec::with_capacity(values.len()),
        Vec::with_capacity(values.len()),
        Vec::with_capacity(values.len()),
        Vec::with_capacity(values.len()),
        Vec::with_capacity(values.len()),
    );
    for (a, b, c, d, e) in values {
        ha.push(a);
        hv.push(b);
        cla.push(c);
        cv.push(d);
        cra.push(e);
    }
    (
        ha.join(sep),
        hv.join(sep),
        cla.join(sep),
        cv.join(sep),
        cra.join(sep),
    )
}

fn format_wind_component_columns_for_row(
    airport: Option<&Airport>,
    runways: &IndexMap<String, RunwayUse>,
) -> WindColumnParts {
    if runways.is_empty() {
        return empty_wind_columns();
    }
    match airport {
        Some(ap) => format_wind_component_columns_for_selection(ap, runways),
        None => missing_airport_wind_columns(),
    }
}

fn empty_wind_columns() -> WindColumnParts {
    (
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    )
}

fn missing_airport_wind_columns() -> WindColumnParts {
    let na = "n/a".to_string();
    (na.clone(), na.clone(), na.clone(), na.clone(), na)
}

fn split_lines(value: &str) -> Vec<String> {
    value.split('\n').map(ToOwned::to_owned).collect()
}

fn build_report_lines_for_row(
    airport: Option<&Airport>,
    runways: &IndexMap<String, RunwayUse>,
) -> Vec<AirportRunwayLineView> {
    let runway_text = if runways.is_empty() {
        "(no selection)".to_string()
    } else {
        match airport {
            Some(ap) => format_runway_usage_for_selection(ap, runways),
            None => format_runway_usage(runways).unwrap_or_default(),
        }
    };

    let wind = format_wind_component_columns_for_row(airport, runways);

    let rwy_lines = split_lines(&runway_text);
    let ha_lines = split_lines(&wind.0);
    let hv_lines = split_lines(&wind.1);
    let cla_lines = split_lines(&wind.2);
    let cv_lines = split_lines(&wind.3);
    let cra_lines = split_lines(&wind.4);

    let line_count = [
        rwy_lines.len(),
        ha_lines.len(),
        hv_lines.len(),
        cla_lines.len(),
        cv_lines.len(),
        cra_lines.len(),
    ]
    .into_iter()
    .max()
    .unwrap_or(1);

    (0..line_count)
        .map(|i| AirportRunwayLineView {
            runway_text: rwy_lines.get(i).cloned().unwrap_or_default(),
            wind_head_arrow_text: ha_lines.get(i).cloned().unwrap_or_default(),
            wind_head_value_text: hv_lines.get(i).cloned().unwrap_or_default(),
            wind_cross_left_arrow_text: cla_lines.get(i).cloned().unwrap_or_default(),
            wind_cross_value_text: cv_lines.get(i).cloned().unwrap_or_default(),
            wind_cross_right_arrow_text: cra_lines.get(i).cloned().unwrap_or_default(),
        })
        .collect()
}
