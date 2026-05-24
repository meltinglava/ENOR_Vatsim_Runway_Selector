use std::{net::TcpListener, time::Duration};

use indexmap::IndexMap;
use reqwest::Client;
use tokio::{process::Child, time::sleep};
use tracing::{debug, info, warn};

use crate::{config::PluginConfig, error::ApplicationResult};
use runway_selector_protocol::{
    AirportInfo, AtisRequest, AtisResponse, PluginAirportsResponse, RunwaySelectionRequest,
    RunwaySelectionResponse,
};

/// A running plugin instance with its port and handled airports.
struct RunningPlugin {
    name: String,
    port: u16,
    airports: Vec<String>,
    #[allow(dead_code)] // held to keep the process alive
    process: Child,
}

/// Manages all plugin processes for the current session.
pub(crate) struct PluginManager {
    plugins: Vec<RunningPlugin>,
    client: Client,
    #[allow(dead_code)]
    parent_port: u16,
}

impl PluginManager {
    /// Start all configured plugins and wait for them to become healthy.
    pub async fn start(configs: &[&PluginConfig], parent_port: u16) -> ApplicationResult<Self> {
        let client = Client::builder().timeout(Duration::from_secs(30)).build()?;

        let mut plugins = Vec::new();

        for cfg in configs {
            match start_plugin(cfg, parent_port, &client).await {
                Ok(plugin) => {
                    info!(
                        plugin = %cfg.name,
                        port = plugin.port,
                        airports = ?plugin.airports,
                        "Plugin started"
                    );
                    plugins.push(plugin);
                }
                Err(e) => {
                    warn!(plugin = %cfg.name, error = %e, "Failed to start plugin – skipping");
                }
            }
        }

        Ok(Self {
            plugins,
            client,
            parent_port,
        })
    }

    /// Return the plugin (if any) that declared it handles the given ICAO.
    fn plugin_for_airport(&self, icao: &str) -> Option<&RunningPlugin> {
        self.plugins
            .iter()
            .find(|p| p.airports.iter().any(|a| a == icao))
    }

    /// POST to the first plugin that handles any of the supplied ATIS entries.
    ///
    /// Returns `None` when no plugin covers any of those airports.
    pub async fn call_atis(&self, request: &AtisRequest) -> Option<AtisResponse> {
        // Group entries by the plugin that handles each airport.
        // A single request is sent per plugin containing only its airports.
        for plugin in &self.plugins {
            let relevant: Vec<_> = request
                .atis_entries
                .iter()
                .filter(|e| plugin.airports.contains(&e.airport_icao))
                .cloned()
                .collect();
            if relevant.is_empty() {
                continue;
            }
            let relevant_airports: Vec<_> = request
                .airports
                .iter()
                .filter(|a| plugin.airports.contains(&a.icao))
                .cloned()
                .collect();

            let sub_request = AtisRequest {
                atis_entries: relevant,
                airports: relevant_airports,
            };

            let url = format!("http://127.0.0.1:{}/atis", plugin.port);
            match self.client.post(&url).json(&sub_request).send().await {
                Ok(resp) if resp.status().is_success() => match resp.json::<AtisResponse>().await {
                    Ok(response) => return Some(response),
                    Err(e) => {
                        warn!(plugin = %plugin.name, error = %e, "Failed to deserialize /atis response")
                    }
                },
                Ok(resp) => {
                    warn!(plugin = %plugin.name, status = %resp.status(), "Plugin /atis returned error")
                }
                Err(e) => warn!(plugin = %plugin.name, error = %e, "Plugin /atis call failed"),
            }
        }
        None
    }

    /// Call the responsible plugin's `POST /runways` endpoint for one airport.
    ///
    /// Returns `None` when no plugin handles this airport.
    pub async fn call_runway_selection(
        &self,
        request: &RunwaySelectionRequest,
    ) -> Option<RunwaySelectionResponse> {
        let plugin = self.plugin_for_airport(&request.airport.icao)?;
        let url = format!("http://127.0.0.1:{}/runways", plugin.port);
        match self.client.post(&url).json(request).send().await {
            Ok(resp) if resp.status().is_success() => {
                match resp.json::<RunwaySelectionResponse>().await {
                    Ok(r) => Some(r),
                    Err(e) => {
                        warn!(plugin = %plugin.name, error = %e, "Failed to deserialize /runways response");
                        None
                    }
                }
            }
            Ok(resp) => {
                warn!(plugin = %plugin.name, status = %resp.status(), "Plugin /runways returned error");
                None
            }
            Err(e) => {
                warn!(plugin = %plugin.name, error = %e, "Plugin /runways call failed");
                None
            }
        }
    }

