use std::{path::Path, time::Duration};

use metar_decoder::{
    obscuration::{Cloud, CloudCoverage, Obscuration, Qualifier, VisibilityUnit, WeatherIntensity},
    optional_data::OptionalData,
    units::{track::Track, velocity::VelocityUnit},
    wind::WindDirection,
};
use runway_plugin_api::{
    AirportSelectionRequest, CloudData, CrosswindDirection, MetarData, ParsedMetar, RunwayInfo,
    RunwaySelectionsRequest, RunwaySelectionsResponse, RvrData, WeatherDescriptor,
    WeatherPhenomenonData, WindData,
};
use tokio::process::{Child, Command};
use tracing::{debug, warn};

use crate::{
    airport::{Airport, CrosswindDirection as InternalCrosswindDirection},
    helpers_server::HelpersServer,
    mise_manager::{find_or_download_mise, mise_invocation_for_extension},
};

pub struct PluginProcess {
    _child: Child,
    _helpers: HelpersServer, // dropped after _child to avoid use-after-free
    base_url: String,
    client: reqwest::Client,
}

#[derive(Debug)]
pub enum PluginError {
    Spawn(std::io::Error),
    Io(std::io::Error),
    Http(reqwest::Error),
    Timeout,
    Archive(String),
}

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PluginError::Spawn(e) => write!(f, "Failed to spawn plugin process: {e}"),
            PluginError::Io(e) => write!(f, "I/O error: {e}"),
            PluginError::Http(e) => write!(f, "Plugin HTTP error: {e}"),
            PluginError::Timeout => write!(f, "Timed out waiting for plugin health check"),
            PluginError::Archive(s) => write!(f, "Archive extraction failed: {s}"),
        }
    }
}

impl PluginProcess {
    pub async fn spawn(binary: &Path) -> Result<Self, PluginError> {
        // Start the helpers server first so the plugin can call it during startup.
        let helpers = HelpersServer::start().await.map_err(PluginError::Spawn)?;

        let port = find_free_port().await.map_err(PluginError::Spawn)?;

        debug!(
            "Spawning plugin {:?} on port {}, helpers on port {}",
            binary, port, helpers.port
        );

        let child = spawn_process(binary, port, helpers.port).await?;

        let client = reqwest::Client::new();
        let base_url = format!("http://127.0.0.1:{port}");

        wait_for_health(&client, &base_url).await?;
        debug!("Plugin is ready at {}", base_url);

        Ok(Self {
            _child: child,
            _helpers: helpers,
            base_url,
            client,
        })
    }

    pub async fn query(
        &self,
        request: &RunwaySelectionsRequest,
    ) -> Result<RunwaySelectionsResponse, PluginError> {
        self.client
            .post(format!("{}/runway-selections", self.base_url))
            .json(request)
            .send()
            .await
            .map_err(PluginError::Http)?
            .json::<RunwaySelectionsResponse>()
            .await
            .map_err(PluginError::Http)
    }
}

/// Spawn the plugin.
///
/// - Native binaries are run directly.
/// - Scripts with a recognised extension (`.py`, `.js`, `.ts`, `.rb`, …) are
///   run via `mise exec --yes <tool@latest> -- <runtime> <script>`.  mise is
///   downloaded and cached automatically if it is not already in PATH.
///
/// Both forms receive `--port` and `--helpers-port`.
async fn spawn_process(binary: &Path, port: u16, helpers_port: u16) -> Result<Child, PluginError> {
    let ext = binary.extension().and_then(|e| e.to_str()).unwrap_or("");

    if let Some((tool, runtime_cmd)) = mise_invocation_for_extension(ext) {
        let mise = find_or_download_mise().await?;
        let mut cmd = Command::new(mise);
        cmd.arg("exec")
            .arg("--yes") // auto-install the runtime if missing
            .arg(tool)
            .arg("--")
            .args(runtime_cmd)
            .arg(binary)
            .arg("--port")
            .arg(port.to_string())
            .arg("--helpers-port")
            .arg(helpers_port.to_string())
            .kill_on_drop(true);
        cmd.spawn().map_err(PluginError::Spawn)
    } else {
        Command::new(binary)
            .arg("--port")
            .arg(port.to_string())
            .arg("--helpers-port")
            .arg(helpers_port.to_string())
            .kill_on_drop(true)
            .spawn()
            .map_err(PluginError::Spawn)
    }
}

