use chrono::{DateTime, FixedOffset, TimeZone};
use lazy_static::lazy_static;

mod parse;

lazy_static! {
    static ref TIMEPARSER_FORMATS: Vec<&'static str> = vec![
        "%Y-%m-%d",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%m-%d",
        "%m/%d",
        "%m-%d %H:%M:%S",
        "%m-%d %H:%M",
        "%d %H:%M",
        "%d %Hh%M",
        "%d %Hh",
        "%H:%M:%S",
        "%H:%M",
        "%Hh%M",
        "%Hh",
        "%Mm",
        "%M",
        "@%s",
    ];
}

/// Generate suffix formats by progressively trimming the leftmost `%X` specifier
/// and its trailing separator.
///
/// Example: "%Y-%m-%d %H:%M:%S" produces:
///   ["%Y-%m-%d %H:%M:%S", "%m-%d %H:%M:%S", "%d %H:%M:%S", "%H:%M:%S", "%M:%S", "%S"]
fn generate_suffix_formats(fmt: &str) -> Vec<String> {
    let mut result = vec![fmt.to_string()];
    let mut current = fmt;

    while let Some(suffix) = trim_leftmost_specifier(current) {
        if !suffix.is_empty() {
            result.push(suffix.to_string());
            current = suffix;
        } else {
            break;
        }
    }

    result
}

/// Trim the leftmost `%X` specifier and its trailing separator from a format string.
/// Returns the remaining suffix, or None if no specifier found.
fn trim_leftmost_specifier(fmt: &str) -> Option<&str> {
    // Find the first '%'
    let pct_pos = fmt.find('%')?;

    // Skip past the '%' and the specifier character
    let after_pct = &fmt[pct_pos + 1..];
    if after_pct.is_empty() {
        return None;
    }

    // Skip the specifier character (handles single char specifiers like %Y, %m, %d, %H, %M, %S)
    let after_spec = &after_pct[1..];

    // Skip any non-'%' characters (the separator: '-', ':', ' ', 'h', 'm', etc.)
    let next_pct = after_spec.find('%').unwrap_or(after_spec.len());
    let suffix = &after_spec[next_pct..];

    Some(suffix)
}

fn parse_with_formats<'a, Tz: TimeZone>(
    timestr: &str,
    reference: &DateTime<Tz>,
    formats: &[&'a str],
) -> Option<(DateTime<FixedOffset>, &'a str)> {
    for format in formats {
        log::trace!("Trying to parse {:?} with format {:?}", timestr, format);
        if let Ok(dt) = parse::parse_partial(timestr, format, reference, true) {
            return Some((dt, format));
        }
    }
    None
}

fn parse_with_reference_internal<Tz: TimeZone>(
    timestr: &str,
    reference: &DateTime<Tz>,
) -> Result<(DateTime<FixedOffset>, &'static str), String> {
    if timestr.is_empty() {
        log::trace!("Using reference: {:?}", reference);
        let dt =
            parse::parse_partial("", "", reference, false).expect("empty parse should never fail");
        return Ok((dt, ""));
    }

    for format in TIMEPARSER_FORMATS.iter() {
        log::trace!("Trying to parse {:?} with format {:?}", timestr, format);
        if let Ok(dt) = parse::parse_partial(timestr, format, reference, true) {
            return Ok((dt, format));
        }
    }
    Err(format!("Could not parse time string: {:?}", timestr))
}

pub fn parse_with_reference<Tz: TimeZone>(
    timestr: &str,
    reference: &DateTime<Tz>,
) -> Result<DateTime<FixedOffset>, String> {
    parse_with_reference_internal(timestr, reference).map(|(dt, _fmt)| dt)
}

pub fn parse(timespan: &str) -> Result<DateTime<FixedOffset>, String> {
    let now = chrono::Local::now();
    parse_with_reference(timespan, &now)
}

pub fn parse_utc(timespan: &str) -> Result<DateTime<FixedOffset>, String> {
    let now = chrono::Utc::now();
    parse_with_reference(timespan, &now)
}

type Timespan = (DateTime<FixedOffset>, DateTime<FixedOffset>);

pub fn parse_timespan_with_reference<Tz: TimeZone>(
    timespan: &str,
    default: &DateTime<Tz>,
) -> Result<Timespan, String> {
    let (start, stop) = match timespan.split_once("..") {
        Some((start_str, stop_str)) => {
            let (first, first_fmt) = parse_with_reference_internal(start_str, default)?;

            // Try suffix formats first for the end part
            let suffix_formats = generate_suffix_formats(first_fmt);
            let suffix_refs: Vec<&str> = suffix_formats.iter().map(|s| s.as_str()).collect();

            let second = if let Some((dt, _)) = parse_with_formats(stop_str, &first, &suffix_refs) {
                dt
            } else {
                // Fall back to standard formats
                parse_with_reference(stop_str, &first)?
            };

            (first, second)
        }
        None => {
            let start = parse_with_reference(timespan, default)?;
            let stop = start + chrono::Duration::days(1);
            (start, stop)
        }
    };

    // Validate that start <= stop (reject reverse timespans)
    if start > stop {
        return Err(format!(
            "Invalid timespan '{}': end time ({}) is before start time ({})",
            timespan,
            stop.format("%Y-%m-%d %H:%M:%S %z"),
            start.format("%Y-%m-%d %H:%M:%S %z")
        ));
    }

    Ok((start, stop))
}

