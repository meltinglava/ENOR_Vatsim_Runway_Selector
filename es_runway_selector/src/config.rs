use std::{
    borrow::Cow,
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::LazyLock,
    time::SystemTime,
};

use config::{Config, ConfigError};
use directories::{BaseDirs, ProjectDirs, UserDirs};
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use jiff::{
    civil::{DateTime, datetime},
    tz::TimeZone,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use sysinfo::{ProcessesToUpdate, System};
use tokio::process::Command;
use tracing::{debug, warn};
use tracing_unwrap::ResultExt;
use walkdir::WalkDir;

use crate::{
    airport::RunwayInUseSource, airports::Airports, error::ApplicationResult, runway::RunwayUse,
};

#[derive(Debug)]
pub(crate) struct ESConfig {
    euroscope_config_folder: PathBuf,
    enor_file_prefix: String,
    #[allow(dead_code)] // used in tests
    config_file_path: PathBuf,
    config: Configurable,
    app_launchers: IndexSet<AppLauncher>,
}

#[derive(Debug, Serialize, Deserialize)]
#[skip_serializing_none]
struct Configurable {
    ignore_airports: IndexSet<String>,
    default_runways: IndexMap<String, u8>,
    euroscope_config_folder: Option<PathBuf>,
    euroscope_executable_path: Option<IndexMap<String, PathBuf>>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
struct AppLauncher {
    name: String,
    args: Vec<String>,
    prf: Option<PathBuf>,
}

impl Configurable {
    fn find_from_config(&self) -> Option<(PathBuf, String)> {
        let path = self.euroscope_config_folder.as_ref()?;
        search_for_ese_with_possibilities(&[path])
    }
}

impl ESConfig {
    pub fn find_euroscope_config_folder(clean_config: bool) -> Option<Self> {
        let (mut config, config_file_path) = setup_configuration(clean_config).unwrap_or_log();
        let (sct_path, enor_file_prefix) = search_for_euroscope_newest_sct_file()
            .or_else(|| config.find_from_config())
            .or_else(|| query_user_euroscope_config_folder(&mut config, &config_file_path))?;

        let app_launchers = get_app_launchers(&config_file_path);

        Some(Self {
            euroscope_config_folder: sct_path,
            enor_file_prefix,
            config,
            config_file_path,
            app_launchers,
        })
    }

    pub fn get_ignore_airports(&self) -> &IndexSet<String> {
        &self.config.ignore_airports
    }

    pub fn get_default_runways(&self) -> &IndexMap<String, u8> {
        &self.config.default_runways
    }

    pub fn get_sct_file_path(&self) -> PathBuf {
        self.euroscope_config_folder
            .join(format!("{}.sct", self.enor_file_prefix))
    }

    pub fn get_rwy_file_path(&self) -> PathBuf {
        self.euroscope_config_folder
            .join(format!("{}.rwy", self.enor_file_prefix))
    }

    pub fn write_runways_to_euroscope_rwy_file(
        &self,
        airports: &Airports,
    ) -> ApplicationResult<()> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(false)
            .truncate(false)
            .open(self.get_rwy_file_path())?;

        let start_of_file = read_active_airport(&mut file)?;
        file.seek(SeekFrom::Start(0))?;
        file.set_len(0)?;
        write_runway_file(&mut file, airports, &start_of_file)
    }

    pub async fn run_apps(&self, euroscope_ready: bool) {
        let mut already_running = IndexMap::new();
        for app in &self.app_launchers {
            if (app.name == "EuroScope") == euroscope_ready {
                let entry = already_running
                    .entry(app.name.clone())
                    .or_insert_with(|| is_process_running(&app.name));
                if *entry {
                    debug!("{} is already running, skipping launch", app.name);
                    continue;
                }
                let app = app.clone();
                let exe_path = self
                    .config
                    .euroscope_executable_path
                    .clone()
                    .unwrap_or_default()
                    .get(&app.name)
                    .cloned()
                    .or_else(|| find_exe_path(&app.name));
                let exe_path = match exe_path {
                    Some(p) => p,
                    None => {
                        warn!("Could not find executable path for {}", app.name);
                        continue;
                    }
                };
                let prf_path = self.euroscope_config_folder.clone();
                tokio::spawn(async move {
                    app.run(&exe_path, prf_path).await;
                });
            }
        }
    }
}

fn is_process_running(name: &str) -> bool {
    let mut sys = System::new_all();
    sys.refresh_processes(ProcessesToUpdate::All, true);

    let lower = name.to_lowercase();
    sys.processes_by_name(OsStr::new(name))
        .chain(sys.processes_by_name(OsStr::new(&lower)))
        .next()
        .is_some()
}

fn find_exe_path(name: &str) -> Option<PathBuf> {
    directories::BaseDirs::new()
        .map(|bd| {
            bd.config_dir()
                .join("Microsoft\\Windows\\Start Menu\\Programs")
        })
        .and_then(|start_menu| {
            WalkDir::new(&start_menu)
                .max_depth(3)
                .into_iter()
                .filter_map(Result::ok)
                .find(|e| {
                    let file_name = e.file_name().to_string_lossy();
                    let Some((exe_name, extention)) = file_name.split_once('.') else {
                        return false;
                    };
                    exe_name == name && ["lnk", "exe"].contains(&extention)
                })
                .map(|e| e.path().to_path_buf())
        })
}

impl AppLauncher {
    /// Lanch the application detached from the current process
    async fn run(&self, exe_path: &Path, prf_folder: PathBuf) {
        #[cfg(target_os = "windows")]
        let mut command = {
            let is_lnk = exe_path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("lnk"))
                .unwrap_or(false);

            if is_lnk {
                // cmd.exe /c start "" "C:\path\to\shortcut.lnk" [args...]
                let mut cmd = Command::new("cmd");
                cmd.arg("/c")
                    .arg("start")
                    .arg("") // window title placeholder for `start`
                    .arg(exe_path); // the .lnk (or any shell-handled file)
                cmd
            } else {
                // Normal executable: launch directly
                Command::new(exe_path)
            }
        };

        #[cfg(not(target_os = "windows"))]
        let mut command = { Command::new(exe_path) };

        // Common args
        for arg in &self.args {
            command.arg(arg);
        }

        if let Some(prf) = &self.prf {
            command.arg((prf_folder.join(prf)).to_string_lossy().to_string());
        }

        #[cfg(target_os = "windows")]
        {
            const DETACHED_PROCESS: u32 = 0x00000008;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
            command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
        }

        #[cfg(not(target_os = "windows"))]
        {
            use std::process::Stdio;
            command
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .stdin(Stdio::null());
        }

        if let Err(e) = command.spawn() {
            warn!(
                "Failed to launch application {}: {:?}",
                exe_path.to_string_lossy(),
                e
            );
        }
    }
}

