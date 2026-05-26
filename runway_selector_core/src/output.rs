//! Writer for EuroScope's `.rwy` runway-assignment file.

use std::{
    fs::OpenOptions,
    io::{self, BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::Path,
};

use itertools::Itertools;

use crate::{airport::RunwayInUseSource, airports::Airports, error::CoreResult};

/// Read the existing `.rwy` file at `rwy_path`, preserve its `ACTIVE_AIRPORT:`
/// header block, and rewrite the file with that header followed by
/// `ACTIVE_RUNWAY:` lines reflecting the current `airports` selections.
pub fn write_runways_to_rwy_file(rwy_path: &Path, airports: &Airports) -> CoreResult<()> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(false)
        .truncate(false)
        .open(rwy_path)?;

    let start_of_file = read_active_airport(&mut file)?;
    file.seek(SeekFrom::Start(0))?;
    file.set_len(0)?;
    write_runway_file(&mut file, airports, &start_of_file)
}

/// Collect the leading `ACTIVE_AIRPORT:` lines from a `.rwy` file. These are
/// preserved verbatim across rewrites so EuroScope's airport activation state
/// is not disturbed.
#[allow(unstable_name_collisions)] // `intersperse_with` — we can drop the allow when itertools stabilizes its replacement
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

fn write_runway_file<T: Write>(
    rwy_file: &mut T,
    airports: &Airports,
    start_of_file: &str,
) -> CoreResult<()> {
    let mut writer = BufWriter::new(rwy_file);
    writeln!(writer, "{start_of_file}")?;

    for airport in airports.airports.values() {
        if let Some(selection) = RunwayInUseSource::default_sort_order()
            .iter()
            .find_map(|method| airport.runways_in_use.get(method))
        {
            for (runway, usage) in selection {
                for flag in usage.active_runway_flags() {
                    writeln!(writer, "ACTIVE_RUNWAY:{}:{}:{}", airport.icao, runway, flag)?;
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_active_airports() {
        let data = "ACTIVE_AIRPORT:ENVA:1\nACTIVE_AIRPORT:ENBR:1\nACTIVE_AIRPORT:ENBO:0\nACTIVE_RUNWAY:ENZV:18:1\nACTIVE_RUNWAY:ENZV:18:0\n";
        let mut cursor = io::Cursor::new(data);
        let result = read_active_airport(&mut cursor).unwrap();
        let expected = "ACTIVE_AIRPORT:ENVA:1\nACTIVE_AIRPORT:ENBR:1\nACTIVE_AIRPORT:ENBO:0";
        assert_eq!(result, expected);
    }
}
