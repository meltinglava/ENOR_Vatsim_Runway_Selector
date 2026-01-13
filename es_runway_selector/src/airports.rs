use askama::Template;
use encoding::{
    DecoderTrap, Encoding,
    all::{ISO_8859_1, UTF_8},
};
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
    atis::find_runway_in_use_from_atis,
    config::ESConfig,
    error::ApplicationResult,
    metar::get_metars,
    runway::{Runway, RunwayDirection, RunwayUse},
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

    pub fn fill_known_airports<R: Read>(
        &mut self,
        reader: &mut R,
        config: &ESConfig,
    ) -> ApplicationResult<()> {
        let sct_file = read_with_encoings(reader).expect("Failed to read SCT file");
        let ignored_set = config.get_ignore_airports();

        for line in sct_file
            .lines()
            .skip_while(|line| *line != "[RUNWAY]")
            .skip(1)
            .take_while(|line| !line.is_empty())
        {
            let parts: Vec<_> = line.split_whitespace().collect();
            let icao = *parts.last().unwrap();
            if ignored_set.contains(icao) {
                continue;
            }

            let airport = self
                .airports
                .entry(icao.to_string())
                .or_insert_with(|| Airport {
                    icao: icao.to_string(),
                    metar: None,
                    runways: Vec::new(),
                    runways_in_use: IndexMap::new(),
                });

            let runway = Runway {
                runways: [
                    RunwayDirection {
                        degrees: parts[2].parse()?,
                        identifier: parts[0].into(),
                    },
                    RunwayDirection {
                        degrees: parts[3].parse()?,
                        identifier: parts[1].into(),
                    },
                ],
            };
            airport.runways.push(runway);
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

    pub async fn read_atises_and_apply_runways(&mut self) -> ApplicationResult<()> {
        let icaos = self.identifiers();
        let v3_data = vatsim_utils::live_api::Vatsim::new()
            .await?
            .get_v3_data()
            .await?;
        let atises = v3_data.atis;
        for atis in atises {
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
            for (runway, config) in find_runway_in_use_from_atis(&text) {
                airport
                    .runways_in_use
                    .entry(RunwayInUseSource::Atis)
                    .or_default()
                    .entry(runway)
                    .and_modify(|e| {
                        *e = match (*e, config) {
                            (RunwayUse::Both, _) | (_, RunwayUse::Both) => RunwayUse::Both,
                            (RunwayUse::Arriving, RunwayUse::Departing)
                            | (RunwayUse::Departing, RunwayUse::Arriving) => RunwayUse::Both,
                            (e, _) => e,
                        }
                    })
                    .or_insert(config);
            }
        }

        Ok(())
    }

    pub fn runway_in_use_based_on_metar(&mut self, config: &ESConfig) {
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

    pub fn apply_default_runways(&mut self, config: &ESConfig) {
        let defaults = config.get_default_runways();
        for airport in self.airports.values_mut() {
            if let Some(runway) = defaults.get(airport.icao.as_str()) {
                let identifier = format!("{runway:02}");
                if airport.runways.iter().any(|rw| {
                    rw.runways
                        .iter()
                        .any(|dir| dir.identifier[0..2] == identifier)
                }) {
                    airport.runways_in_use.insert(
                        RunwayInUseSource::Default,
                        [(identifier, RunwayUse::Both)].into(),
                    );
                } else {
                    warn!(airport.icao, default_runway = %runway, "Default runway not found in airport runways");
                }
            }
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

    pub(crate) fn make_runway_report(&self) {
        let mut stdout = std::io::stdout().lock();
        self.make_runway_report_with_writer(&mut stdout)
            .expect("Failed to write runway report");
    }

    fn make_runway_report_with_writer<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        let mut counter = AirportsConfigReportData::new();
        'airport_loop: for airport in self.airports.values() {
            for selection_source in RunwayInUseSource::default_sort_order() {
                if let Some(runway_selection) = airport.runways_in_use.get(&selection_source) {
                    counter
                        .entry(Some(selection_source))
                        .or_insert_with(|| Vec::with_capacity(1))
                        .push((airport.icao.clone(), runway_selection.clone()));
                    continue 'airport_loop;
                }
            }
            counter
                .entry(None)
                .or_insert_with(|| Vec::with_capacity(1))
                .push((airport.icao.clone(), IndexMap::new()));
        }
        counter.sort_unstable_by(|k1, _v1, k2, _v2| match (k1, k2) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (Some(k1), Some(k2)) => k1.cmp(k2),
        });
        self.write_runway_report_tabled(writer, &counter)?;
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
            let source_str = match source {
                Some(RunwayInUseSource::Atis) => "ATIS",
                Some(RunwayInUseSource::Metar) => "METAR",
                Some(RunwayInUseSource::Default) => "fallback",
                None => "No runway config:",
            };
            let airports_str = configs
                .iter()
                .map(|(icao, runways)| {
                    if runways.is_empty() {
                        return icao.to_owned();
                    }
                    let runways_str = runways
                        .iter()
                        .map(|(rw, usage)| {
                            format!(
                                "{}{}",
                                rw,
                                match usage {
                                    RunwayUse::Arriving => " Arr",
                                    RunwayUse::Departing => " Dep",
                                    RunwayUse::Both => "",
                                }
                            )
                        })
                        .join(" + ");
                    format!("{}: {}", icao, runways_str)
                })
                .join("\n");
            let metars = configs
                .iter()
                .map(|(icao, _)| -> String {
                    self.airports
                        .get(icao)
                        .and_then(|airport| airport.metar.as_ref().map(|m| m.raw.clone()))
                        .unwrap_or_else(|| format!("{} No METAR", icao))
                })
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
        let mut file = tempfile::NamedTempFile::with_prefix("runway_selector")?;
        self.make_runway_report_html_with_writer(&mut file)?;
        open::that_detached(file.path())?;
        file.keep()?;
        Ok(())
    }

    fn make_runway_report_html_with_writer<W: Write>(
        &self,
        writer: &mut W,
    ) -> io::Result<()> {
        let mut counter = AirportsConfigReportData::new();

        // Build the grouped report data (same logic as your table output)
        'airport_loop: for airport in self.airports.values() {
            for selection_source in RunwayInUseSource::default_sort_order() {
                if let Some(runway_selection) = airport.runways_in_use.get(&selection_source) {
                    counter
                        .entry(Some(selection_source))
                        .or_insert_with(|| Vec::with_capacity(1))
                        .push((airport.icao.clone(), runway_selection.clone()));
                    continue 'airport_loop;
                }
            }

            // No runway config found for any source
            counter
                .entry(None)
                .or_insert_with(|| Vec::with_capacity(1))
                .push((airport.icao.clone(), IndexMap::new()));
        }

        // Sort groups so None (no config) ends last; otherwise by RunwayInUseSource ordering
        counter.sort_unstable_by(|k1, _v1, k2, _v2| match (k1, k2) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (Some(k1), Some(k2)) => k1.cmp(k2),
        });

        // Convert to view model for the template
        let view = self.build_runway_report_view(&counter);

        // Render template
        let tpl = RunwayReportTemplate {
            groups: &view.groups,
        };

        let html = tpl
            .render()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        writer.write_all(html.as_bytes())
    }


    fn build_runway_report_view(&self, data: &AirportsConfigReportData) -> RunwayReportView {
        let mut groups = Vec::new();

        for (source, configs) in data {
            let (label, class) = match source {
                Some(RunwayInUseSource::Atis) => ("ATIS", ""),
                Some(RunwayInUseSource::Metar) => ("METAR", ""),
                Some(RunwayInUseSource::Default) => ("fallback", ""),
                None => ("No runway config", "none"),
            };

            let mut airports = Vec::with_capacity(configs.len());

            for (icao, runways) in configs {
                let runway_text = if runways.is_empty() {
                    "(no selection)".to_string()
                } else {
                    runways
                        .iter()
                        .map(|(rw, usage)| {
                            let suffix = match usage {
                                RunwayUse::Arriving => " Arr",
                                RunwayUse::Departing => " Dep",
                                RunwayUse::Both => "",
                            };
                            format!("{rw}{suffix}")
                        })
                        .join(" + ")
                };

                let metar = self
                    .airports
                    .get(icao)
                    .and_then(|a| a.metar.as_ref().map(|m| m.raw.clone()))
                    .unwrap_or_else(|| format!("{icao} No METAR"));

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

fn read_with_encoings<R: Read>(reader: &mut R) -> ApplicationResult<String> {
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer)?;

    let utf8_decoded = UTF_8.decode(&buffer, DecoderTrap::Strict);

    match utf8_decoded {
        Ok(text) => Ok(text),
        Err(e) => ISO_8859_1
            .decode(&buffer, DecoderTrap::Strict)
            .map_err(|_| crate::error::ApplicationError::EncodingError(e.to_string())),
    }
}

#[derive(Debug)]
pub struct RunwayReportView {
    pub groups: Vec<RunwaySourceGroupView>,
}

#[derive(Debug)]
pub struct RunwaySourceGroupView {
    pub source_label: String,     // "ATIS", "METAR", "fallback", "No runway config"
    pub source_class: String,     // "", "none"
    pub airports: Vec<AirportRunwayView>,
}

#[derive(Debug)]
pub struct AirportRunwayView {
    pub icao: String,
    pub runway_text: String,     // "27 Arr + 09 Dep" or "(no selection)"
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
        ap.fill_known_airports(&mut reader, &config).unwrap();
        assert_eq!(ap.airports.len(), 50);
    }

    pub(crate) fn make_test_airport(metar_str: &str) -> Airport {
        let icao = metar_str[0..4].to_string();
        let mut ap = Airports::new();
        let mut reader = std::io::Cursor::new(include_str!("../runway.test"));
        let config = ESConfig::new_for_test();
        ap.fill_known_airports(&mut reader, &config).unwrap();
        let airport = ap.airports.swap_remove(&icao).unwrap();
        let metar: Metar = metar_str.parse().unwrap();
        Airport {
            icao: airport.icao,
            metar: Some(metar),
            runways: airport.runways,
            runways_in_use: IndexMap::new(),
        }
    }
}