fn get_app_launchers(config_file_path: &Path) -> IndexSet<AppLauncher> {
    let app_launchers_file_path = config_file_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("app_launchers.toml");

    if !app_launchers_file_path.exists() {
        debug!(
            "App launchers config file does not exist at {:?}",
            app_launchers_file_path
        );
        return IndexSet::new();
    }

    let raw_app_launchers_file = fs::read_to_string(&app_launchers_file_path).unwrap_or_log();

    let toml_file: toml::Value = toml::from_str(&raw_app_launchers_file).unwrap_or_log();

    let toml::Value::Table(map) = &toml_file else {
        warn!(
            "App launchers config file is not a table, it is: {:?}",
            toml_file
        );
        return IndexSet::new();
    };

    let Some(array) = map.get("executable") else {
        warn!(
            "App launchers config file is not a table, it is: {:?}",
            toml_file
        );
        return IndexSet::new();
    };

    let toml::Value::Array(executables) = array else {
        warn!(
            "App launchers config file 'executable' is not an array, it is: {:?}",
            array
        );
        return IndexSet::new();
    };

    let mut app_launchers = IndexSet::new();

    for exe in executables {
        let toml::Value::Table(exe_table) = exe else {
            warn!("App launcher entry is not a table, it is: {:?}", exe);
            continue;
        };

        let name = match exe_table.get("name") {
            Some(toml::Value::String(s)) => s.clone(),
            _ => {
                warn!(
                    "App launcher entry missing 'name' field or it is not a string: {:?}",
                    exe_table
                );
                continue;
            }
        };

        let args = match exe_table.get("args") {
            Some(toml::Value::Array(arr)) => arr
                .iter()
                .filter_map(|v| {
                    if let toml::Value::String(s) = v {
                        Some(s.clone())
                    } else {
                        None
                    }
                })
                .collect(),
            _ => Vec::new(),
        };

        let prf = match exe_table.get("prf") {
            Some(toml::Value::String(s)) => Some(PathBuf::from(s)),
            _ => None,
        };

        app_launchers.insert(AppLauncher { name, args, prf });
    }

    app_launchers
}

