//! Plugin lifecycle: spawn an area's subprocess, wait for it to come up,
//! talk to it over gRPC, shut it down.
//!
//! Each area ships a `manifest.toml` declaring a [`Runtime`] and an `entry`
//! path. For Rust areas we exec the entry directly; for Python / Node / Deno
//! we delegate to [`mise`](https://mise.jdx.dev/) so end users do not have to
//! install language runtimes manually.
//!
//! Once the child is alive, we wait until its
//! `grpc.health.v1.Health/Check` reports `SERVING` and then hand back a
//! [`PluginHandle`] holding the channel.

use std::{
    net::TcpListener,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use std::io;

use runway_selector_core::area_config::{AreaManifest, Runtime};
use thiserror::Error;
use tokio::{process::Child, time::sleep};
use tonic::transport::{Channel, Endpoint};
use tonic_health::pb::{
    HealthCheckRequest, health_check_response::ServingStatus, health_client::HealthClient,
};

/// Default per-step wait between health-check polls.
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Hard ceiling on how long we wait for a plugin to report `SERVING`.
const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("Failed to bind a free local port: {0}")]
    Bind(String),
    #[error("`mise` is required for runtime {runtime:?} but was not found on PATH")]
    MiseMissing { runtime: Runtime },
    #[error("Plugin entry point does not exist: {0}")]
    EntryMissing(PathBuf),
    #[error("gRPC connection to plugin failed: {0}")]
    Transport(#[from] tonic::transport::Error),
    #[error("gRPC health check failed: {0}")]
    Health(#[from] tonic::Status),
    #[error("Timed out after {0:?} waiting for plugin to report SERVING")]
    StartupTimeout(Duration),
}

pub type PluginResult<T> = Result<T, PluginError>;

/// A live plugin subprocess plus the gRPC channel to it. Drop semantics:
/// dropping a `PluginHandle` only releases the channel — call
/// [`PluginHandle::shutdown`] explicitly to terminate the child.
pub struct PluginHandle {
    pub area_name: String,
    pub port: u16,
    pub channel: Channel,
    pub child: Child,
}

impl PluginHandle {
    /// Gracefully terminate the child: send SIGTERM (or `kill` on Windows),
    /// wait, and return the exit status.
    pub async fn shutdown(mut self) -> PluginResult<std::process::ExitStatus> {
        // tokio::process::Child::kill sends SIGKILL on Unix; SIGTERM would be
        // nicer but requires unsafe libc on stable. Keep it simple — areas
        // are expected to handle abrupt termination since this is a desktop
        // tool the user can close at any time.
        self.child.start_kill()?;
        Ok(self.child.wait().await?)
    }
}

/// Build (but do not yet spawn) the command that runs a plugin's entry
/// point. Splits out for unit testing — see [`spawn_plugin`] for the full
/// spawn + handshake.
pub fn build_command(
    manifest: &AreaManifest,
    area_dir: &Path,
    port: u16,
) -> PluginResult<tokio::process::Command> {
    let plugin_dir = area_dir.join("plugin");
    let entry = plugin_dir.join(&manifest.entry);

    if !entry.exists() {
        return Err(PluginError::EntryMissing(entry));
    }

    let mut cmd = match manifest.runtime {
        Runtime::Rust => tokio::process::Command::new(&entry),
        Runtime::Python | Runtime::Node | Runtime::Deno => {
            let mise = which::which("mise").map_err(|_| PluginError::MiseMissing {
                runtime: manifest.runtime,
            })?;
            let interpreter = match manifest.runtime {
                Runtime::Python => "python",
                Runtime::Node => "node",
                Runtime::Deno => "deno",
                Runtime::Rust => unreachable!(),
            };
            let mut c = tokio::process::Command::new(mise);
            // `mise exec <runtime> -- <interpreter> <entry>`
            c.args(["exec", interpreter, "--", interpreter]).arg(&entry);
            c
        }
    };

    cmd.env("RUNWAY_SELECTOR_PORT", port.to_string())
        .env("RUNWAY_SELECTOR_AREA_DIR", area_dir)
        .current_dir(area_dir)
        .stdin(Stdio::null());

    Ok(cmd)
}

/// Reserve a free local TCP port by binding to 0 and reading the assigned
/// port back. The bind is released immediately, leaving a small race window
/// before the child re-binds — acceptable for a desktop tool that talks
/// over localhost.
pub fn pick_free_port() -> PluginResult<u16> {
    let listener =
        TcpListener::bind(("127.0.0.1", 0)).map_err(|e| PluginError::Bind(e.to_string()))?;
    let port = listener
        .local_addr()
        .map_err(|e| PluginError::Bind(e.to_string()))?
        .port();
    drop(listener);
    Ok(port)
}

/// Spawn the plugin, wait for `grpc.health.v1` to report `SERVING`, and
/// return the handle. Uses [`DEFAULT_STARTUP_TIMEOUT`] for the health wait.
pub async fn spawn_plugin(manifest: &AreaManifest, area_dir: &Path) -> PluginResult<PluginHandle> {
    spawn_plugin_with_timeout(manifest, area_dir, DEFAULT_STARTUP_TIMEOUT).await
}

pub async fn spawn_plugin_with_timeout(
    manifest: &AreaManifest,
    area_dir: &Path,
    startup_timeout: Duration,
) -> PluginResult<PluginHandle> {
    let port = pick_free_port()?;
    let mut cmd = build_command(manifest, area_dir, port)?;
    let child = cmd.spawn()?;

    let endpoint: Endpoint = format!("http://127.0.0.1:{port}").parse()?;
    let channel = wait_for_serving(endpoint, startup_timeout).await?;

    Ok(PluginHandle {
        area_name: manifest.name.clone(),
        port,
        channel,
        child,
    })
}

async fn wait_for_serving(endpoint: Endpoint, timeout: Duration) -> PluginResult<Channel> {
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        if let Ok(channel) = endpoint.connect().await {
            let mut client = HealthClient::new(channel.clone());
            match client
                .check(HealthCheckRequest {
                    service: String::new(),
                })
                .await
            {
                Ok(resp) if resp.get_ref().status() == ServingStatus::Serving => {
                    return Ok(channel);
                }
                Ok(_) | Err(_) => {}
            }
        }

        if tokio::time::Instant::now() >= deadline {
            return Err(PluginError::StartupTimeout(timeout));
        }
        sleep(HEALTH_POLL_INTERVAL).await;
    }
}

