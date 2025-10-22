use std::env;
use std::fmt::Write as _;
use std::process;

use chrono::{DateTime, FixedOffset};
use kal_time::{
    parse, parse_timespan, parse_timespan_with_reference, parse_with_reference,
};

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
                Some(ref_dt) => parse_with_reference(input, &ref_dt)
                    .map_err(|e| format!("Failed to parse time: {e}"))?,
                None => parse(input).map_err(|e| format!("Failed to parse time: {e}"))?,
            };
            println!("{}", format_timestamp(&dt));
        }
        "timespan" => {
            let (start, stop) = match reference {
                Some(ref_dt) => parse_timespan_with_reference(input, &ref_dt)
                    .map_err(|e| format!("Failed to parse timespan: {e}"))?,
                None => parse_timespan(input)
                    .map_err(|e| format!("Failed to parse timespan: {e}"))?,
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

fn parse_reference(s: &str) -> Result<DateTime<FixedOffset>, String> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt);
    }

    const FORMATS: &[&str] = &[
        "%Y-%m-%d %H:%M:%S %:z",
        "%Y-%m-%d %H:%M %:z",
        "%Y-%m-%dT%H:%M:%S%:z",
        "%Y-%m-%dT%H:%M%:z",
    ];

    for fmt in FORMATS {
        if let Ok(dt) = DateTime::parse_from_str(s, fmt) {
            return Ok(dt);
        }
    }

    Err(format!("Unable to parse reference timestamp: {s}"))
}

fn format_timestamp(dt: &DateTime<FixedOffset>) -> String {
    format!(
        "{} {}",
        dt.timestamp(),
        dt.format("%Y-%m-%d %H:%M:%S %:z")
    )
}
