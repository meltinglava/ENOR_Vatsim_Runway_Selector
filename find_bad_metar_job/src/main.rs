use std::{fs::File, path::Path, str::FromStr, sync::LazyLock};

use futures::future::try_join_all;
use indexmap::IndexSet;
use metar_decoder::metar::Metar;

async fn get_metars_text() -> reqwest::Result<String> {
    // let urls = ["https://metar.vatsim.net/E", "https://metar.vatsim.net/L"];
    let urls = ["https://metar.vatsim.net/*"];
    let pages =
        try_join_all(urls.iter().map(async |&url| -> reqwest::Result<String> {
            reqwest::get(url).await?.text().await
        }))
        .await?;
    Ok(pages.join("\n"))
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

    let mut failed = IndexSet::new();
    let mut to_test = get_already_failed_metars(path);
    to_test.extend(metars.lines().map(str::to_owned));
    for line in to_test {
        if line.trim().is_empty() {
            continue;
        }
        if IGNORE_AIRPORTS.contains(&&line[0..4]) {
            continue;
        }
        if Metar::from_str(&line).is_err() {
            failed.insert(line);
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
