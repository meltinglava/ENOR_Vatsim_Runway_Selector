use std::path::{Path, PathBuf};

use directories::BaseDirs;
use jiff::civil::DateTime;
use walkdir::WalkDir;

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
                name.starts_with("ENOR") || extension == "sct"
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
    DateTime::strptime(time_str, "%Y%m%d%H%M%S").unwrap()
}
