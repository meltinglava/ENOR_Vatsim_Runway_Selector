use askama::Template;
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use tracing::warn;
use tracing_unwrap::ResultExt;

use std::{
    io::{self, Read, Write},
    ops::{Index, IndexMut},
};

use crate::{
    airport::{Airport, CrosswindDirection, RunwayInUseSource, RunwayWindComponents},
    atis_parser::find_runway_in_use_from_atis,
    config::ESConfig,
    error::ApplicationResult,
    metar::get_metars,
    runway::{RunwayDirection, RunwayUse},
    sector_file::load_airports_from_sct_runway_section,
};

pub struct Airports {
    pub airports: IndexMap<String, Airport>,
}

type AirportsConfigReportData =
    IndexMap<Option<RunwayInUseSource>, Vec<(String, IndexMap<String, RunwayUse>)>>;
type WindColumnParts = (String, String, String, String, String);

impl Airports {
    pub fn new() -> Self {
        Self {
            airports: IndexMap::new(),
        }
    }

    #[allow(dead_code)] // used in tests
    pub fn add_airport(&mut self, airport: Airport) {
        self.airports.insert(airport.icao.clone(), airport);
    }

    pub fn load_airports_from_sector_file<R: Read>(
        &mut self,
        reader: &mut R,
        config: &ESConfig,
    ) -> ApplicationResult<()> {
        let parsed_airports =
            load_airports_from_sct_runway_section(reader, config.get_ignore_airports())?;

        for (icao, mut airport) in parsed_airports {
            match self.airports.entry(icao) {
                indexmap::map::Entry::Occupied(mut existing) => {
                    existing.get_mut().runways.append(&mut airport.runways);
                }
                indexmap::map::Entry::Vacant(vacant) => {
                    vacant.insert(airport);
                }
            }
        }
        Ok(())
    }

    pub async fn add_metars(&mut self, conf: &ESConfig) {
        let metars = get_metars(conf).await.unwrap_or_log();
        for metar in metars {
            if let Some(airport) = self.airports.get_mut(&metar.icao) {
                airport.metar = Some(metar);
            }
        }
    }

    pub async fn read_atis_and_apply_runways(&mut self) -> ApplicationResult<()> {
        let icaos = self.identifiers();
        let v3_data = vatsim_utils::live_api::Vatsim::new()
            .await?
            .get_v3_data()
            .await?;
        let atis_entries = v3_data.atis;
        for atis in atis_entries {
            let icao = &atis.callsign[0..4];
            if !icaos.contains(icao) {
                continue;
            }
            let Some(atis_lines) = atis.text_atis else {
                continue;
            };
            let Some(airport) = self.airports.get_mut(icao) else {
                continue;
            };
            let text = atis_lines.into_iter().collect::<Vec<_>>().join(" ");
            for (runway, runway_use) in find_runway_in_use_from_atis(&text) {
                airport
                    .runways_in_use
                    .entry(RunwayInUseSource::Atis)
                    .or_default()
                    .entry(runway)
                    .and_modify(|existing| {
                        *existing = existing.merged_with(runway_use);
                    })
                    .or_insert(runway_use);
            }
        }

        Ok(())
    }

    pub fn select_runway_in_use(&mut self, config: &ESConfig) {
        self.runway_in_use_based_on_metar(config);
        self.apply_default_runways(config);
    }

    fn runway_in_use_based_on_metar(&mut self, config: &ESConfig) {
        for airport in self.airports.values_mut() {
            if airport.icao == "ENGM" {
                let (source, runway_in_use) = airport.set_runway_for_engm(config).unwrap_or_log();
                airport.runways_in_use.insert(source, runway_in_use);
            } else if let Ok(runways_in_use) = airport.set_runway_based_on_metar_wind()
                && !runways_in_use.is_empty()
            {
                airport
                    .runways_in_use
                    .insert(RunwayInUseSource::Metar, runways_in_use);
            }
        }
    }

