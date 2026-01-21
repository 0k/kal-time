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

/// Output format for timestamps
#[derive(Clone, Copy, Default)]
enum OutputFormat {
    #[default]
    Full,
    Timestamp,
    Iso,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();

    let (format, remaining_args) = parse_format_flag(&args)?;

    if remaining_args.len() < 2 || remaining_args.len() > 3 {
        return Err(usage());
    }

    let action = &remaining_args[0];
    let input = &remaining_args[1];
    let reference = if let Some(reference_str) = remaining_args.get(2) {
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
            println!("{}", format_output(&dt, format));
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
            println!("{}", format_output(&start, format));
            println!("{}", format_output(&stop, format));
        }
        _ => return Err(usage()),
    }

    Ok(())
}

fn parse_format_value(value: &str) -> Result<OutputFormat, String> {
    match value {
        "ts" => Ok(OutputFormat::Timestamp),
        "iso" => Ok(OutputFormat::Iso),
        "full" => Ok(OutputFormat::Full),
        _ => Err(format!(
            "Invalid format '{}'. Valid options: ts, iso, full",
            value
        )),
    }
}

fn parse_format_flag(args: &[String]) -> Result<(OutputFormat, Vec<String>), String> {
    let mut format = OutputFormat::default();
    let mut remaining = Vec::new();
    let mut iter = args.iter();

    while let Some(arg) = iter.next() {
        if arg == "-F" || arg == "--format" {
            let value = iter
                .next()
                .ok_or_else(|| "Missing value for -F/--format flag".to_string())?;
            format = parse_format_value(value)?;
        } else if let Some(value) = arg.strip_prefix("-F") {
            format = parse_format_value(value)?;
        } else {
            remaining.push(arg.clone());
        }
    }

    Ok((format, remaining))
}

fn usage() -> String {
    let mut msg = String::from("Usage: kt-parse [-F <format>] <time|timespan> <input> [reference]");
    let _ = write!(
        msg,
        "\n\nOptions:\n  -F, --format <format>  Output format: ts (timestamp), iso, full (default)\n\nArguments:\n  <input>      Time or timespan string accepted by kal-time\n  [reference]  Fully specified timestamp with timezone (e.g. 2025-10-22T09:10:11+00:00)\n"
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
                    return Err(format!("Reference time does not exist (DST gap): {s}"));
                }
            }
        }
    }

    Err(format!("Unable to parse reference timestamp: {s}"))
}