pub fn parse_timespan(timespan: &str) -> Result<Timespan, String> {
    let now = chrono::Local::now();
    parse_timespan_with_reference(timespan, &now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn pp<Tz: TimeZone>(s: &str, dt: &DateTime<Tz>) -> String {
        format!("{:?}", parse_with_reference(s, dt))
    }

    #[test]
    fn test_simple() {
        let dt = Utc.with_ymd_and_hms(2014, 7, 8, 9, 10, 11).unwrap(); // `2014-07-08T09:10:11Z`

        assert_eq!(pp("2014-07-08", &dt), "Ok(2014-07-08T00:00:00+00:00)");
        assert_eq!(pp("2015-01-01 08:08", &dt), "Ok(2015-01-01T08:08:00+00:00)");
        assert_eq!(pp("9h", &dt), "Ok(2014-07-08T09:00:00+00:00)");
        assert_eq!(pp("30m", &dt), "Ok(2014-07-08T09:30:00+00:00)");
    }

    #[test]
    fn test_ts() {
        let dt = Utc.with_ymd_and_hms(2014, 7, 8, 9, 10, 11).unwrap(); // `2014-07-08T09:10:11Z`
        assert_eq!(pp("@1704150000", &dt), "Ok(2024-01-01T23:00:00+00:00)");
    }

    #[test]
    fn test_timespan_end_uses_start_for_missing_fields() {
        let reference = Utc.with_ymd_and_hms(2025, 10, 27, 6, 0, 0).unwrap();
        let offset = FixedOffset::east_opt(0).unwrap();

        let (start, stop) =
            super::parse_timespan_with_reference("10:15..30", &reference).expect("timespan parse");

        let expected_start = offset.with_ymd_and_hms(2025, 10, 27, 10, 15, 0).unwrap();
        let expected_stop = offset.with_ymd_and_hms(2025, 10, 27, 10, 30, 0).unwrap();

        assert_eq!(start, expected_start);
        assert_eq!(stop, expected_stop);
    }

    #[test]
    fn test_timespan_full_start_keeps_end_on_same_day() {
        let reference = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let offset = FixedOffset::east_opt(0).unwrap();

        let (start, stop) =
            super::parse_timespan_with_reference("2025-10-27 10:30..11:30", &reference)
                .expect("timespan parse");

        let expected_start = offset.with_ymd_and_hms(2025, 10, 27, 10, 30, 0).unwrap();
        let expected_stop = offset.with_ymd_and_hms(2025, 10, 27, 11, 30, 0).unwrap();

        assert_eq!(start, expected_start);
        assert_eq!(stop, expected_stop);
    }

    #[test]
    fn test_timespan_minutes_seconds_inherit_hour_from_start() {
        let reference = Utc.with_ymd_and_hms(2025, 10, 1, 0, 0, 0).unwrap();
        let offset = FixedOffset::east_opt(0).unwrap();

        let (start, stop) =
            super::parse_timespan_with_reference("2025-10-27 10:00:00..01:30", &reference)
                .expect("timespan parse");

        let expected_start = offset.with_ymd_and_hms(2025, 10, 27, 10, 0, 0).unwrap();
        let expected_stop = offset.with_ymd_and_hms(2025, 10, 27, 10, 1, 30).unwrap();

        assert_eq!(start, expected_start);
        assert_eq!(stop, expected_stop);
    }

    #[test]
    fn test_full_datetime_should_ignore_reference_offset() {
        // Demonstrate bug: a fully specified local datetime string parses differently
        // when the reference has different offsets (winter vs summer).
        unsafe {
            std::env::set_var("TZ", "Europe/Paris");
        }
        let s = "2025-10-22 03:17"; // wall time in Paris

        let ref_winter = chrono::FixedOffset::east_opt(3600)
            .unwrap()
            .with_ymd_and_hms(2025, 12, 1, 12, 0, 0)
            .unwrap();
        let ref_summer = chrono::FixedOffset::east_opt(7200)
            .unwrap()
            .with_ymd_and_hms(2025, 7, 1, 12, 0, 0)
            .unwrap();

        let a = super::parse_with_reference(s, &ref_winter).expect("winter ref parse");
        let b = super::parse_with_reference(s, &ref_summer).expect("summer ref parse");

        assert_eq!(
            a, b,
            "Parsed times should be equal regardless of reference offset, got {} vs {}",
            a, b
        );
    }
}
