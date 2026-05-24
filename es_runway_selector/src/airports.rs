use indexmap::{IndexMap, IndexSet};
use tracing::warn;

use std::{
    io::{self, Read},
    ops::{Index, IndexMut},
};

use crate::{
    airport::{Airport, RunwayInUseSource},
    atis_parser::find_runway_in_use_from_atis,
    config::ESConfig,
    error::ApplicationResult,
    metar::get_metars,
    plugin_manager::PluginManager,
    protocol_convert::{airport_to_protocol_info, metar_to_protocol, protocol_to_internal_use},
    report_builder,
    runway::RunwayUse,
    sector_file::load_airports_from_sct_runway_section,
};
use runway_selector_protocol::{AtisEntry, AtisRequest, RunwaySelectionRequest};

pub struct Airports {
    pub airports: IndexMap<String, Airport>,
}

impl Airports {
    pub fn new() -> Self {
        Self {
            airports: IndexMap::new(),
        }
    }

    #[allow(dead_code)]
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
        match get_metars(conf).await {
            Ok(metars) => {
                for metar in metars {
                    if let Some(airport) = self.airports.get_mut(&metar.icao) {
                        airport.metar = Some(metar);
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "METAR fetch failed; runway selection will fall back to defaults");
            }
        }
    }

    /// Fetch all VATSIM ATIS, route each airport's ATIS to the appropriate
    /// plugin (or fall back to the built-in regex parser).
    pub async fn read_atis_and_apply_runways(
        &mut self,
        plugins: &PluginManager,
    ) -> ApplicationResult<()> {
        let icaos = self.identifiers();
        let v3_data = vatsim_utils::live_api::Vatsim::new()
            .await?
            .get_v3_data()
            .await?;
        let atis_entries = v3_data.atis;

        // Collect ATIS texts grouped by ICAO.
        let mut atis_by_icao: IndexMap<String, (Vec<String>, Option<char>)> = IndexMap::new();
        for atis in &atis_entries {
            let icao = &atis.callsign[0..4];
            if !icaos.contains(icao) {
                continue;
            }
            let Some(ref atis_lines) = atis.text_atis else {
                continue;
            };
            let text = atis_lines.to_vec().join(" ");
            let letter = atis.callsign.chars().last();
            atis_by_icao.insert(icao.to_string(), (vec![text], letter));
        }

        // Build protocol ATIS entries + airport infos for the plugin call.
        let plugin_entries: Vec<AtisEntry> = atis_by_icao
            .iter()
            .flat_map(|(icao, (texts, letter))| {
                texts.iter().map(|text| AtisEntry {
                    airport_icao: icao.clone(),
                    atis_text: text.clone(),
                    information_letter: *letter,
                })
            })
            .collect();

        let plugin_airports: Vec<_> = plugin_entries
            .iter()
            .filter_map(|e| {
                self.airports
                    .get(&e.airport_icao)
                    .map(|ap| airport_to_protocol_info(&ap.icao, &ap.runways))
            })
            .collect();

        if !plugin_entries.is_empty() {
            let req = AtisRequest {
                atis_entries: plugin_entries.clone(),
                airports: plugin_airports,
            };
            if let Some(plugin_resp) = plugins.call_atis(&req).await {
                for airport_assignment in plugin_resp.airports {
                    let Some(airport) = self.airports.get_mut(&airport_assignment.airport_icao)
                    else {
                        continue;
                    };
                    for assignment in airport_assignment.assignments {
                        airport
                            .runways_in_use
                            .entry(RunwayInUseSource::Atis)
                            .or_default()
                            .entry(assignment.runway_id)
                            .and_modify(|existing| {
                                let new = protocol_to_internal_use(assignment.runway_use);
                                *existing = existing.merged_with(new);
                            })
                            .or_insert_with(|| protocol_to_internal_use(assignment.runway_use));
                    }
                    if !airport_assignment.tags.is_empty() {
                        airport
                            .selection_tags
                            .insert(RunwayInUseSource::Atis, airport_assignment.tags);
                    }
                }
            }
        }

        // Built-in ATIS parser for airports not handled by any plugin.
        for (icao, (texts, _letter)) in &atis_by_icao {
            if plugins.has_plugin_for(icao) {
                continue;
            }
            let Some(airport) = self.airports.get_mut(icao.as_str()) else {
                continue;
            };
            let text = texts.join(" ");
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

    /// Apply METAR-based runway selection, then defaults.
    pub async fn select_runway_in_use(&mut self, config: &ESConfig, plugins: &PluginManager) {
        self.apply_plugin_runway_selection(plugins).await;
        self.apply_metar_wind_fallback(plugins);
        self.apply_default_runways(config);
    }

    /// Route plugin-handled airports to their plugin for METAR-based selection.
    async fn apply_plugin_runway_selection(&mut self, plugins: &PluginManager) {
        let icaos: Vec<String> = self.airports.keys().cloned().collect();
        for icao in icaos {
            if !plugins.has_plugin_for(&icao) {
                continue;
            }
            let airport = self.airports.get(&icao).unwrap();
            let req = RunwaySelectionRequest {
                airport: airport_to_protocol_info(&airport.icao, &airport.runways),
                metar: airport.metar.as_ref().map(metar_to_protocol),
            };
            if let Some(resp) = plugins.call_runway_selection(&req).await
                && !resp.runways.is_empty()
            {
                let map: IndexMap<String, RunwayUse> = resp
                    .runways
                    .into_iter()
                    .map(|a| (a.runway_id, protocol_to_internal_use(a.runway_use)))
                    .collect();
                let airport = self.airports.get_mut(&icao).unwrap();
                airport.runways_in_use.insert(RunwayInUseSource::Metar, map);
                if !resp.tags.is_empty() {
                    airport
                        .selection_tags
                        .insert(RunwayInUseSource::Metar, resp.tags);
                }
            }
        }
    }

    /// Apply built-in headwind logic for airports not handled by any plugin.
    fn apply_metar_wind_fallback(&mut self, plugins: &PluginManager) {
        for airport in self.airports.values_mut() {
            if plugins.has_plugin_for(&airport.icao) {
                continue;
            }
            if let Ok(runways_in_use) = airport.set_runway_based_on_metar_wind()
                && !runways_in_use.is_empty()
            {
                airport
                    .runways_in_use
                    .insert(RunwayInUseSource::Metar, runways_in_use);
            }
        }
    }

    pub(crate) fn apply_default_runways(&mut self, config: &ESConfig) {
        let defaults = config.get_default_runways();
        for airport in self.airports.values_mut() {
            let default_entry = airport.runways_in_use.entry(RunwayInUseSource::Default);
            match default_entry {
                indexmap::map::Entry::Occupied(_) => continue,
                indexmap::map::Entry::Vacant(v) => {
                    if let Some(runway) = defaults.get(airport.icao.as_str()) {
                        let identifier = format!("{runway:02}");
                        if airport
                            .runways
                            .iter()
                            .any(|rw| rw.iter().any(|dir| dir.identifier == identifier))
                        {
                            v.insert([(identifier, RunwayUse::Both)].into());
                        } else {
                            warn!(airport.icao, default_runway = %runway, "Default runway not found in airport runways; airport will appear as unselected");
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

    pub fn make_runway_report_html(&self) -> io::Result<()> {
        report_builder::open_html_report(&self.airports)
    }

    #[cfg(test)]
    pub(crate) fn make_runway_report_html_with_writer<W: std::io::Write>(
        &self,
        writer: &mut W,
    ) -> io::Result<()> {
        let html = report_builder::render_html(&report_builder::build_report(&self.airports))?;
        writer.write_all(html.as_bytes())
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

#[cfg(test)]
pub(crate) mod tests {
    use metar_decoder::metar::Metar;

    use super::*;
    use crate::{
        airport::CrosswindDirection, airport::RunwayWindComponents, config::ESConfig,
        report_builder, runway::RunwayUse,
    };

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
            selection_tags: IndexMap::new(),
        }
    }

    fn setup_test_airport_from_metar_no_plugins(metar: &str) -> Airport {
        let ap = make_test_airport(metar);
        let mut aps = Airports::new();
        aps.add_airport(ap);
        let config = ESConfig::new_for_test();
        for airport in aps.airports.values_mut() {
            if let Ok(runways_in_use) = airport.set_runway_based_on_metar_wind()
                && !runways_in_use.is_empty()
            {
                airport
                    .runways_in_use
                    .insert(RunwayInUseSource::Metar, runways_in_use);
            }
        }
        aps.apply_default_runways(&config);
        aps.airports.pop().unwrap().1
    }

    #[test]
    fn test_metar_enmh() {
        let metar = "ENMH 220550Z AUTO 30009KT 250V330 9999 BKN028/// OVC049/// 07/02 Q1016";
        let airport = setup_test_airport_from_metar_no_plugins(metar);
        assert_eq!(
            airport.runways_in_use,
            IndexMap::from([(
                RunwayInUseSource::Metar,
                [("35".to_string(), RunwayUse::Both)].into()
            )])
        );
    }

    #[test]
    fn test_wind_text_dual_runway_in_use_source() {
        let mut airport = make_test_airport("ENZV 011200Z 05030KT CAVOK 20/20 Q1013");
        airport.runways_in_use = IndexMap::from([(
            RunwayInUseSource::Metar,
            IndexMap::from([
                ("36".to_owned(), RunwayUse::Both),
                ("10".to_owned(), RunwayUse::Both),
            ]),
        )]);

        let runway_text = report_builder::format_runway_usage_for_selection(
            &airport,
            &airport.runways_in_use[&RunwayInUseSource::Metar],
        );

        assert!(runway_text.contains('\n'));
        assert_eq!(runway_text.lines().count(), 2);
    }

    #[test]
    fn test_variable_crosswind_splits_to_both_arrow_columns() {
        let (head_arrow, head_value, cross_left_arrow, cross_value, cross_right_arrow) =
            report_builder::format_wind_columns(&RunwayWindComponents {
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
    fn test_wind_column_head_and_tail_arrows_match_report_orientation() {
        let (head_arrow, head_value, _, _, _) =
            report_builder::format_wind_columns(&RunwayWindComponents {
                headwind: 8,
                crosswind: 0,
                crosswind_direction: CrosswindDirection::Left,
            });
        assert_eq!(head_arrow, "↓");
        assert_eq!(head_value, "8");

        let (tail_arrow, tail_value, _, _, _) =
            report_builder::format_wind_columns(&RunwayWindComponents {
                headwind: -6,
                crosswind: 0,
                crosswind_direction: CrosswindDirection::Left,
            });
        assert_eq!(tail_arrow, "↑");
        assert_eq!(tail_value, "6");
    }

    #[test]
    fn test_wind_component_columns_split_when_non_parallel_runways_are_in_use() {
        let airport = make_test_airport("ENZV 011200Z 05030KT CAVOK 20/20 Q1013");
        let selection = IndexMap::from([
            ("36".to_string(), RunwayUse::Both),
            ("10".to_string(), RunwayUse::Both),
        ]);
        let (head_arrow, head_value, cross_left_arrow, cross_value, cross_right_arrow) =
            report_builder::format_wind_component_columns_for_selection(&airport, &selection);

        assert_eq!(head_value.split('\n').count(), 2);
        assert_eq!(cross_value.split('\n').count(), 2);
        assert!(!cross_value.contains("36"));
        assert!(!cross_value.contains("10"));
        assert!(head_arrow.contains('\n'));
        assert!(cross_left_arrow.contains('\n') || cross_right_arrow.contains('\n'));
    }

    fn make_issue20_enzv_airports() -> Airports {
        let mut airport = make_test_airport("ENZV 191650Z 30005KT CAVOK 08/04 Q1026 NOSIG");
        airport.runways_in_use = IndexMap::from([(
            RunwayInUseSource::Metar,
            IndexMap::from([
                ("36".to_string(), RunwayUse::Both),
                ("28".to_string(), RunwayUse::Both),
            ]),
        )]);

        Airports {
            airports: IndexMap::from([(airport.icao.clone(), airport)]),
        }
    }

    #[test]
    fn test_report_view_splits_non_parallel_selection_into_aligned_lines() {
        let airports = make_issue20_enzv_airports();
        let view = report_builder::build_report(&airports.airports);
        let row = &view.groups[0].airports[0];

        assert_eq!(row.line_count, 2);
        assert_eq!(row.lines[0].runway_text, "36");
        assert_eq!(row.lines[0].wind_head_arrow_text, "↓");
        assert_eq!(row.lines[0].wind_head_value_text, "3");
        assert_eq!(row.lines[0].wind_cross_left_arrow_text, "→");
        assert_eq!(row.lines[0].wind_cross_value_text, "5");
        assert_eq!(row.lines[0].wind_cross_right_arrow_text, "");

        assert_eq!(row.lines[1].runway_text, "28");
        assert_eq!(row.lines[1].wind_head_arrow_text, "↓");
        assert_eq!(row.lines[1].wind_head_value_text, "5");
        assert_eq!(row.lines[1].wind_cross_left_arrow_text, "");
        assert_eq!(row.lines[1].wind_cross_value_text, "2");
        assert_eq!(row.lines[1].wind_cross_right_arrow_text, "←");
    }

    #[test]
    fn test_report_html_renders_non_parallel_selection_as_rowspans() {
        let airports = make_issue20_enzv_airports();
        let mut rendered = Vec::new();
        airports
            .make_runway_report_html_with_writer(&mut rendered)
            .unwrap();
        let html = String::from_utf8(rendered).unwrap();

        assert!(html.contains("rowspan=\"2\">ENZV</td>"));
        assert!(html.contains("rowspan=\"2\">ENZV 191650Z 30005KT CAVOK 08/04 Q1026 NOSIG</td>"));
        assert!(!html.contains("36\n28"));
        assert!(!html.contains("↓\n↓"));
    }

    #[test]
    #[ignore = "writes a manual inspection artifact to /tmp"]
    fn write_issue20_demo_report() {
        let airports = make_issue20_enzv_airports();
        let mut rendered = Vec::new();
        airports
            .make_runway_report_html_with_writer(&mut rendered)
            .unwrap();
        let path = "/tmp/issue20_enzv_report.html";
        std::fs::write(path, rendered).unwrap();
        assert!(std::path::Path::new(path).exists());
    }
}