    fn apply_default_runways(&mut self, config: &ESConfig) {
        let defaults = config.get_default_runways();
        for airport in self.airports.values_mut() {
            let default_entry = airport.runways_in_use.entry(RunwayInUseSource::Default);
            match default_entry {
                indexmap::map::Entry::Occupied(_) => continue,
                indexmap::map::Entry::Vacant(v) => {
                    if let Some(runway) = defaults.get(airport.icao.as_str()) {
                        let identifier = format!("{runway:02}");
                        if airport.runways.iter().any(|rw| {
                            rw.runways
                                .iter()
                                .any(|dir| dir.identifier[0..2] == identifier)
                        }) {
                            v.insert([(identifier, RunwayUse::Both)].into());
                        } else {
                            warn!(airport.icao, default_runway = %runway, "Default runway not found in airport runways");
                        }
                    }
                }
            };
        }
    }

    pub fn airports_without_runway_config(&self) -> Vec<&Airport> {
        self.airports
            .values()
            .filter(|a| a.runways_in_use.is_empty())
            .collect()
    }

    pub fn identifiers(&self) -> IndexSet<String> {
        self.airports.keys().cloned().collect()
    }

    pub fn sort(&mut self) {
        self.airports.sort_unstable_keys();
    }

    fn grouped_runway_config_report_data(&self) -> AirportsConfigReportData {
        let mut data = AirportsConfigReportData::new();

        for airport in self.airports.values() {
            let preferred_selection = RunwayInUseSource::default_sort_order()
                .into_iter()
                .find_map(|selection_source| {
                    airport
                        .runways_in_use
                        .get(&selection_source)
                        .map(|selection| (selection_source, selection.clone()))
                });

            match preferred_selection {
                Some((selection_source, runway_selection)) => {
                    data.entry(Some(selection_source))
                        .or_default()
                        .push((airport.icao.clone(), runway_selection));
                }
                None => {
                    data.entry(None)
                        .or_default()
                        .push((airport.icao.clone(), IndexMap::new()));
                }
            }
        }

        Self::sort_runway_config_report_data(&mut data);
        data
    }