pub(crate) async fn find_free_port() -> std::io::Result<u16> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

async fn wait_for_health(client: &reqwest::Client, base_url: &str) -> Result<(), PluginError> {
    let url = format!("{base_url}/health");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);

    loop {
        if let Ok(resp) = client.get(&url).send().await
            && resp.status().is_success()
        {
            return Ok(());
        }

        if tokio::time::Instant::now() >= deadline {
            warn!("Plugin health check timed out at {}", base_url);
            return Err(PluginError::Timeout);
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

pub fn build_request(
    airports: impl Iterator<Item = impl std::borrow::Borrow<Airport>>,
) -> RunwaySelectionsRequest {
    let timestamp_utc = jiff::Timestamp::now().to_string();
    RunwaySelectionsRequest {
        airports: airports
            .map(|a| build_airport_request(a.borrow(), &timestamp_utc))
            .collect(),
    }
}

fn build_airport_request(airport: &Airport, timestamp_utc: &str) -> AirportSelectionRequest {
    let runways = airport
        .runways
        .iter()
        .flat_map(|runway| runway.runways.iter())
        .map(|dir| {
            let headwind = airport.runway_max_headwind(dir);
            let tailwind = airport.runway_max_tailwind(dir);
            let (crosswind, crosswind_direction) = match airport.runway_max_crosswind(dir) {
                Some((cw, d)) => (
                    Some(cw),
                    Some(match d {
                        InternalCrosswindDirection::Left => CrosswindDirection::Left,
                        InternalCrosswindDirection::Right => CrosswindDirection::Right,
                        InternalCrosswindDirection::Variable => CrosswindDirection::Variable,
                    }),
                ),
                None => (None, None),
            };
            RunwayInfo {
                identifier: dir.identifier.clone(),
                heading: dir.degrees,
                headwind_kt: headwind,
                tailwind_kt: tailwind,
                crosswind_kt: crosswind,
                crosswind_direction,
            }
        })
        .collect();

    let metar = airport.metar.as_ref().map(convert_metar);

    AirportSelectionRequest {
        icao: airport.icao.clone(),
        runways,
        metar,
        timestamp_utc: timestamp_utc.to_string(),
    }
}

fn convert_metar(metar: &metar_decoder::metar::Metar) -> MetarData {
    MetarData {
        raw: metar.raw.clone(),
        parsed: Some(convert_parsed_metar(metar)),
    }
}

fn convert_parsed_metar(metar: &metar_decoder::metar::Metar) -> ParsedMetar {
    let is_cavok = matches!(&metar.obscuration, Obscuration::Cavok);
    let wind = Some(convert_wind(&metar.wind));

    let (visibility_meters, rvr, clouds, vertical_visibility_hundreds_ft, weather_phenomena) =
        match &metar.obscuration {
            Obscuration::Cavok => (None, vec![], vec![], None, vec![]),
            Obscuration::Described(desc) => {
                let visibility = match desc.visibility.value {
                    VisibilityUnit::Meters(OptionalData::Data(v)) => Some(v),
                    _ => None,
                };

                let rvr = desc
                    .rvr
                    .iter()
                    .map(|r| RvrData {
                        runway: r.runway.clone(),
                        meters: r.value.to_option(),
                    })
                    .collect();

                let clouds = desc
                    .clouds
                    .iter()
                    .filter_map(|c| match c {
                        Cloud::CloudData(cd) => Some(CloudData {
                            coverage: match cd.coverage {
                                OptionalData::Data(CloudCoverage::Few) => {
                                    Some(runway_plugin_api::CloudCoverage::Few)
                                }
                                OptionalData::Data(CloudCoverage::Scattered) => {
                                    Some(runway_plugin_api::CloudCoverage::Scattered)
                                }
                                OptionalData::Data(CloudCoverage::Broken) => {
                                    Some(runway_plugin_api::CloudCoverage::Broken)
                                }
                                OptionalData::Data(CloudCoverage::Overcast) => {
                                    Some(runway_plugin_api::CloudCoverage::Overcast)
                                }
                                OptionalData::Undefined => None,
                            },
                            height_hundreds_ft: match &cd.height {
                                OptionalData::Data(h) => Some(h.height),
                                OptionalData::Undefined => None,
                            },
                            cloud_type: cd.cloud_type.as_ref().and_then(|ct| match ct {
                                OptionalData::Data(s) => Some(s.clone()),
                                OptionalData::Undefined => None,
                            }),
                        }),
                        Cloud::NCD | Cloud::NSC | Cloud::CLR => None,
                    })
                    .collect();

                let vv = desc
                    .vertical_visibility
                    .as_ref()
                    .map(|vv| vv.visibility.to_option().map(|v| v as i32).unwrap_or(0));

                let weather = desc
                    .present_weather
                    .iter()
                    .map(|pw| WeatherPhenomenonData {
                        intensity: pw.intensity.as_ref().map(|i| match i {
                            WeatherIntensity::Light => runway_plugin_api::WeatherIntensity::Light,
                            WeatherIntensity::Heavy => runway_plugin_api::WeatherIntensity::Heavy,
                            WeatherIntensity::Vicinity => {
                                runway_plugin_api::WeatherIntensity::Vicinity
                            }
                        }),
                        descriptors: pw
                            .descriptor
                            .as_ref()
                            .map(|d| {
                                vec![match d {
                                    Qualifier::Shallow => WeatherDescriptor::Shallow,
                                    Qualifier::Partial => WeatherDescriptor::Partial,
                                    Qualifier::Patches => WeatherDescriptor::Patches,
                                    Qualifier::Low => WeatherDescriptor::LowDrifting,
                                    Qualifier::Blowing => WeatherDescriptor::Blowing,
                                    Qualifier::Showers => WeatherDescriptor::Shower,
                                    Qualifier::Thunderstorm => WeatherDescriptor::Thunderstorm,
                                    Qualifier::Freezing => WeatherDescriptor::Freezing,
                                }]
                            })
                            .unwrap_or_default(),
                        phenomena: pw
                            .phenomena
                            .iter()
                            .filter_map(|p| p.clone().to_option())
                            .map(|p| format!("{p:?}"))
                            .collect(),
                    })
                    .collect();

                (visibility, rvr, clouds, vv, weather)
            }
        };

    let temperature_c = metar.temperature.temp.to_option();
    let dew_point_c = metar.temperature.dew_point.to_option();
    let qnh_hpa = metar.pressure.qnh.as_ref().and_then(|p| {
        matches!(p.unit, metar_decoder::pressure::PressureUnit::Hectopascals)
            .then(|| p.value.to_option())
            .flatten()
    });

    ParsedMetar {
        is_cavok,
        wind,
        visibility_meters,
        rvr,
        clouds,
        vertical_visibility_hundreds_ft,
        weather_phenomena,
        temperature_c,
        dew_point_c,
        qnh_hpa,
    }
}

fn convert_wind(wind: &metar_decoder::wind::Wind) -> WindData {
    let (direction_degrees, is_variable) = match &wind.dir {
        WindDirection::Heading(Track(OptionalData::Data(deg))) => (Some(*deg), false),
        WindDirection::Heading(Track(OptionalData::Undefined)) => (None, false),
        WindDirection::Variable => (None, true),
    };

    let mps_to_kt = |v: u32| -> u32 {
        match wind.speed.unit {
            VelocityUnit::Knots => v,
            VelocityUnit::MetersPerSecond => (v as f64 * 1.94384).round() as u32,
        }
    };

    let speed_kt = wind.speed.velocity.to_option().map(mps_to_kt).unwrap_or(0);
    let gust_kt = wind.speed.gust.and_then(|g| g.to_option()).map(mps_to_kt);

    let (variable_from_degrees, variable_to_degrees) = match wind.varying {
        Some((Track(OptionalData::Data(from)), Track(OptionalData::Data(to)))) => {
            (Some(from), Some(to))
        }
        _ => (None, None),
    };

    WindData {
        direction_degrees,
        is_variable,
        speed_kt,
        gust_kt,
        variable_from_degrees,
        variable_to_degrees,
    }
}
