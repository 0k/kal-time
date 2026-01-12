use std::env;
use std::fmt::Write as _;
use std::process;

use chrono::{DateTime, FixedOffset, Local, NaiveDateTime, TimeZone};
use kal_time::{parse, parse_timespan, parse_timespan_with_reference, parse_with_reference};

/// Represents a parsed reference that can be either Local (DST-aware) or FixedOffset
enum Reference {
    Local(DateTime<Local>),
    Fixed(DateTime<FixedOffset>),
}

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();

    if args.len() < 2 || args.len() > 3 {
        return Err(usage());
    }

    let action = &args[0];
    let input = &args[1];
    let reference = if let Some(reference_str) = args.get(2) {
        Some(parse_reference(reference_str).map_err(|e| format!("Invalid reference time: {e}"))?)
    } else {
        None
    };

    match action.as_str() {
        "time" => {
            let dt = match reference {
                Some(Reference::Fixed(ref_dt)) => parse_with_reference(input, &ref_dt)
                    .map_err(|e| format!("Failed to parse time: {e}"))?,
                Some(Reference::Local(ref_dt)) => parse_with_reference(input, &ref_dt)
                    .map_err(|e| format!("Failed to parse time: {e}"))?,
                None => parse(input).map_err(|e| format!("Failed to parse time: {e}"))?,
            };
            println!("{}", format_timestamp(&dt));
        }
        "timespan" => {
            let (start, stop) = match reference {
                Some(Reference::Fixed(ref_dt)) => parse_timespan_with_reference(input, &ref_dt)
                    .map_err(|e| format!("Failed to parse timespan: {e}"))?,
                Some(Reference::Local(ref_dt)) => parse_timespan_with_reference(input, &ref_dt)
                    .map_err(|e| format!("Failed to parse timespan: {e}"))?,
                None => {
                    parse_timespan(input).map_err(|e| format!("Failed to parse timespan: {e}"))?
                }
            };
            println!("{}", format_timestamp(&start));
            println!("{}", format_timestamp(&stop));
        }
        _ => return Err(usage()),
    }

    Ok(())
}

fn usage() -> String {
    let mut msg = String::from("Usage: kt-parse <time|timespan> <input> [reference]");
    let _ = write!(
        msg,
        "\n  <input>: time or timespan string accepted by kal-time\n  [reference]: fully specified timestamp with timezone (e.g. 2025-10-22T09:10:11+00:00)\n"
    );
    msg
}

fn parse_reference(s: &str) -> Result<Reference, String> {
    // First, try formats with explicit timezone offset
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(Reference::Fixed(dt));
    }

    const FORMATS_WITH_TZ: &[&str] = &[
        "%Y-%m-%d %H:%M:%S%:z",
        "%Y-%m-%d %H:%M:%S %:z",
        "%Y-%m-%d %H:%M%:z",
        "%Y-%m-%d %H:%M %:z",
        "%Y-%m-%dT%H:%M:%S%:z",
        "%Y-%m-%dT%H:%M%:z",
    ];

    for fmt in FORMATS_WITH_TZ {
        if let Ok(dt) = DateTime::parse_from_str(s, fmt) {
            return Ok(Reference::Fixed(dt));
        }
    }

    // No timezone in input - parse as naive and interpret as Local
    const FORMATS_NO_TZ: &[&str] = &[
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
    ];

    for fmt in FORMATS_NO_TZ {
        if let Ok(naive) = NaiveDateTime::parse_from_str(s, fmt) {
            match Local.from_local_datetime(&naive) {
                chrono::LocalResult::Single(dt) => return Ok(Reference::Local(dt)),
                chrono::LocalResult::Ambiguous(dt, _) => return Ok(Reference::Local(dt)),
                chrono::LocalResult::None => {
                    return Err(format!("Reference time does not exist (DST gap): {s}"))
                }
            }
        }
    }

    Err(format!("Unable to parse reference timestamp: {s}"))
}

fn format_timestamp(dt: &DateTime<FixedOffset>) -> String {
    format!("{} {}", dt.timestamp(), dt.format("%Y-%m-%d %H:%M:%S %:z"))
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    fn tzset_refresh() {
        unsafe extern "C" {
            fn tzset();
        }
        unsafe {
            tzset();
        }
    }

    fn kt_parse_time(input: &str, reference: Option<&str>) -> String {
        // Get the path to the test binary's directory and find kt-parse there
        let exe = std::env::current_exe()
            .expect("current_exe")
            .parent()
            .expect("parent")
            .parent()
            .expect("parent")
            .join("kt-parse");
        let mut cmd = Command::new(&exe);
        cmd.arg("time").arg(input);
        if let Some(r) = reference {
            cmd.arg(r);
        }
        let output = cmd.output().expect("failed to run kt-parse");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[test]
    fn test_local_winter_time() {
        // TZ=Europe/Paris kt-parse time "2025-01-06 11:45"
        // => 1736160300 2025-01-06 11:45:00 +01:00
        unsafe { std::env::set_var("TZ", "Europe/Paris"); }
        tzset_refresh();

        let result = kt_parse_time("2025-01-06 11:45", None);
        assert_eq!(result, "1736160300 2025-01-06 11:45:00 +01:00");
    }

    #[test]
    fn test_local_summer_time() {
        // TZ=Europe/Paris kt-parse time "2025-06-06 11:45"
        // => 1749203100 2025-06-06 11:45:00 +02:00
        unsafe { std::env::set_var("TZ", "Europe/Paris"); }
        tzset_refresh();

        let result = kt_parse_time("2025-06-06 11:45", None);
        assert_eq!(result, "1749203100 2025-06-06 11:45:00 +02:00");
    }

    #[test]
    fn test_local_reference_dst_aware() {
        // TZ=Europe/Paris kt-parse time "2025-06-06 11:45" "2025-01-01 11:00:00"
        // Reference without offset → Local → DST-aware for target date
        // => 1749203100 2025-06-06 11:45:00 +02:00
        unsafe { std::env::set_var("TZ", "Europe/Paris"); }
        tzset_refresh();

        let result = kt_parse_time("2025-06-06 11:45", Some("2025-01-01 11:00:00"));
        assert_eq!(result, "1749203100 2025-06-06 11:45:00 +02:00");
    }

    #[test]
    fn test_fixed_offset_reference() {
        // TZ=Europe/Paris kt-parse time "2025-06-06 11:45" "2025-01-01 11:00:00+05:30"
        // Reference with explicit offset → use that offset
        // => 1749190500 2025-06-06 11:45:00 +05:30
        unsafe { std::env::set_var("TZ", "Europe/Paris"); }
        tzset_refresh();

        let result = kt_parse_time("2025-06-06 11:45", Some("2025-01-01 11:00:00+05:30"));
        assert_eq!(result, "1749190500 2025-06-06 11:45:00 +05:30");
    }
}