    fn sort_runway_config_report_data(data: &mut AirportsConfigReportData) {
        data.sort_unstable_by(|k1, _v1, k2, _v2| match (k1, k2) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (Some(k1), Some(k2)) => k1.cmp(k2),
        });
    }

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

    fn format_runway_usage(runways: &IndexMap<String, RunwayUse>) -> Option<String> {
        if runways.is_empty() {
            None
        } else {
            Some(
                runways
                    .iter()
                    .map(|(runway, usage)| format!("{runway}{}", usage.report_suffix()))
                    .join(" + "),
            )
        }
    }

    fn should_split_runway_lines(airport: &Airport, runways: &IndexMap<String, RunwayUse>) -> bool {
        runways.len() > 1 && !Self::selected_runways_are_parallel(airport, runways)
    }

    fn format_runway_usage_for_selection(
        airport: &Airport,
        runways: &IndexMap<String, RunwayUse>,
    ) -> String {
        if runways.is_empty() {
            return "(no selection)".to_string();
        }

        let parts = runways
            .iter()
            .map(|(runway, usage)| format!("{runway}{}", usage.report_suffix()))
            .collect_vec();

        if Self::should_split_runway_lines(airport, runways) {
            parts.join("\n")
        } else {
            parts.join(" + ")
        }
    }

    fn metar_text_for_airport(&self, icao: &str) -> String {
        self.airports
            .get(icao)
            .and_then(|airport| airport.metar.as_ref().map(|metar| metar.raw.clone()))
            .unwrap_or_else(|| format!("{icao} No METAR"))
    }

    fn runway_direction_for_identifier<'a>(
        airport: &'a Airport,
        runway_identifier: &str,
    ) -> Option<&'a RunwayDirection> {
        airport
            .runways
            .iter()
            .flat_map(|runway| runway.runways.iter())
            .find(|direction| {
                direction.identifier == runway_identifier
                    || (runway_identifier.len() == 2
                        && direction.identifier.starts_with(runway_identifier))
            })
    }

    fn format_wind_components_for_selection(
        airport: &Airport,
        runways: &IndexMap<String, RunwayUse>,
    ) -> String {
        if runways.is_empty() {
            return String::new();
        }

        let show_runway_identifier = Self::should_split_runway_lines(airport, runways);
        let mut values = runways
            .keys()
            .map(|runway| {
                let components = Self::runway_direction_for_identifier(airport, runway)
                    .and_then(|direction| airport.runway_wind_components(direction));

                match components {
                    Some(components) => {
                        let component_text = Self::format_wind_components(components);
                        if show_runway_identifier {
                            format!("{runway:<3} {component_text}")
                        } else {
                            component_text
                        }
                    }
                    None => {
                        if show_runway_identifier {
                            format!("{runway:<3} n/a    ")
                        } else {
                            "n/a".to_string()
                        }
                    }
                }
            })
            .collect_vec();

        if !show_runway_identifier {
            values = values.into_iter().unique().collect_vec();
        }

        if show_runway_identifier {
            values.join("\n")
        } else {
            values.join("  +  ")
        }
    }

    fn format_wind_component_columns_for_selection(
        airport: &Airport,
        runways: &IndexMap<String, RunwayUse>,
    ) -> WindColumnParts {
        if runways.is_empty() {
            return (
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            );
        }

        let split_lines = Self::should_split_runway_lines(airport, runways);
        let mut values = runways
            .keys()
            .map(|runway| {
                let components = Self::runway_direction_for_identifier(airport, runway)
                    .and_then(|direction| airport.runway_wind_components(direction));

                match components {
                    Some(components) => Self::format_wind_columns(&components),
                    None => (
                        String::new(),
                        "n/a".to_string(),
                        String::new(),
                        "n/a".to_string(),
                        String::new(),
                    ),
                }
            })
            .collect_vec();

        if !split_lines {
            values = values.into_iter().unique().collect_vec();
        }

        let separator = if split_lines { "\n" } else { "  +  " };
        let mut head_arrow = Vec::with_capacity(values.len());
        let mut head_value = Vec::with_capacity(values.len());
        let mut cross_left_arrow = Vec::with_capacity(values.len());
        let mut cross_value = Vec::with_capacity(values.len());
        let mut cross_right_arrow = Vec::with_capacity(values.len());

        for (ha, hv, cla, cv, cra) in values {
            head_arrow.push(ha);
            head_value.push(hv);
            cross_left_arrow.push(cla);
            cross_value.push(cv);
            cross_right_arrow.push(cra);
        }

        (
            head_arrow.join(separator),
            head_value.join(separator),
            cross_left_arrow.join(separator),
            cross_value.join(separator),
            cross_right_arrow.join(separator),
        )
    }

    fn selected_runways_are_parallel(
        airport: &Airport,
        runways: &IndexMap<String, RunwayUse>,
    ) -> bool {
        let directions = runways
            .keys()
            .filter_map(|runway_identifier| {
                Self::runway_direction_for_identifier(airport, runway_identifier)
            })
            .collect_vec();

        if directions.len() != runways.len() {
            return false;
        }

        directions
            .iter()
            .tuple_combinations()
            .all(|(left, right)| left.degrees % 180 == right.degrees % 180)
    }

    fn format_wind_components(components: RunwayWindComponents) -> String {
        const CALM_THRESHOLD: i32 = 1;
        const CALM: &str = "○";
        const HEADWIND: &str = "↑";
        const TAILWIND: &str = "↓";
        const INWARD_FROM_LEFT: &str = "→";
        const INWARD_FROM_RIGHT: &str = "←";
        const INWARD_FROM_BOTH_LEFT: &str = "→";
        const INWARD_FROM_BOTH_RIGHT: &str = "←";

        let longitudinal = if components.headwind > CALM_THRESHOLD {
            format!("{HEADWIND}{:>2}", components.headwind)
        } else if components.headwind < -CALM_THRESHOLD {
            format!("{TAILWIND}{:>2}", components.headwind.abs())
        } else {
            format!("{CALM}  ")
        };

        let cross = if components.crosswind > CALM_THRESHOLD {
            match components.crosswind_direction {
                CrosswindDirection::Left => {
                    format!("{INWARD_FROM_LEFT}{:>2} ", components.crosswind)
                }
                CrosswindDirection::Right => {
                    format!(" {:>2}{INWARD_FROM_RIGHT}", components.crosswind)
                }
                CrosswindDirection::Variable => {
                    format!(
                        "{INWARD_FROM_BOTH_LEFT}{:>2}{INWARD_FROM_BOTH_RIGHT}",
                        components.crosswind
                    )
                }
            }
        } else {
            format!(" {CALM}  ")
        };

        format!("{longitudinal} {cross}")
    }

    fn format_wind_columns(components: &RunwayWindComponents) -> WindColumnParts {
        const CALM_THRESHOLD: i32 = 1;
        const CALM: &str = "○";
        const HEADWIND: &str = "↑";
        const TAILWIND: &str = "↓";
        const INWARD_FROM_LEFT: &str = "→";
        const INWARD_FROM_RIGHT: &str = "←";

        let (head_arrow, head_value) = if components.headwind > CALM_THRESHOLD {
            (HEADWIND.to_string(), components.headwind.to_string())
        } else if components.headwind < -CALM_THRESHOLD {
            (TAILWIND.to_string(), components.headwind.abs().to_string())
        } else {
            (CALM.to_string(), String::new())
        };

        let (cross_left_arrow, cross_value, cross_right_arrow) =
            if components.crosswind <= CALM_THRESHOLD {
                (String::new(), CALM.to_string(), String::new())
            } else {
                match components.crosswind_direction {
                    CrosswindDirection::Left => (
                        INWARD_FROM_LEFT.to_string(),
                        components.crosswind.to_string(),
                        String::new(),
                    ),
                    CrosswindDirection::Right => (
                        String::new(),
                        components.crosswind.to_string(),
                        INWARD_FROM_RIGHT.to_string(),
                    ),
                    CrosswindDirection::Variable => (
                        INWARD_FROM_LEFT.to_string(),
                        components.crosswind.to_string(),
                        INWARD_FROM_RIGHT.to_string(),
                    ),
                }
            };

        (
            head_arrow,
            head_value,
            cross_left_arrow,
            cross_value,
            cross_right_arrow,
        )
    }

    fn missing_airport_wind_columns() -> WindColumnParts {
        (
            "n/a".to_string(),
            "n/a".to_string(),
            "n/a".to_string(),
            "n/a".to_string(),
            "n/a".to_string(),
        )
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

    fn format_wind_component_columns_for_row(
        airport: Option<&Airport>,
        runways: &IndexMap<String, RunwayUse>,
    ) -> WindColumnParts {
        if runways.is_empty() {
            Self::empty_wind_columns()
        } else {
            match airport {
                Some(airport) => {
                    Self::format_wind_component_columns_for_selection(airport, runways)
                }
                None => Self::missing_airport_wind_columns(),
            }
        }
    }

    pub fn make_runway_report_html(&self) -> io::Result<()> {
        let mut file = tempfile::Builder::new()
            .prefix("runways_")
            .suffix(".html")
            .rand_bytes(5)
            .tempfile()?;
        self.make_runway_report_html_with_writer(&mut file)?;
        open::that_detached(file.path())?;
        file.keep()?;
        Ok(())
    }

    fn make_runway_report_html_with_writer<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        let report_data = self.grouped_runway_config_report_data();

        // Convert to view model for the template
        let view = self.build_runway_report_view(&report_data);

        // Render template
        let tpl = RunwayReportTemplate {
            groups: &view.groups,
        };

        let html = tpl.render().map_err(io::Error::other)?;

        writer.write_all(html.as_bytes())
    }

    fn build_runway_report_view(&self, data: &AirportsConfigReportData) -> RunwayReportView {
        let mut groups = Vec::new();

        for (source, configs) in data {
            let label = Self::source_label(source.as_ref());
            let class = Self::source_class(source.as_ref());

            let mut airports = Vec::with_capacity(configs.len());

            for (icao, runways) in configs {
                let (
                    runway_text,
                    wind_text,
                    wind_head_arrow_text,
                    wind_head_value_text,
                    wind_cross_left_arrow_text,
                    wind_cross_value_text,
                    wind_cross_right_arrow_text,
                ) = if runways.is_empty() {
                    (
                        "(no selection)".to_string(),
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                    )
                } else {
                    let airport = self.airports.get(icao);
                    let (
                        wind_head_arrow_text,
                        wind_head_value_text,
                        wind_cross_left_arrow_text,
                        wind_cross_value_text,
                        wind_cross_right_arrow_text,
                    ) = Self::format_wind_component_columns_for_row(airport, runways);
                    let (runway_text, wind_text) = match airport {
                        Some(airport) => (
                            Self::format_runway_usage_for_selection(airport, runways),
                            Self::format_wind_components_for_selection(airport, runways),
                        ),
                        None => (
                            Self::format_runway_usage(runways).unwrap_or_default(),
                            "(missing airport)".to_string(),
                        ),
                    };
                    (
                        runway_text,
                        wind_text,
                        wind_head_arrow_text,
                        wind_head_value_text,
                        wind_cross_left_arrow_text,
                        wind_cross_value_text,
                        wind_cross_right_arrow_text,
                    )
                };
                let metar = self.metar_text_for_airport(icao);

                airports.push(AirportRunwayView {
                    icao: icao.clone(),
                    runway_text,
                    wind_text,
                    wind_head_arrow_text,
                    wind_head_value_text,
                    wind_cross_left_arrow_text,
                    wind_cross_value_text,
                    wind_cross_right_arrow_text,
                    metar,
                });
            }

            groups.push(RunwaySourceGroupView {
                source_label: label.to_string(),
                source_class: class.to_string(),
                airports,
            });
        }

        RunwayReportView { groups }
    }
}