fn format_output(dt: &DateTime<FixedOffset>, format: OutputFormat) -> String {
    match format {
        OutputFormat::Full => {
            format!("{} {}", dt.timestamp(), dt.format("%Y-%m-%d %H:%M:%S %:z"))
        }
        OutputFormat::Timestamp => dt.timestamp().to_string(),
        OutputFormat::Iso => dt.to_rfc3339(),
    }
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

    fn kt_parse_exe() -> std::path::PathBuf {
        std::env::current_exe()
            .expect("current_exe")
            .parent()
            .expect("parent")
            .parent()
            .expect("parent")
            .join("kt-parse")
    }

    fn kt_parse_time(input: &str, reference: Option<&str>) -> String {
        kt_parse_time_with_format(input, reference, None)
    }

    fn kt_parse_time_with_format(
        input: &str,
        reference: Option<&str>,
        format: Option<&str>,
    ) -> String {
        let mut cmd = Command::new(kt_parse_exe());
        if let Some(f) = format {
            cmd.arg("-F").arg(f);
        }
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
        unsafe {
            std::env::set_var("TZ", "Europe/Paris");
        }
        tzset_refresh();

        let result = kt_parse_time("2025-01-06 11:45", None);
        assert_eq!(result, "1736160300 2025-01-06 11:45:00 +01:00");
    }

    #[test]
    fn test_local_summer_time() {
        // TZ=Europe/Paris kt-parse time "2025-06-06 11:45"
        // => 1749203100 2025-06-06 11:45:00 +02:00
        unsafe {
            std::env::set_var("TZ", "Europe/Paris");
        }
        tzset_refresh();

        let result = kt_parse_time("2025-06-06 11:45", None);
        assert_eq!(result, "1749203100 2025-06-06 11:45:00 +02:00");
    }

    #[test]
    fn test_local_reference_dst_aware() {
        // TZ=Europe/Paris kt-parse time "2025-06-06 11:45" "2025-01-01 11:00:00"
        // Reference without offset → Local → DST-aware for target date
        // => 1749203100 2025-06-06 11:45:00 +02:00
        unsafe {
            std::env::set_var("TZ", "Europe/Paris");
        }
        tzset_refresh();

        let result = kt_parse_time("2025-06-06 11:45", Some("2025-01-01 11:00:00"));
        assert_eq!(result, "1749203100 2025-06-06 11:45:00 +02:00");
    }

    #[test]
    fn test_fixed_offset_reference() {
        // TZ=Europe/Paris kt-parse time "2025-06-06 11:45" "2025-01-01 11:00:00+05:30"
        // Reference with explicit offset → use that offset
        // => 1749190500 2025-06-06 11:45:00 +05:30
        unsafe {
            std::env::set_var("TZ", "Europe/Paris");
        }
        tzset_refresh();

        let result = kt_parse_time("2025-06-06 11:45", Some("2025-01-01 11:00:00+05:30"));
        assert_eq!(result, "1749190500 2025-06-06 11:45:00 +05:30");
    }

    #[test]
    fn test_format_timestamp_only() {
        unsafe {
            std::env::set_var("TZ", "Europe/Paris");
        }
        tzset_refresh();

        let result = kt_parse_time_with_format("2025-01-06 11:45", None, Some("ts"));
        assert_eq!(result, "1736160300");
    }

    #[test]
    fn test_format_iso() {
        unsafe {
            std::env::set_var("TZ", "Europe/Paris");
        }
        tzset_refresh();

        let result = kt_parse_time_with_format("2025-01-06 11:45", None, Some("iso"));
        assert_eq!(result, "2025-01-06T11:45:00+01:00");
    }

    #[test]
    fn test_format_full_explicit() {
        unsafe {
            std::env::set_var("TZ", "Europe/Paris");
        }
        tzset_refresh();

        let result = kt_parse_time_with_format("2025-01-06 11:45", None, Some("full"));
        assert_eq!(result, "1736160300 2025-01-06 11:45:00 +01:00");
    }

    #[test]
    fn test_format_flag_compact() {
        unsafe {
            std::env::set_var("TZ", "Europe/Paris");
        }
        tzset_refresh();

        let mut cmd = Command::new(kt_parse_exe());
        cmd.args(["-Fts", "time", "2025-01-06 11:45"]);
        let output = cmd.output().expect("failed to run kt-parse");
        let result = String::from_utf8_lossy(&output.stdout).trim().to_string();
        assert_eq!(result, "1736160300");
    }

    #[test]
    fn test_format_long_flag() {
        unsafe {
            std::env::set_var("TZ", "Europe/Paris");
        }
        tzset_refresh();

        let mut cmd = Command::new(kt_parse_exe());
        cmd.args(["--format", "iso", "time", "2025-01-06 11:45"]);
        let output = cmd.output().expect("failed to run kt-parse");
        let result = String::from_utf8_lossy(&output.stdout).trim().to_string();
        assert_eq!(result, "2025-01-06T11:45:00+01:00");
    }

    #[test]
    fn test_format_invalid_value() {
        let mut cmd = Command::new(kt_parse_exe());
        cmd.args(["-F", "invalid", "time", "2025-01-06 11:45"]);
        let output = cmd.output().expect("failed to run kt-parse");
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("Invalid format 'invalid'"));
        assert!(stderr.contains("ts, iso, full"));
    }

    fn kt_parse_timespan_with_format(input: &str, format: Option<&str>) -> String {
        let mut cmd = Command::new(kt_parse_exe());
        if let Some(f) = format {
            cmd.arg("-F").arg(f);
        }
        cmd.arg("timespan").arg(input);
        let output = cmd.output().expect("failed to run kt-parse");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[test]
    fn test_timespan_format_ts() {
        unsafe {
            std::env::set_var("TZ", "Europe/Paris");
        }
        tzset_refresh();

        let result = kt_parse_timespan_with_format("2025-01-06 11:45..12:00", Some("ts"));
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "1736160300");
        assert_eq!(lines[1], "1736161200");
    }

    #[test]
    fn test_timespan_format_iso() {
        unsafe {
            std::env::set_var("TZ", "Europe/Paris");
        }
        tzset_refresh();

        let result = kt_parse_timespan_with_format("2025-01-06 11:45..12:00", Some("iso"));
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "2025-01-06T11:45:00+01:00");
        assert_eq!(lines[1], "2025-01-06T12:00:00+01:00");
    }
}
