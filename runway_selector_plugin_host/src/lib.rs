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
//! [`PluginHandle`] holding the channel. The handle owns the child: dropping
//! it without calling [`PluginHandle::shutdown`] still kills the subprocess
//! best-effort so a host panic does not leak processes.

use std::{
    net::TcpListener,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    time::Duration,
};

use std::io;

use runway_selector_core::area_config::{AreaManifest, Runtime};
use semver::Version;
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, ChildStderr, ChildStdout},
    task::JoinHandle,
    time::sleep,
};
use tonic::transport::{Channel, Endpoint};
use tonic_health::pb::{
    HealthCheckRequest, health_check_response::ServingStatus, health_client::HealthClient,
};

/// Default per-step wait between health-check polls.
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Hard ceiling on how long we wait for a plugin to report `SERVING`.
const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
/// Time between SIGTERM and the fallback SIGKILL during graceful shutdown.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

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
    #[error(
        "Plugin {area_name:?} exited during startup with {status} before reporting SERVING.{stderr_hint}",
        stderr_hint = stderr_hint(stderr_tail)
    )]
    StartupExit {
        area_name: String,
        status: ExitStatus,
        stderr_tail: String,
    },
    #[error("Plugin {area_name:?} requires host version >= {required} but this host is {current}")]
    IncompatibleHostVersion {
        area_name: String,
        required: Version,
        current: Version,
    },
}

fn stderr_hint(stderr_tail: &str) -> String {
    let trimmed = stderr_tail.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!(" Plugin stderr (tail):\n{trimmed}")
    }
}

pub type PluginResult<T> = Result<T, PluginError>;

/// A live plugin subprocess plus the gRPC channel to it.
///
/// The handle owns the child process and the tasks forwarding its stdio into
/// `tracing`. Dropping the handle kills the child best-effort with SIGKILL;
/// prefer [`PluginHandle::shutdown`] for a graceful SIGTERM-then-SIGKILL exit.
pub struct PluginHandle {
    pub area_name: String,
    pub port: u16,
    pub channel: Channel,
    child: Option<Child>,
    stdout_task: Option<JoinHandle<()>>,
    stderr_task: Option<JoinHandle<()>>,
}

impl PluginHandle {
    /// Gracefully terminate the child: send SIGTERM (or `start_kill` on
    /// Windows), wait up to [`SHUTDOWN_GRACE`], then SIGKILL on timeout.
    pub async fn shutdown(mut self) -> PluginResult<ExitStatus> {
        let Some(mut child) = self.child.take() else {
            return Err(PluginError::Io(io::Error::other(
                "PluginHandle already shut down",
            )));
        };

        send_graceful_terminate(&child);

        let status = match tokio::time::timeout(SHUTDOWN_GRACE, child.wait()).await {
            Ok(res) => res?,
            Err(_) => {
                tracing::warn!(
                    area = %self.area_name,
                    "Plugin did not exit within {:?} of SIGTERM; sending SIGKILL",
                    SHUTDOWN_GRACE
                );
                child.start_kill()?;
                child.wait().await?
            }
        };

        if let Some(t) = self.stdout_task.take() {
            let _ = t.await;
        }
        if let Some(t) = self.stderr_task.take() {
            let _ = t.await;
        }
        Ok(status)
    }
}

impl Drop for PluginHandle {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            // shutdown() wasn't called — fall back to SIGKILL so a host panic
            // does not leak the subprocess. tokio's Child does not kill on
            // drop by default; we set `kill_on_drop(true)` on the command too
            // as a belt-and-braces.
            let _ = child.start_kill();
        }
    }
}

#[cfg(unix)]
fn send_graceful_terminate(child: &Child) {
    let Some(pid) = child.id() else {
        return;
    };
    // SAFETY: libc::kill is a thin wrapper around the kill(2) syscall. The
    // PID came from a Child we own; SIGTERM is a signal number constant.
    let pid_signed = pid as i32;
    let result = unsafe { libc::kill(pid_signed, libc::SIGTERM) };
    if result != 0 {
        let err = io::Error::last_os_error();
        tracing::debug!(pid = pid_signed, error = ?err, "SIGTERM to plugin failed");
    }
}