impl Index<&str> for Airports {
    type Output = Airport;

    fn index(&self, index: &str) -> &Self::Output {
        &self.airports[index]
    }
}

impl IndexMut<&str> for Airports {
    fn index_mut(&mut self, index: &str) -> &mut Self::Output {
        &mut self.airports[index]
    }
}

#[derive(Debug)]
pub struct RunwayReportView {
    pub groups: Vec<RunwaySourceGroupView>,
}

#[derive(Debug)]
pub struct RunwaySourceGroupView {
    pub source_label: String, // "ATIS", "METAR", "fallback", "No runway config"
    pub source_class: String, // "", "none"
    pub airports: Vec<AirportRunwayView>,
}

#[derive(Debug)]
pub struct AirportRunwayView {
    pub icao: String,
    pub runway_text: String,          // "27 Arr + 09 Dep" or "(no selection)"
    #[allow(unused)]
    pub wind_text: String,            // "↑8 →3", "↓6 ←4", or "○"
    pub wind_head_arrow_text: String, // "↑", "↓", or "○"
    pub wind_head_value_text: String, // "8", "6", or ""
    pub wind_cross_left_arrow_text: String, // "→" or ""
    pub wind_cross_value_text: String, // "3", "5", or "○"
    pub wind_cross_right_arrow_text: String, // "←" or ""
    pub metar: String,
}