#[cfg(not(target_env = "musl"))]
fn query_user_euroscope_config_folder<P: AsRef<Path>>(
    config: &mut Configurable,
    config_file_path: &P,
) -> Option<(PathBuf, String)> {
    let bd = BaseDirs::new()?;

    let possibility = rfd::FileDialog::new()
        .set_title("Select Euroscope sector file folder. The folder containing the ese file")
        .set_directory(bd.config_dir())
        .add_filter("Euroscope Configuration", &["sct", "rwy"])
        .pick_folder()
        .inspect(|path: &PathBuf| {
            config.euroscope_config_folder = Some(path.clone());
            fs::write(config_file_path, toml::to_string_pretty(&config).unwrap())
                .expect("Failed to write config file");
        })?;
    search_for_ese_with_possibilities(&[possibility])
}

#[cfg(target_env = "musl")]
fn query_user_euroscope_config_folder<P: AsRef<Path>>(
    config: &mut Configurable,
    config_file_path: &P,
) -> Option<(PathBuf, String)> {
    use crossterm::terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    };
    use ratatui::{
        Terminal,
        backend::CrosstermBackend,
        crossterm::event::{self, Event, KeyCode},
    };
    use ratatui_explorer::{File, FileExplorer};

    use std::io::Stdout;

    fn pick_folder() -> io::Result<Option<PathBuf>> {
        use crossterm::ExecutableCommand;

        enable_raw_mode()?;
        io::stdout().execute(EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(io::stdout());
        let mut terminal: Terminal<CrosstermBackend<Stdout>> = Terminal::new(backend)?;

        let mut explorer = FileExplorer::new()?;
        // Start in XDG config dir (or fall back to home) using `directories`
        if let Some(bd) = BaseDirs::new() {
            let start = bd.config_dir();
            let _ = explorer.set_cwd(start);
        }

        let chosen = loop {
            terminal.draw(|f| {
                let area = f.area();
                f.render_widget(&explorer.widget(), area);
            })?;

            let event = event::read()?;
            if let Event::Key(key) = &event
                && key.code == KeyCode::Enter
            {
                let current: &File = explorer.current();
                let path = if current.is_dir() {
                    current.path().to_path_buf()
                } else {
                    current
                        .path()
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| PathBuf::from("/"))
                };
                break Some(path);
            }
            let _ = explorer.handle(&event);
        };

        disable_raw_mode()?;
        io::stdout().execute(LeaveAlternateScreen)?;
        Ok(chosen)
    }

    let selected = match pick_folder() {
        Ok(Some(p)) => p,
        Ok(None) => return None,
        Err(err) => {
            eprintln!("Error in file explorer: {err}");
            return None;
        }
    };

    config.euroscope_config_folder = Some(selected.clone());
    if let Ok(serialized) = toml::to_string_pretty(&config)
        && let Err(e) = fs::write(config_file_path, serialized)
    {
        eprintln!("Failed to write config file: {e}");
    }

    search_for_ese_with_possibilities(&[selected])
}

#[allow(unstable_name_collisions)] // `intersperse_with` is but we can update itertools once it stabilizes
pub fn read_active_airport<T: Read>(rwy_file: &mut T) -> io::Result<String> {
    let reader = BufReader::new(rwy_file);

    reader
        .lines()
        .take_while(|l| match l {
            Ok(l) => l.starts_with("ACTIVE_AIRPORT:"),
            Err(_) => false,
        })
        .intersperse_with(|| Ok("\n".to_string()))
        .collect::<io::Result<String>>()
}