/// Return true if `mise` is on `PATH`. Useful for the first-run wizard so we
/// can prompt the user to bootstrap it before installing non-Rust areas.
pub fn mise_available() -> bool {
    which::which("mise").is_ok()
}

// Re-export the standard health types so callers don't need a direct
// `tonic-health` dependency.
pub use tonic_health::pb as health_pb;

#[cfg(test)]
mod tests {
    use super::*;
    use semver::Version;
    use std::fs;
    use tempfile::tempdir;

    fn dummy_manifest(name: &str, runtime: Runtime, entry: &str) -> AreaManifest {
        AreaManifest {
            name: name.into(),
            version: Version::new(0, 1, 0),
            display_name: name.into(),
            description: None,
            runtime,
            entry: entry.into(),
            supported_icaos: vec![],
            min_core_version: None,
        }
    }

    fn write_entry(dir: &Path, entry: &str) -> PathBuf {
        let plugin = dir.join("plugin");
        fs::create_dir_all(&plugin).unwrap();
        let entry_path = plugin.join(entry);
        fs::write(&entry_path, "").unwrap();
        entry_path
    }

    #[test]
    fn pick_free_port_returns_nonzero() {
        let p = pick_free_port().unwrap();
        assert!(p > 0);
    }

    #[test]
    fn build_command_for_rust_invokes_entry_directly() {
        let dir = tempdir().unwrap();
        let entry_path = write_entry(dir.path(), "area_enor");
        let manifest = dummy_manifest("enor", Runtime::Rust, "area_enor");

        let cmd = build_command(&manifest, dir.path(), 50_000).unwrap();
        let std_cmd: &std::process::Command = cmd.as_std();
        assert_eq!(std_cmd.get_program(), entry_path.as_os_str());
        assert!(std_cmd.get_args().count() == 0);
    }

    #[test]
    fn build_command_fails_when_entry_missing() {
        let dir = tempdir().unwrap();
        let manifest = dummy_manifest("enor", Runtime::Rust, "does_not_exist");
        let err = build_command(&manifest, dir.path(), 50_000).unwrap_err();
        assert!(matches!(err, PluginError::EntryMissing(_)));
    }

    #[test]
    fn build_command_for_python_routes_through_mise_when_available() {
        if !mise_available() {
            // Can't exercise the happy path without mise installed; the
            // failure case is covered by the next test.
            return;
        }
        let dir = tempdir().unwrap();
        write_entry(dir.path(), "server.py");
        let manifest = dummy_manifest("py_area", Runtime::Python, "server.py");

        let cmd = build_command(&manifest, dir.path(), 50_000).unwrap();
        let std_cmd: &std::process::Command = cmd.as_std();
        let program = std_cmd.get_program().to_string_lossy();
        assert!(program.ends_with("mise"), "expected mise, got {program}");

        let args: Vec<String> = std_cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args[0], "exec");
        assert_eq!(args[1], "python");
        assert_eq!(args[2], "--");
        assert_eq!(args[3], "python");
    }

    #[test]
    fn build_command_for_python_errors_when_mise_missing() {
        if mise_available() {
            return; // can't fake "mise missing" if it's installed
        }
        let dir = tempdir().unwrap();
        write_entry(dir.path(), "server.py");
        let manifest = dummy_manifest("py_area", Runtime::Python, "server.py");

        let err = build_command(&manifest, dir.path(), 50_000).unwrap_err();
        assert!(matches!(
            err,
            PluginError::MiseMissing {
                runtime: Runtime::Python
            }
        ));
    }
}