#[derive(Template)]
#[template(path = "runway_report.html")]
struct RunwayReportTemplate<'a> {
    groups: &'a [RunwaySourceGroupView],
}

#[cfg(test)]
pub(crate) mod tests {
    use metar_decoder::metar::Metar;

    use super::*;

    #[test]
    fn test_airport_gen() {
        let mut ap = Airports::new();
        let mut reader = std::io::Cursor::new(include_str!("../runway.test"));
        let config = ESConfig::new_for_test();
        ap.load_airports_from_sector_file(&mut reader, &config)
            .unwrap();
        assert_eq!(ap.airports.len(), 50);
    }

    pub(crate) fn make_test_airport(metar_str: &str) -> Airport {
        let icao = metar_str[0..4].to_string();
        let mut ap = Airports::new();
        let mut reader = std::io::Cursor::new(include_str!("../runway.test"));
        let config = ESConfig::new_for_test();
        ap.load_airports_from_sector_file(&mut reader, &config)
            .unwrap();
        let airport = ap.airports.swap_remove(&icao).unwrap();
        let metar: Metar = metar_str.parse().unwrap();
        Airport {
            icao: airport.icao,
            metar: Some(metar),
            runways: airport.runways,
            runways_in_use: IndexMap::new(),
        }
    }

    fn setup_test_airport_from_metar(metar: &str) -> Airport {
        let ap = make_test_airport(metar);
        let mut aps = Airports::new();
        aps.add_airport(ap);
        let config = ESConfig::new_for_test();
        aps.select_runway_in_use(&config);
        aps.airports.pop().unwrap().1
    }