fn setup_configuration(clean_config: bool) -> Result<(Configurable, PathBuf), ConfigError> {
    let config_dir = ProjectDirs::from("", "meltinglava", "es_runway_selector")
        .expect("Failed to get project directories")
        .config_dir()
        .to_path_buf();

    let mut raw_config_file = Cow::Borrowed(include_str!("../config.toml"));
    let config_file = config_dir.join("config.toml");
    if !config_file.exists() {
        std::fs::create_dir_all(&config_dir).expect("Failed to create config directory");
        std::fs::write(&config_file, raw_config_file.as_bytes())
            .expect("Failed to create config file");
    }
    let configurable = Config::builder()
        .add_source(config::File::from(config_file.clone()).required(true))
        .build()
        .expect("Failed to build configuration")
        .try_deserialize::<Configurable>()?;
    if clean_config {
        if let Some(path) = &configurable.euroscope_config_folder {
            raw_config_file = format!(
                "euroscope_config_folder = '{}'\n\n{}",
                path.to_string_lossy(),
                raw_config_file
            )
            .into();
        }
        fs::write(&config_file, raw_config_file.as_bytes())
            .expect("Failed to write cleaned config file");
        self::setup_configuration(false)
    } else {
        Ok((configurable, config_file))
    }
}

fn write_runway_file<T: Write>(
    rwy_file: &mut T,
    airports: &Airports,
    start_of_file: &str,
) -> ApplicationResult<()> {
    let mut writer = BufWriter::new(rwy_file);
    writeln!(writer, "{}", start_of_file)?;

    for airport in airports.airports.values() {
        if airport.runways.is_empty() {
            warn!("No runways for airport {}", airport.icao);
            continue;
        }

        for selection_method in RunwayInUseSource::default_sort_order() {
            let selection = match airport.runways_in_use.get(&selection_method) {
                None => continue,
                Some(s) => s,
            };
            for (runway, usage) in selection {
                let flags = match usage {
                    RunwayUse::Departing => vec![1],
                    RunwayUse::Arriving => vec![0],
                    RunwayUse::Both => vec![1, 0],
                };

                for flag in flags {
                    writeln!(writer, "ACTIVE_RUNWAY:{}:{}:{}", airport.icao, runway, flag)?;
                }
            }
        }
    }

    Ok(())
}

fn search_for_euroscope_newest_sct_file() -> Option<(PathBuf, String)> {
    let bd = BaseDirs::new();
    let ud = UserDirs::new();
    let mut possibilities = [
        bd.map(|d| d.config_dir().join("Euroscope")),
        ud.clone()
            .and_then(|d| d.document_dir().map(|d| d.join("Euroscope"))),
    ]
    .into_iter()
    .flatten()
    .chain({
        std::iter::once(PathBuf::from(format!(
            "/mnt/c/Users/{}/Documents/Euroscope/Euroscope_dev",
            whoami::username()
        )))
    })
    .collect_vec();

    possibilities.retain(|p| p.exists() && p.is_dir());

    search_for_ese_with_possibilities(&possibilities)
}

fn search_for_ese_with_possibilities<P: AsRef<Path>>(
    possibilities: &[P],
) -> Option<(PathBuf, String)> {
    let sct_files = possibilities
        .iter()
        .flat_map(|p| {
            WalkDir::new(p)
                .max_depth(1)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|e| {
                    let name = e.file_name().to_string_lossy();
                    let Some(extension) = e.path().extension() else {
                        return false;
                    };
                    name.starts_with("ENOR") && extension == "sct"
                })
                .map(|e| e.path().to_path_buf())
        })
        .collect::<Vec<_>>();
    let file = sct_files.iter().max_by_key(get_es_file_name_time)?;
    Some((
        file.parent()?.to_owned(),
        Path::new(file.file_name()?)
            .file_stem()?
            .to_string_lossy()
            .to_string(),
    ))
}