#[cfg(not(unix))]
fn send_graceful_terminate(_child: &Child) {
    // On Windows there is no SIGTERM; the SIGKILL fallback in `shutdown` is
    // the only termination path. Plugins on Windows should treat the channel
    // closing as the shutdown signal.
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
            c.args(["exec", interpreter, "--", interpreter]);
            // Deno requires explicit permission grants; without them, a script
            // launched non-interactively just fails when it tries to open a
            // socket. Areas run sandboxed under the host already (separate
            // subprocess, ephemeral lifetime), so grant the lot.
            if matches!(manifest.runtime, Runtime::Deno) {
                c.arg("run").arg("-A");
            }
            c.arg(&entry);
            c
        }
    };

    cmd.env("RUNWAY_SELECTOR_PORT", port.to_string())
        .env("RUNWAY_SELECTOR_AREA_DIR", area_dir)
        .current_dir(area_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

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

/// Check that the manifest's `min_core_version` (if any) is satisfied by the
/// supplied host version. Pure — no I/O. Returns
/// [`PluginError::IncompatibleHostVersion`] when the host is too old.
pub fn check_host_compatibility(
    manifest: &AreaManifest,
    host_version: &Version,
) -> PluginResult<()> {
    let Some(required) = manifest.min_core_version.as_ref() else {
        return Ok(());
    };
    if host_version < required {
        return Err(PluginError::IncompatibleHostVersion {
            area_name: manifest.name.clone(),
            required: required.clone(),
            current: host_version.clone(),
        });
    }
    Ok(())
}

/// Spawn the plugin, wait for `grpc.health.v1` to report `SERVING`, and
/// return the handle. Uses [`DEFAULT_STARTUP_TIMEOUT`] for the health wait.
///
/// Verifies `manifest.min_core_version` against `host_version` before
/// spawning — an incompatible plugin is reported, not run.
pub async fn spawn_plugin(
    manifest: &AreaManifest,
    area_dir: &Path,
    host_version: &Version,
) -> PluginResult<PluginHandle> {
    spawn_plugin_with_timeout(manifest, area_dir, host_version, DEFAULT_STARTUP_TIMEOUT).await
}

pub async fn spawn_plugin_with_timeout(
    manifest: &AreaManifest,
    area_dir: &Path,
    host_version: &Version,
    startup_timeout: Duration,
) -> PluginResult<PluginHandle> {
    check_host_compatibility(manifest, host_version)?;

    let port = pick_free_port()?;
    let mut cmd = build_command(manifest, area_dir, port)?;
    let mut child = cmd.spawn()?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let area_name = manifest.name.clone();
    let stderr_tail = StderrTail::default();

    let stdout_task = stdout.map(|s| spawn_stdout_forwarder(area_name.clone(), s));
    let stderr_task =
        stderr.map(|s| spawn_stderr_forwarder(area_name.clone(), s, stderr_tail.clone()));

    let endpoint: Endpoint = format!("http://127.0.0.1:{port}").parse()?;

    let channel = match wait_for_serving(&mut child, endpoint, startup_timeout).await {
        Ok(channel) => channel,
        Err(PluginError::StartupExit {
            area_name: name,
            status,
            stderr_tail: _,
        }) => {
            // Drain captured stderr to attach to the error before returning.
            if let Some(t) = stderr_task {
                let _ = t.await;
            }
            if let Some(t) = stdout_task {
                let _ = t.await;
            }
            return Err(PluginError::StartupExit {
                area_name: name,
                status,
                stderr_tail: stderr_tail.snapshot(),
            });
        }
        Err(other) => {
            // Best-effort cleanup; child is killed by kill_on_drop when
            // `child` is dropped at function-return.
            let _ = child.start_kill();
            return Err(other);
        }
    };

    Ok(PluginHandle {
        area_name,
        port,
        channel,
        child: Some(child),
        stdout_task,
        stderr_task,
    })
}

async fn wait_for_serving(
    child: &mut Child,
    endpoint: Endpoint,
    timeout: Duration,
) -> PluginResult<Channel> {
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        // First check whether the child has exited — `try_wait` is
        // non-blocking. If it has, polling the port is hopeless.
        if let Some(status) = child.try_wait()? {
            return Err(PluginError::StartupExit {
                area_name: String::new(),
                status,
                stderr_tail: String::new(),
            });
        }

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

/// A ring-buffered tail of the child's stderr, used to attach diagnostic
/// context to startup-failure errors. Cheaply cloneable.
#[derive(Clone, Default)]
struct StderrTail {
    inner: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<String>>>,
}

impl StderrTail {
    const CAPACITY: usize = 20;

    fn push(&self, line: String) {
        let mut g = self.inner.lock().unwrap();
        if g.len() == Self::CAPACITY {
            g.pop_front();
        }
        g.push_back(line);
    }

    fn snapshot(&self) -> String {
        let g = self.inner.lock().unwrap();
        g.iter().cloned().collect::<Vec<_>>().join("\n")
    }
}

fn spawn_stdout_forwarder(area_name: String, stdout: ChildStdout) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        loop {
            match reader.next_line().await {
                Ok(Some(line)) => tracing::info!(target: "plugin", area = %area_name, "{line}"),
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!(area = %area_name, error = ?e, "Error reading plugin stdout");
                    break;
                }
            }
        }
    })
}

fn spawn_stderr_forwarder(
    area_name: String,
    stderr: ChildStderr,
    tail: StderrTail,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        loop {
            match reader.next_line().await {
                Ok(Some(line)) => {
                    tracing::warn!(target: "plugin", area = %area_name, "{line}");
                    tail.push(line);
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!(area = %area_name, error = ?e, "Error reading plugin stderr");
                    break;
                }
            }
        }
    })
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
    fn build_command_for_deno_passes_allow_all_to_run() {
        if !mise_available() {
            return;
        }
        let dir = tempdir().unwrap();
        write_entry(dir.path(), "server.ts");
        let manifest = dummy_manifest("deno_area", Runtime::Deno, "server.ts");

        let cmd = build_command(&manifest, dir.path(), 50_000).unwrap();
        let args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            &args[0..6],
            &["exec", "deno", "--", "deno", "run", "-A"],
            "deno entries need explicit permissions or they fail non-interactively",
        );
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

    #[test]
    fn host_compatibility_passes_when_no_minimum_declared() {
        let manifest = dummy_manifest("x", Runtime::Rust, "x");
        check_host_compatibility(&manifest, &Version::new(0, 0, 1)).unwrap();
    }

    #[test]
    fn host_compatibility_passes_when_current_satisfies_minimum() {
        let mut manifest = dummy_manifest("x", Runtime::Rust, "x");
        manifest.min_core_version = Some(Version::new(1, 0, 0));
        check_host_compatibility(&manifest, &Version::new(1, 2, 3)).unwrap();
    }

    #[test]
    fn host_compatibility_fails_when_current_too_old() {
        let mut manifest = dummy_manifest("x", Runtime::Rust, "x");
        manifest.min_core_version = Some(Version::new(2, 0, 0));
        let err = check_host_compatibility(&manifest, &Version::new(1, 9, 9)).unwrap_err();
        assert!(matches!(err, PluginError::IncompatibleHostVersion { .. }));
    }
}