    #[test]
    fn test_engm_variable_to_segregated_name() {
        let metar = "ENGM 080920Z VRB03KT 9999 -SHSN OVC009 M09/M12 Q1024 NOSIG";
        let gm = setup_test_airport_from_metar(metar);
        assert_eq!(
            gm.runways_in_use,
            IndexMap::from([(
                RunwayInUseSource::Default,
                [
                    ("01L".to_string(), RunwayUse::Departing),
                    ("01R".to_string(), RunwayUse::Arriving)
                ]
                .into()
            )])
        )
    }

    #[test]
    fn test_metar_enmh() {
        let metar = "ENMH 220550Z AUTO 30009KT 250V330 9999 BKN028/// OVC049/// 07/02 Q1016";
        let airport = setup_test_airport_from_metar(metar);
        assert_eq!(
            airport.runways_in_use,
            IndexMap::from([(
                RunwayInUseSource::Metar,
                [("35".to_string(), RunwayUse::Both)].into()
            )])
        );
    }

    #[test]
    fn test_wind_text_omits_runway_identifier_for_single_and_parallel_selection() {
        let airport = make_test_airport("ENGM 011200Z 02006KT 9999 BKN020 M01/M04 Q1020 NOSIG");
        let single = IndexMap::from([("01L".to_string(), RunwayUse::Both)]);
        let parallel = IndexMap::from([
            ("01L".to_string(), RunwayUse::Both),
            ("01R".to_string(), RunwayUse::Both),
        ]);

        let single_text = Airports::format_wind_components_for_selection(&airport, &single);
        let parallel_text = Airports::format_wind_components_for_selection(&airport, &parallel);

        assert!(!single_text.contains("01L"));
        assert!(!parallel_text.contains("01L"));
        assert!(!parallel_text.contains("01R"));
    }

    #[test]
    fn test_wind_text_includes_runway_identifier_for_non_parallel_selection() {
        let airport = make_test_airport("ENZV 011200Z 08010KT 9999 OVC010 08/07 Q1001");
        let selection = IndexMap::from([
            ("10".to_string(), RunwayUse::Both),
            ("18".to_string(), RunwayUse::Both),
        ]);

        let text = Airports::format_wind_components_for_selection(&airport, &selection);
        assert!(text.contains("10"));
        assert!(text.contains("18"));
    }

    #[test]
    fn test_wind_text_dual_runway_in_use_source() {
        let mut airport = make_test_airport("ENZV 011200Z 05030KT CAVOK 20/20 Q1013");

        let runway_in_use: IndexMap<RunwayInUseSource, IndexMap<String, RunwayUse>> =
            IndexMap::from([(
                RunwayInUseSource::Metar,
                IndexMap::from([
                    ("36".to_owned(), RunwayUse::Both),
                    ("10".to_owned(), RunwayUse::Both),
                ]),
            )]);
        airport.runways_in_use = runway_in_use;

        let runway_text = Airports::format_runway_usage_for_selection(
            &airport,
            &airport.runways_in_use[&RunwayInUseSource::Metar],
        );
        let wind_text = Airports::format_wind_components_for_selection(
            &airport,
            &airport.runways_in_use[&RunwayInUseSource::Metar],
        );

        assert!(runway_text.contains('\n'));
        assert_eq!(runway_text.lines().count(), 2);
        assert!(wind_text.contains('\n'));
        assert_eq!(wind_text.lines().count(), 2);
        assert!(!wind_text.contains("  +  "));

        let first_wind = wind_text.lines().next().unwrap_or_default();
        let second_wind = wind_text.lines().nth(1).unwrap_or_default();
        assert!(first_wind.starts_with("36 "));
        assert!(second_wind.starts_with("10 "));
    }

