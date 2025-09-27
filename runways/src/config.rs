use std::{
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use config::{Config, ConfigError};
use directories::{BaseDirs, ProjectDirs, UserDirs};
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use jiff::civil::DateTime;
use rfd::FileDialog;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use tracing::warn;
use walkdir::WalkDir;

use crate::{airports::Airports, error::ApplicationResult, runway::RunwayUse};

#[derive(Debug)]
pub(crate) struct ESConfig {
    euroscope_config_folder: PathBuf,
    enor_file_prefix: String,
    #[allow(dead_code)] // used in tests
    config_file_path: PathBuf,
    config: Configurable,
}

#[derive(Debug, Serialize, Deserialize)]
#[skip_serializing_none]
struct Configurable {
    ignore_airports: IndexSet<String>,
    default_runways: IndexMap<String, u8>,
    euroscope_config_folder: Option<PathBuf>,
}

impl ESConfig {
    pub fn find_euroscope_config_folder() -> Option<Self> {
        let (mut config, config_file_path) = setup_configuration().unwrap();
        let sct_path = search_for_euroscope_newest_sct_file()
            .or_else(|| config.euroscope_config_folder.clone())
            .or_else(|| get_rfd_euroscope_config_folder(&mut config, &config_file_path))?;
        let enor_file_prefix = sct_path.file_stem()?.to_string_lossy().to_string();
        Some(Self {
            euroscope_config_folder: sct_path.parent()?.to_path_buf(),
            enor_file_prefix,
            config,
            config_file_path,
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
}

fn get_rfd_euroscope_config_folder<P: AsRef<Path>>(
    config: &mut Configurable,
    config_file_path: &P,
) -> Option<PathBuf> {
    let bd = BaseDirs::new()?;

    FileDialog::new()
        .set_title("Select Euroscope sector file folder. The folder containing the ese file")
        .set_directory(bd.config_dir())
        .add_filter("Euroscope Configuration", &["sct", "rwy"])
        .pick_folder()
        .inspect(|path: &PathBuf| {
            config.euroscope_config_folder = Some(path.clone());
            fs::write(config_file_path, toml::to_string_pretty(&config).unwrap())
                .expect("Failed to write config file");
        })
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

fn setup_configuration() -> Result<(Configurable, PathBuf), ConfigError> {
    let config_dir = ProjectDirs::from("", "meltinglava", "vatsca_es_setup")
        .expect("Failed to get project directories")
        .config_dir()
        .to_path_buf();

    let config_file = config_dir.join("config.toml");
    if !config_file.exists() {
        std::fs::create_dir_all(&config_dir).expect("Failed to create config directory");
        std::fs::write(&config_file, include_str!("../config.toml"))
            .expect("Failed to create config file");
    }
    let configurable = Config::builder()
        .add_source(config::File::from(config_file.clone()).required(true))
        .build()
        .expect("Failed to build configuration")
        .try_deserialize::<Configurable>()?;
    Ok((configurable, config_file))
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

        for (runway, usage) in &airport.runways_in_use {
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

    Ok(())
}

fn search_for_euroscope_newest_sct_file() -> Option<PathBuf> {
    let bd = BaseDirs::new();
    let ud = UserDirs::new();
    let mut possibilities = [
        bd.map(|d| d.config_dir().join("Euroscope")),
        ud.and_then(|d| d.document_dir().map(|d| d.join("Euroscope"))),
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

    let extra_locations = [];
    possibilities.extend(extra_locations);
    possibilities.retain(|p| p.exists() && p.is_dir());

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
    sct_files.iter().max_by_key(get_es_file_name_time).cloned()
}

fn get_es_file_name_time<P: AsRef<Path>>(path: &P) -> DateTime {
    // example file name: ENOR-Norway-NC_20250612121259-241301-0006.sct
    let file_name = path.as_ref().file_name().unwrap().to_string_lossy();
    let time_str = file_name
        .split('-')
        .nth(2)
        .unwrap()
        .split_once('_')
        .unwrap()
        .1;
    DateTime::strptime("%Y%m%d%H%M%S", time_str).unwrap()
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
}