fn get_es_file_name<P: AsRef<Path>>(path: &P) -> Option<String> {
    path.as_ref()
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
}

fn get_es_file_name_time<P: AsRef<Path>>(path: &P) -> DateTime {
    // example file name: ENOR-Norway-NC_20250612121259-241301-0006.sct
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?<time>\d{14})").unwrap());

    if let Some(caps) = RE.captures(get_es_file_name(path).as_deref().unwrap_or(""))
        && let Some(time_str) = caps.name("time")
        && let Ok(dt) = DateTime::strptime("%Y%m%d%H%M%S", time_str.as_str())
    {
        return dt;
    }
    path.as_ref()
        .metadata()
        .and_then(|m| m.created())
        .map(systemtime_to_jiff_datetime)
        .unwrap_or(datetime(1970, 1, 1, 0, 0, 0, 0))
}

fn systemtime_to_jiff_datetime(st: SystemTime) -> DateTime {
    // Convert to duration since UNIX_EPOCH
    let duration = st.duration_since(SystemTime::UNIX_EPOCH).unwrap();

    // Convert seconds and nanoseconds into a jiff DateTime (UTC)
    let ts = jiff::Timestamp::from_second(duration.as_secs() as i64).unwrap();
    let mut zoned = ts.to_zoned(TimeZone::system());
    zoned = zoned.with_time_zone(TimeZone::UTC);
    zoned.datetime()
}

#[cfg(test)]
mod tests {
    use super::*;

    impl ESConfig {
        pub fn new_for_test() -> Self {
            let config_file = PathBuf::from("config.toml");
            let config: Configurable = Config::builder()
                .add_source(config::File::from(config_file.clone()).required(true))
                .build()
                .expect("Failed to build configuration")
                .try_deserialize::<Configurable>()
                .expect("Failed to deserialize configuration");
            Self {
                euroscope_config_folder: PathBuf::from("/test/path"),
                enor_file_prefix: "ENOR-Test".to_string(),
                config,
                config_file_path: config_file,
                app_launchers: IndexSet::new(),
            }
        }
    }

    #[test]
    fn test_read_active_airports() {
        let data = "ACTIVE_AIRPORT:ENVA:1\nACTIVE_AIRPORT:ENBR:1\nACTIVE_AIRPORT:ENBO:0\nACTIVE_RUNWAY:ENZV:18:1\nACTIVE_RUNWAY:ENZV:18:0\n";
        let mut cursor = io::Cursor::new(data);
        let result = read_active_airport(&mut cursor).unwrap();
        let expected = "ACTIVE_AIRPORT:ENVA:1\nACTIVE_AIRPORT:ENBR:1\nACTIVE_AIRPORT:ENBO:0";
        assert_eq!(result, expected);
    }

    #[test]
    fn test_get_es_file_name() {
        let p = "ENOR-Norway-NC-DEV_20230403191923-230301-0004.sct";
        let dt = get_es_file_name_time(&p);
        let target = DateTime::strptime("%Y%m%d%H%M%S", "20230403191923").unwrap();
        assert_eq!(dt, target);
    }

    // [[executable]]
    // name = "Euroscope"
    // prf = "enor_rads.prf"

    // [[executable]]
    // name = "Euroscope"
    // prf = "enor_gnd.prf"

    // [[executable]]
    // name = "TrackAudio"

    #[test]
    fn test_app_loucher_reader() {
        let config_file = get_app_launchers(&PathBuf::from("test.toml"));
        let mut expected = IndexSet::new();
        expected.insert(AppLauncher {
            name: "Euroscope".to_string(),
            args: vec![],
            prf: Some(PathBuf::from("enor_rads.prf")),
        });
        expected.insert(AppLauncher {
            name: "Euroscope".to_string(),
            args: vec![],
            prf: Some(PathBuf::from("enor_gnd.prf")),
        });
        expected.insert(AppLauncher {
            name: "TrackAudio".to_string(),
            args: vec![],
            prf: None,
        });
        assert_eq!(config_file, expected);
    }
}
