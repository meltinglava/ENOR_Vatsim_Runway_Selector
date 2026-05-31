//! Writer for EuroScope's `.rwy` runway-assignment file.

use std::{
    fs::File,
    io::{self, BufRead, BufReader, BufWriter, Read, Write},
    path::Path,
};

use itertools::Itertools;
use tempfile::NamedTempFile;

use crate::{airport::RunwayInUseSource, airports::Airports, error::CoreResult};

/// Read the existing `.rwy` file at `rwy_path`, preserve its `ACTIVE_AIRPORT:`
/// header block, and rewrite the file with that header followed by
/// `ACTIVE_RUNWAY:` lines reflecting the current `airports` selections.
///
/// Writes atomically: the new content is staged in a temp file in the same
/// directory and `rename`d over the target on success, so a failure midway
/// through writing leaves the original `.rwy` untouched.
pub fn write_runways_to_rwy_file(rwy_path: &Path, airports: &Airports) -> CoreResult<()> {
    let start_of_file = {
        let mut existing = File::open(rwy_path)?;
        read_active_airport(&mut existing)?
    };

    let parent = rwy_path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = NamedTempFile::new_in(parent)?;
    {
        let mut writer = BufWriter::new(tmp.as_file());
        write_runway_file(&mut writer, airports, &start_of_file)?;
        writer.flush()?;
    }
    tmp.as_file().sync_all()?;
    tmp.persist(rwy_path).map_err(|e| e.error)?;
    Ok(())
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
