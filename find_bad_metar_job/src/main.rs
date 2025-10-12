use std::{fs::File, path::Path, str::FromStr, sync::LazyLock};

use indexmap::IndexSet;
use metar_decoder::metar::Metar;

async fn get_metars_text() -> reqwest::Result<String> {
    let response = reqwest::get("https://metar.vatsim.net/E")
        .await?
        .text()
        .await?;
    Ok(response)
}

fn get_already_failed_metars(path: &Path) -> IndexSet<String> {
    File::open(path)
        .ok()
        .and_then(|rdr| serde_json::from_reader(rdr).ok())
        .unwrap_or_default()
}

fn write_failed_metars(path: &Path, failed: &IndexSet<String>) {
    if let Ok(file) = File::create(path) {
        let _ = serde_json::to_writer_pretty(file, failed);
    }
}

fn find_fail_parsed_metars(metars: &str, path: &Path) {
    static IGNORE_AIRPORTS: LazyLock<IndexSet<&str>> = LazyLock::new(|| IndexSet::from(["EQYS"]));

    let mut failed = get_already_failed_metars(path);
    for line in metars.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if IGNORE_AIRPORTS.contains(&&line[0..4]) {
            continue;
        }
        if Metar::from_str(line).is_err() {
            failed.insert(line.to_owned());
        }
    }
    write_failed_metars(path, &failed);
}

#[tokio::main]
async fn main() -> reqwest::Result<()> {
    tracing_subscriber::fmt::init();
    let metars = get_metars_text().await?;
    let p = Path::new("failed_metars.json");
    find_fail_parsed_metars(&metars, p);
    Ok(())
}
