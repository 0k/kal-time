use chrono::{DateTime, FixedOffset, TimeZone};
use chrono::offset::Offset;
use chrono_english::{parse_date_string, Dialect};
use lazy_static::lazy_static;

mod parse;

lazy_static! {
    static ref TIMEPARSER_FORMATS: Vec<&'static str> = vec![
        // ISO 8601 with timezone offset (most specific first)
        "%Y-%m-%dT%H:%M:%S%:z",
        "%Y-%m-%dT%H:%M:%S%#z",
        "%Y-%m-%dT%H:%M%:z",
        "%Y-%m-%dT%H:%M%#z",
        // ISO 8601 with T separator (no timezone)
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        // Space-separated formats
        "%Y-%m-%d %H:%M:%S%:z",
        "%Y-%m-%d %H:%M:%S%#z",
        "%Y-%m-%d %H:%M%:z",
        "%Y-%m-%d %H:%M%#z",
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

fn parse_with_formats<'a, Tz: TimeZone + 'static>(
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

fn parse_with_reference_internal<Tz: TimeZone + 'static>(
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
        match parse::parse_partial(timestr, format, reference, true) {
            Ok(dt) => return Ok((dt, format)),
            Err(e) if e.contains("Ambiguous") || e.contains("Non-existent") => {
                // DST issues are real errors, don't try other formats
                return Err(e);
            }
            Err(_) => continue, // Format didn't match, try next
        }
    }

    // Fall back to chrono-english for natural language expressions
    log::trace!("Trying chrono-english for {:?}", timestr);
    let ref_fixed: DateTime<FixedOffset> = reference.with_timezone(&reference.offset().fix());
    if let Ok(dt) = parse_date_string(timestr, ref_fixed, Dialect::Uk) {
        log::trace!("chrono-english parsed {:?} as {:?}", timestr, dt);
        return Ok((dt, "chrono-english"));
    }

    Err(format!("Could not parse time string: {:?}", timestr))
}

pub fn parse_with_reference<Tz: TimeZone + 'static>(
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

