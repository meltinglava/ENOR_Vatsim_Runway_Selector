use askama::Template;
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use tabled::builder::Builder;
use tracing::warn;
use tracing_unwrap::ResultExt;

use std::{
    io::{self, Read, Write},
    ops::{Index, IndexMut},
};

use crate::{
    airport::{Airport, RunwayInUseSource},
    atis_parser::find_runway_in_use_from_atis,
    config::ESConfig,
    error::ApplicationResult,
    metar::get_metars,
    runway::RunwayUse,
    sector_file::load_airports_from_sct_runway_section,
};

pub struct Airports {
    pub airports: IndexMap<String, Airport>,
}

type AirportsConfigReportData =
    IndexMap<Option<RunwayInUseSource>, Vec<(String, IndexMap<String, RunwayUse>)>>;

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

    fn source_label(source: Option<&RunwayInUseSource>, none_with_colon: bool) -> &'static str {
        match source {
            Some(RunwayInUseSource::Atis) => "ATIS",
            Some(RunwayInUseSource::Metar) => "METAR",
            Some(RunwayInUseSource::Default) => "fallback",
            None if none_with_colon => "No runway config:",
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

    fn metar_text_for_airport(&self, icao: &str) -> String {
        self.airports
            .get(icao)
            .and_then(|airport| airport.metar.as_ref().map(|metar| metar.raw.clone()))
            .unwrap_or_else(|| format!("{icao} No METAR"))
    }

    pub(crate) fn make_runway_report(&self) {
        let mut stdout = std::io::stdout().lock();
        self.make_runway_report_with_writer(&mut stdout)
            .expect("Failed to write runway report");
    }

    fn make_runway_report_with_writer<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        let report_data = self.grouped_runway_config_report_data();
        self.write_runway_report_tabled(writer, &report_data)?;
        Ok(())
    }

    fn write_runway_report_tabled<W: Write>(
        &self,
        writer: &mut W,
        data: &AirportsConfigReportData,
    ) -> io::Result<()> {
        let mut builder = Builder::default();
        builder.push_record([
            "Selection Source",
            "Number of Airports",
            "Airports and Runways",
            "METAR",
        ]);
        for (source, configs) in data {
            let source_str = Self::source_label(source.as_ref(), true);
            let airports_str = configs
                .iter()
                .map(|(icao, runways)| {
                    Self::format_runway_usage(runways)
                        .map(|runways_str| format!("{icao}: {runways_str}"))
                        .unwrap_or_else(|| icao.to_owned())
                })
                .join("\n");
            let metars = configs
                .iter()
                .map(|(icao, _)| self.metar_text_for_airport(icao))
                .join("\n");
            builder.push_record([
                source_str,
                &configs.len().to_string(),
                &airports_str,
                &metars,
            ]);
        }
        let table = builder.build();
        writeln!(writer, "{}", table)
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
            let label = Self::source_label(source.as_ref(), false);
            let class = Self::source_class(source.as_ref());

            let mut airports = Vec::with_capacity(configs.len());

            for (icao, runways) in configs {
                let runway_text = Self::format_runway_usage(runways)
                    .unwrap_or_else(|| "(no selection)".to_string());
                let metar = self.metar_text_for_airport(icao);

                airports.push(AirportRunwayView {
                    icao: icao.clone(),
                    runway_text,
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
    pub runway_text: String, // "27 Arr + 09 Dep" or "(no selection)"
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
}
