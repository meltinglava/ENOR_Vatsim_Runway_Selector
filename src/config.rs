use std::{error::Error, fs::OpenOptions, io::{self, BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write}, path::{Path, PathBuf}};

use itertools::Itertools;
use tracing::warn;
use directories::BaseDirs;
use jiff::civil::DateTime;
use walkdir::WalkDir;

use crate::{airports::Airports, runway::RunwayUse};

pub(crate) struct Config {
    euroscope_config_folder: PathBuf,
    enor_file_prefix: String,
}

impl Config {
    pub fn find_euroscope_config_folder() -> Option<Self> {
        let sct_path = search_for_euroscope_newest_sct_file()?;
        let enor_file_prefix = sct_path.file_stem()?
            .to_string_lossy()
            .to_string();
        Some(Self {
            euroscope_config_folder: sct_path.parent()?.to_path_buf(),
            enor_file_prefix,
        })
    }

    pub fn get_sct_file_path(&self) -> PathBuf {
        self.euroscope_config_folder.join(format!("{}.sct", self.enor_file_prefix))
    }

    pub fn get_rwy_file_path(&self) -> PathBuf {
        self.euroscope_config_folder.join(format!("{}.rwy", self.enor_file_prefix))
    }

    pub fn write_runways_to_euroscope_rwy_file(&self, airports: &Airports) -> Result<(), Box<dyn std::error::Error>> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(false)
            .truncate(false)
            .open(self.get_rwy_file_path())?;

        let start_of_file = read_active_airportt(&mut file)?;
        file.seek(SeekFrom::Start(0))?;
        file.set_len(0)?;
        write_runway_file(&mut file, airports, &start_of_file)
    }
}

#[allow(unstable_name_collisions)] // `intersperse_with` is but we can update itertools once it stabilizes
pub fn read_active_airportt<T: Read>(rwy_file: &mut T) -> io::Result<String> {
    let reader = BufReader::new(rwy_file);

    reader
        .lines()
        .take_while(|l| {
            match l {
                Ok(l) => l.starts_with("ACTIVE_AIRPORT:"),
                Err(_) => false,
            }
        })
        .intersperse_with(|| Ok("\n".to_string()))
        .collect::<io::Result<String>>()
}

fn write_runway_file<T: Write>(rwy_file: &mut T, airports: &Airports, start_of_file: &str) -> Result<(), Box<dyn Error>> {

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
    let mut possibilities = bd.map(|bd| {
        vec![
            bd.config_dir().join("Euroscope"),
            bd.home_dir().join("Documents/Euroscope"),
        ]
    }).unwrap_or_default();

    let extra_locations = [
        PathBuf::from(format!("/mnt/c/Users/{}/Documents/Euroscope/Euroscope_dev", whoami::username())),
    ];
    possibilities.extend(extra_locations);
    possibilities.retain(|p| p.exists() && p.is_dir());

    let sct_files = possibilities.iter().flat_map(|p| {
        WalkDir::new(p)
            .max_depth(1)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| {
                let name = e.file_name().to_string_lossy();
                let Some(extension) = e.path().extension() else {return false;};
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
    let time_str = file_name.split('-').nth(2).unwrap().split_once('_').unwrap().1;
    DateTime::strptime("%Y%m%d%H%M%S", time_str).unwrap()
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_active_airports() {
        let data = "ACTIVE_AIRPORT:ENVA:1\nACTIVE_AIRPORT:ENBR:1\nACTIVE_AIRPORT:ENBO:0\nACTIVE_RUNWAY:ENZV:18:1\nACTIVE_RUNWAY:ENZV:18:0\n";
        let mut cursor = io::Cursor::new(data);
        let result = read_active_airportt(&mut cursor).unwrap();
        let expected = "ACTIVE_AIRPORT:ENVA:1\nACTIVE_AIRPORT:ENBR:1\nACTIVE_AIRPORT:ENBO:0";
        assert_eq!(result, expected);
    }
}