pub fn parse_timespan_with_reference<Tz: TimeZone + 'static>(
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

    fn pp<Tz: TimeZone + 'static>(s: &str, dt: &DateTime<Tz>) -> String {
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

    /// Test all documented format examples from README.org
    /// Reference: 2024-03-20T11:45:30+05:00
    #[test]
    fn test_documented_formats() {
        let offset = chrono::FixedOffset::east_opt(5 * 3600).unwrap();
        let dt = offset.with_ymd_and_hms(2024, 3, 20, 11, 45, 30).unwrap();

        // === ISO 8601 with timezone (reference ignored) ===
        assert_eq!(pp("2025-01-12T14:30:00+01:00", &dt), "Ok(2025-01-12T14:30:00+01:00)");
        assert_eq!(pp("2025-01-12T14:30:00Z", &dt), "Ok(2025-01-12T14:30:00+00:00)");
        assert_eq!(pp("2025-01-12T14:30+01:00", &dt), "Ok(2025-01-12T14:30:00+01:00)");
        assert_eq!(pp("2025-01-12T14:30Z", &dt), "Ok(2025-01-12T14:30:00+00:00)");
        assert_eq!(pp("2025-01-12 14:30:00+01:00", &dt), "Ok(2025-01-12T14:30:00+01:00)");
        assert_eq!(pp("2025-01-12 14:30:00Z", &dt), "Ok(2025-01-12T14:30:00+00:00)");

        // === ISO 8601 no timezone (offset +05:00 from reference) ===
        assert_eq!(pp("2025-01-12T14:30:00", &dt), "Ok(2025-01-12T14:30:00+05:00)");
        assert_eq!(pp("2025-01-12T14:30", &dt), "Ok(2025-01-12T14:30:00+05:00)");
        assert_eq!(pp("2025-01-12 14:30:00", &dt), "Ok(2025-01-12T14:30:00+05:00)");
        assert_eq!(pp("2025-01-12 14:30", &dt), "Ok(2025-01-12T14:30:00+05:00)");
        assert_eq!(pp("2025-01-12", &dt), "Ok(2025-01-12T00:00:00+05:00)");

        // === Partial date (year=2024, offset=+05:00 from ref) ===
        assert_eq!(pp("01-12", &dt), "Ok(2024-01-12T00:00:00+05:00)");
        assert_eq!(pp("01/12", &dt), "Ok(2024-01-12T00:00:00+05:00)");
        assert_eq!(pp("01-12 14:30", &dt), "Ok(2024-01-12T14:30:00+05:00)");
        assert_eq!(pp("01-12 14:30:45", &dt), "Ok(2024-01-12T14:30:45+05:00)");
        assert_eq!(pp("15 14:30", &dt), "Ok(2024-03-15T14:30:00+05:00)"); // year+month from ref

        // === Time only (date 2024-03-20, offset +05:00 from ref) ===
        assert_eq!(pp("14:30:59", &dt), "Ok(2024-03-20T14:30:59+05:00)");
        assert_eq!(pp("14:30", &dt), "Ok(2024-03-20T14:30:00+05:00)");

        // === Terse formats ===
        assert_eq!(pp("14h30", &dt), "Ok(2024-03-20T14:30:00+05:00)"); // date from ref
        assert_eq!(pp("9h", &dt), "Ok(2024-03-20T09:00:00+05:00)");    // date from ref
        assert_eq!(pp("30m", &dt), "Ok(2024-03-20T11:30:00+05:00)");   // date+hour(11) from ref
        assert_eq!(pp("15 14h30", &dt), "Ok(2024-03-15T14:30:00+05:00)"); // year+month from ref
        assert_eq!(pp("15 9h", &dt), "Ok(2024-03-15T09:00:00+05:00)");    // year+month from ref

        // === Unix timestamp (always UTC, reference ignored) ===
        assert_eq!(pp("@1736692200", &dt), "Ok(2025-01-12T14:30:00+00:00)");
    }

    #[test]
    fn test_natural_language_formats() {
        // Reference: Wednesday 2024-03-20 11:45:30+05:00
        let offset = chrono::FixedOffset::east_opt(5 * 3600).unwrap();
        let dt = offset.with_ymd_and_hms(2024, 3, 20, 11, 45, 30).unwrap();

        // Relative days
        assert_eq!(pp("yesterday", &dt), "Ok(2024-03-19T11:45:30+05:00)");
        assert_eq!(pp("tomorrow", &dt), "Ok(2024-03-21T11:45:30+05:00)");

        // Weekdays (reference is Wednesday)
        assert_eq!(pp("friday", &dt), "Ok(2024-03-22T00:00:00+05:00)"); // this friday
        assert_eq!(pp("last friday", &dt), "Ok(2024-03-15T00:00:00+05:00)");
        assert_eq!(pp("next monday", &dt), "Ok(2024-04-01T00:00:00+05:00)");

        // Intervals
        assert_eq!(pp("2 days ago", &dt), "Ok(2024-03-18T11:45:30+05:00)");
        assert_eq!(pp("3 hours ago", &dt), "Ok(2024-03-20T08:45:30+05:00)");
        assert_eq!(pp("1 hour", &dt), "Ok(2024-03-20T12:45:30+05:00)");

        // With time
        assert_eq!(pp("friday 8pm", &dt), "Ok(2024-03-22T20:00:00+05:00)");

        // Month names
        assert_eq!(pp("april 1", &dt), "Ok(2024-04-01T00:00:00+05:00)");
        assert_eq!(pp("1 april", &dt), "Ok(2024-04-01T00:00:00+05:00)");
    }

    #[test]
    fn test_offset_comes_from_reference() {
        // When no timezone is specified in input and reference has an explicit
        // fixed offset (not matching local TZ), that offset is preserved.
        // Use +05:00 and +05:30 which don't match common European timezones.
        let s = "2025-10-22 03:17";

        let ref_plus5 = chrono::FixedOffset::east_opt(5 * 3600)
            .unwrap()
            .with_ymd_and_hms(2025, 12, 1, 12, 0, 0)
            .unwrap();
        let ref_plus5_30 = chrono::FixedOffset::east_opt(5 * 3600 + 1800)
            .unwrap()
            .with_ymd_and_hms(2025, 7, 1, 12, 0, 0)
            .unwrap();

        let a = super::parse_with_reference(s, &ref_plus5).expect("+05:00 ref parse");
        let b = super::parse_with_reference(s, &ref_plus5_30).expect("+05:30 ref parse");

        // Offset comes from reference (not from system local TZ)
        assert_eq!(a.offset().local_minus_utc(), 5 * 3600, "should use +05:00 from ref");
        assert_eq!(b.offset().local_minus_utc(), 5 * 3600 + 1800, "should use +05:30 from ref");
        // Same wall time but different offsets = different instants
        assert_ne!(a, b);
    }

    #[test]
    fn test_dst_ambiguous_time_errors() {
        // In Europe/Paris, October 26 2025 at 2:30 AM is ambiguous:
        // clocks go back from 3:00 to 2:00, so 2:30 exists twice
        unsafe {
            std::env::set_var("TZ", "Europe/Paris");
        }
        // Force chrono to re-read TZ
        tzset_refresh();

        // Use Local reference to trigger DST-aware parsing
        let reference = chrono::Local.with_ymd_and_hms(2025, 10, 26, 0, 0, 0).unwrap();

        // Check what Local thinks about this time
        let naive = chrono::NaiveDateTime::parse_from_str(
            "2025-10-26 02:30:00",
            "%Y-%m-%d %H:%M:%S",
        )
        .unwrap();
        let local_result = chrono::Local.from_local_datetime(&naive);
        eprintln!("TZ={:?}, Local result for {}: {:?}",
            std::env::var("TZ"), naive, local_result);

        // 2:30 AM on Oct 26 2025 is ambiguous - should error
        let result = super::parse_with_reference("2025-10-26 02:30:00", &reference);
        assert!(
            result.is_err(),
            "Ambiguous DST time should return error, got {:?}",
            result
        );
        assert!(
            result.unwrap_err().contains("Ambiguous"),
            "Error should mention ambiguity"
        );

        // 1:59:59 AM is unambiguous (before transition)
        let before = super::parse_with_reference("2025-10-26 01:59:59", &reference);
        assert!(before.is_ok(), "Time before DST transition should parse: {:?}", before);

        // 3:00:00 AM is also ambiguous (it exists as both 3:00 CEST before fallback and 3:00 CET after)
        let at_three = super::parse_with_reference("2025-10-26 03:00:00", &reference);
        assert!(at_three.is_err(), "3:00 AM should also be ambiguous: {:?}", at_three);

        // 3:00:01 AM is unambiguous (just after the ambiguous window ends)
        let after = super::parse_with_reference("2025-10-26 03:00:01", &reference);
        assert!(after.is_ok(), "Time after DST transition should parse: {:?}", after);
    }

    #[test]
    fn test_dst_nonexistent_time_errors() {
        // In Europe/Paris, March 30 2025 at 2:30 AM doesn't exist:
        // clocks jump from 2:00 to 3:00 (spring forward)
        unsafe {
            std::env::set_var("TZ", "Europe/Paris");
        }
        tzset_refresh();

        // Use Local reference to trigger DST-aware parsing
        let reference = chrono::Local.with_ymd_and_hms(2025, 3, 30, 0, 0, 0).unwrap();

        // 2:30 AM on March 30 2025 doesn't exist - should error
        let result = super::parse_with_reference("2025-03-30 02:30:00", &reference);
        assert!(
            result.is_err(),
            "Non-existent DST time should return error, got {:?}",
            result
        );
        assert!(
            result.unwrap_err().contains("Non-existent"),
            "Error should mention non-existent"
        );

        // 1:59:59 AM is fine (before the gap)
        let before = super::parse_with_reference("2025-03-30 01:59:59", &reference);
        assert!(before.is_ok(), "Time before DST gap should parse: {:?}", before);

        // 3:00:00 AM is fine (after the gap)
        let after = super::parse_with_reference("2025-03-30 03:00:00", &reference);
        assert!(after.is_ok(), "Time after DST gap should parse: {:?}", after);
    }

    fn tzset_refresh() {
        // Call libc tzset to refresh timezone info after TZ env change
        unsafe extern "C" {
            fn tzset();
        }
        unsafe {
            tzset();
        }
    }

    #[test]
    fn test_dst_with_explicit_offset_no_error() {
        // When input has explicit timezone, DST ambiguity doesn't apply
        unsafe {
            std::env::set_var("TZ", "Europe/Paris");
        }

        let reference = Utc.with_ymd_and_hms(2025, 10, 26, 0, 0, 0).unwrap();

        // 2:30 AM with explicit +01:00 (winter time) - should succeed
        let winter = super::parse_with_reference("2025-10-26T02:30:00+01:00", &reference);
        assert!(winter.is_ok(), "Explicit +01:00 should parse: {:?}", winter);

        // 2:30 AM with explicit +02:00 (summer time) - should succeed
        let summer = super::parse_with_reference("2025-10-26T02:30:00+02:00", &reference);
        assert!(summer.is_ok(), "Explicit +02:00 should parse: {:?}", summer);

        // They should represent different instants
        assert_ne!(winter.unwrap(), summer.unwrap());
    }

    #[test]
    fn test_parsed_time_uses_target_date_dst_offset() {
        use chrono::Timelike;

        // When parsing a date without explicit offset, the resulting offset
        // should match the local timezone's DST for that target date,
        // not the reference's offset.
        unsafe {
            std::env::set_var("TZ", "Europe/Paris");
        }
        tzset_refresh();

        // Winter reference: December 2025 in Paris is UTC+1
        let winter_ref = chrono::Local
            .with_ymd_and_hms(2025, 12, 1, 12, 0, 0)
            .unwrap();
        assert_eq!(
            winter_ref.offset().local_minus_utc(),
            3600,
            "Winter reference should be UTC+1"
        );

        // Parse a summer date (July 2025) - Paris is UTC+2 in summer
        let summer_time = super::parse_with_reference("2025-07-01 03:17", &winter_ref)
            .expect("should parse summer date");

        // The parsed time should have the summer offset (UTC+2),
        // not the winter reference's offset (UTC+1)
        assert_eq!(
            summer_time.offset().local_minus_utc(),
            7200,
            "Parsed summer time should have UTC+2 offset, not reference's UTC+1"
        );

        // Wall clock time should be preserved: 03:17 in input = 03:17 in output
        assert_eq!(summer_time.hour(), 3);
        assert_eq!(summer_time.minute(), 17);
    }
}