    /// True if any loaded plugin declared it handles the given ICAO.
    pub fn has_plugin_for(&self, icao: &str) -> bool {
        self.plugin_for_airport(icao).is_some()
    }

    /// Build an `AirportInfo` map keyed by ICAO so callers can quickly look up
    /// info to pass to plugin requests.
    #[allow(dead_code)]
    pub fn airports_map<'a>(
        airports: impl Iterator<Item = &'a AirportInfo>,
    ) -> IndexMap<String, AirportInfo> {
        airports.map(|a| (a.icao.clone(), a.clone())).collect()
    }
}

async fn start_plugin(
    cfg: &PluginConfig,
    parent_port: u16,
    client: &Client,
) -> ApplicationResult<RunningPlugin> {
    let plugin_port = find_free_port()?;

    let process = spawn_plugin_process(cfg, plugin_port, parent_port).await?;

    wait_for_health(cfg, plugin_port, client).await?;

    let airports = fetch_airports(cfg, plugin_port, client).await?;

    Ok(RunningPlugin {
        name: cfg.name.clone(),
        port: plugin_port,
        airports,
        process,
    })
}

async fn spawn_plugin_process(
    cfg: &PluginConfig,
    plugin_port: u16,
    parent_port: u16,
) -> ApplicationResult<Child> {
    let mut cmd_parts = cfg.command.split_whitespace();
    let exe = cmd_parts.next().expect("plugin command is empty");
    let args: Vec<&str> = cmd_parts.collect();

    let mut command = if let Some(runtime) = &cfg.runtime {
        let mut c = tokio::process::Command::new("mise");
        c.arg("exec").arg(runtime).arg("--").arg(exe);
        c
    } else {
        tokio::process::Command::new(exe)
    };

    for arg in &args {
        command.arg(arg);
    }

    if let Some(dir) = &cfg.working_dir {
        command.current_dir(dir);
    }

    command
        .env("ES_RUNWAY_SELECTOR_PORT", parent_port.to_string())
        .env("ES_RUNWAY_SELECTOR_PLUGIN_PORT", plugin_port.to_string())
        .kill_on_drop(true);

    #[cfg(target_os = "windows")]
    {
        use std::process::Stdio;
        // Detach from the parent console so the console window can close when the
        // runway selector exits. Without this, a plugin launched from Explorer keeps
        // the console handle alive past the parent's exit and the window (with the
        // profile selection prompt still on screen) stays open forever.
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW);
    }

    #[cfg(not(target_os = "windows"))]
    {
        use std::process::Stdio;
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }

    debug!(plugin = %cfg.name, port = plugin_port, parent_port, "Spawning plugin");
    Ok(command.spawn()?)
}

async fn wait_for_health(cfg: &PluginConfig, port: u16, client: &Client) -> ApplicationResult<()> {
    let url = format!("http://127.0.0.1:{}/health", port);
    let max_attempts = 30;
    for attempt in 1..=max_attempts {
        if let Ok(resp) = client.get(&url).send().await
            && resp.status().is_success()
        {
            debug!(plugin = %cfg.name, port, "Plugin is healthy");
            return Ok(());
        }
        if attempt < max_attempts {
            sleep(Duration::from_millis(200)).await;
        }
    }
    Err(crate::error::ApplicationError::PluginStartupError(format!(
        "Plugin '{}' did not become healthy within {} attempts",
        cfg.name, max_attempts
    )))
}

async fn fetch_airports(
    cfg: &PluginConfig,
    port: u16,
    client: &Client,
) -> ApplicationResult<Vec<String>> {
    let url = format!("http://127.0.0.1:{}/airports", port);
    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        warn!(plugin = %cfg.name, "GET /airports returned non-success; plugin handles no airports");
        return Ok(Vec::new());
    }
    let body: PluginAirportsResponse = resp.json().await?;
    Ok(body.airports)
}

fn find_free_port() -> ApplicationResult<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}