    #[test]
    fn test_variable_crosswind_renders_arrows_on_both_sides() {
        let text = Airports::format_wind_components(RunwayWindComponents {
            headwind: 0,
            crosswind: 12,
            crosswind_direction: CrosswindDirection::Variable,
        });
        assert!(text.contains("→12←"));
    }

    #[test]
    fn test_variable_crosswind_splits_to_both_arrow_columns() {
        let (head_arrow, head_value, cross_left_arrow, cross_value, cross_right_arrow) =
            Airports::format_wind_columns(&RunwayWindComponents {
                headwind: 0,
                crosswind: 12,
                crosswind_direction: CrosswindDirection::Variable,
            });
        assert_eq!(head_arrow, "○");
        assert_eq!(head_value, "");
        assert_eq!(cross_left_arrow, "→");
        assert_eq!(cross_value, "12");
        assert_eq!(cross_right_arrow, "←");
    }

    #[test]
    fn test_wind_component_columns_split_when_non_parallel_runways_are_in_use() {
        let airport = make_test_airport("ENZV 011200Z 05030KT CAVOK 20/20 Q1013");
        let selection = IndexMap::from([
            ("36".to_string(), RunwayUse::Both),
            ("10".to_string(), RunwayUse::Both),
        ]);
        let (head_arrow, head_value, cross_left_arrow, cross_value, cross_right_arrow) =
            Airports::format_wind_component_columns_for_selection(&airport, &selection);

        assert_eq!(head_value.split('\n').count(), 2);
        assert_eq!(cross_value.split('\n').count(), 2);
        assert!(!cross_value.contains("36"));
        assert!(!cross_value.contains("10"));
        assert!(head_arrow.contains('\n'));
        assert!(cross_left_arrow.contains('\n') || cross_right_arrow.contains('\n'));
    }

    #[test]
    fn test_wind_text_uses_calm_symbol_and_no_config_has_single_no_selection_marker() {
        let airport = make_test_airport("ENHV 011200Z 00000KT 9999 OVC010 08/07 Q1001");
        let selection = IndexMap::from([("08".to_string(), RunwayUse::Both)]);
        let wind_text = Airports::format_wind_components_for_selection(&airport, &selection);
        assert!(wind_text.contains('○'));
        assert_eq!(wind_text.chars().count(), 8);

        let stronger_headwind_airport =
            make_test_airport("ENHV 011200Z 08015KT 9999 OVC010 08/07 Q1001");
        let stronger_wind_text =
            Airports::format_wind_components_for_selection(&stronger_headwind_airport, &selection);
        assert_eq!(
            wind_text.chars().count(),
            stronger_wind_text.chars().count()
        );

        let no_config_airport = make_test_airport("ENHV 011200Z 08010KT 9999 OVC010 08/07 Q1001");
        let airports = Airports {
            airports: IndexMap::from([(no_config_airport.icao.clone(), no_config_airport)]),
        };
        let report_data = IndexMap::from([(None, vec![("ENHV".to_string(), IndexMap::new())])]);
        let view = airports.build_runway_report_view(&report_data);
        let row = &view.groups[0].airports[0];
        assert_eq!(row.runway_text, "(no selection)");
        assert_eq!(row.wind_text, "");
        assert_eq!(row.wind_head_arrow_text, "");
        assert_eq!(row.wind_head_value_text, "");
        assert_eq!(row.wind_cross_left_arrow_text, "");
        assert_eq!(row.wind_cross_value_text, "");
        assert_eq!(row.wind_cross_right_arrow_text, "");
    }
}
