use chrono::offset::Offset;
use chrono::{DateTime, Duration, FixedOffset, Months, TimeZone};
use chrono_english::{parse_date_string, Dialect};
use lazy_static::lazy_static;
use two_timer::{parse as two_timer_parse, Config as TwoTimerConfig};

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
        // Bare year-month / year placed AT THE END so they only catch
        // inputs that no more-specific format accepted. `01-12` matches
        // `%m-%d` first; `2024-10-01` matches `%Y-%m-%d` first.
        "%Y-%m",
        "%Y",
    ];
}

/// The natural period attached to a parsed instant, derived from the
/// smallest explicitly-specified field of the matched format string.
///
/// Used by `parse_timespan_with_reference` to compute `R = L + 1 unit`
/// for half-open `[L, R)` intervals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PeriodUnit {
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
}

/// Read the rightmost time-field specifier of a chrono format string and
/// return the corresponding `PeriodUnit`. Skips offset specifiers
/// (`%:z`, `%#z`) which carry no precision.
///
/// **Panics** if `fmt` contains no recognized time-field specifier. This
/// is a programmer-error invariant: every entry in `TIMEPARSER_FORMATS`
/// (and every suffix derived from it via `generate_suffix_formats`)
/// contains at least one such specifier.
pub(crate) fn unit_from_format(fmt: &str) -> PeriodUnit {
    let bytes = fmt.as_bytes();
    let mut i = 0;
    let mut last_unit: Option<PeriodUnit> = None;
    while i < bytes.len() {
        if bytes[i] != b'%' {
            i += 1;
            continue;
        }
        if i + 1 >= bytes.len() {
            break;
        }
        let spec = bytes[i + 1];
        // Skip offset specifiers: %:z and %#z (3 bytes each, no precision)
        if (spec == b':' || spec == b'#') && i + 2 < bytes.len() && bytes[i + 2] == b'z' {
            i += 3;
            continue;
        }
        match spec {
            b'Y' => last_unit = Some(PeriodUnit::Year),
            b'm' => last_unit = Some(PeriodUnit::Month),
            b'd' => last_unit = Some(PeriodUnit::Day),
            b'H' => last_unit = Some(PeriodUnit::Hour),
            b'M' => last_unit = Some(PeriodUnit::Minute),
            b'S' | b's' => last_unit = Some(PeriodUnit::Second),
            _ => {} // unknown/other specifier; ignore
        }
        i += 2;
    }
    last_unit.unwrap_or_else(|| panic!("format string {:?} contains no time-field specifier", fmt))
}

/// Compute `R = L + one unit` for a half-open period.
///
/// For `Year`/`Month`, calendar arithmetic is used (`Months::new`).
/// When the reference is `DateTime<Local>`, the resulting `R`'s offset
/// is recomputed via the system timezone for the target date, so that
/// e.g. `2024-03` under Paris correctly produces a stop with the summer
/// offset (DST-aware). For `FixedOffset`/`Utc` references, the start's
/// offset is preserved (DST-blind).
///
/// For `Day`/`Hour`/`Minute`/`Second`, plain `chrono::Duration`
/// arithmetic is used (intentionally DST-blind — `[09:00, 10:00)` is
/// always 3600 wall-clock-aware seconds in the captured offset).
///
/// Returns `Err` if calendar arithmetic overflows or if the resulting
/// boundary lands inside a DST gap that cannot be resolved by the
/// captured offset (both unreachable for typical period-1 boundaries
/// from the format-table parsers, but signalled cleanly to the caller
/// instead of panicking).
fn add_period_unit<Tz: TimeZone + 'static>(
    unit: PeriodUnit,
    l: DateTime<FixedOffset>,
    _reference: &DateTime<Tz>,
) -> Result<DateTime<FixedOffset>, String> {
    match unit {
        PeriodUnit::Year | PeriodUnit::Month => {
            let months = if matches!(unit, PeriodUnit::Year) {
                12
            } else {
                1
            };
            let stop_naive = l
                .naive_local()
                .checked_add_months(Months::new(months))
                .ok_or_else(|| {
                    format!(
                        "Calendar arithmetic overflow when adding {} month(s) to {}",
                        months,
                        l.naive_local()
                    )
                })?;
            let ref_is_local =
                std::any::TypeId::of::<Tz>() == std::any::TypeId::of::<chrono::Local>();
            if ref_is_local {
                match chrono::Local.from_local_datetime(&stop_naive) {
                    chrono::LocalResult::Single(dt) => Ok(dt.with_timezone(&dt.offset().fix())),
                    // Period boundary inside DST gap/fold (not realistic
                    // for month-1 boundaries; defensive fallback).
                    chrono::LocalResult::Ambiguous(a, _) => Ok(a.with_timezone(&a.offset().fix())),
                    chrono::LocalResult::None => l
                        .offset()
                        .from_local_datetime(&stop_naive)
                        .single()
                        .ok_or_else(|| {
                            format!("Non-existent local time at period boundary: {}", stop_naive)
                        }),
                }
            } else {
                l.offset()
                    .from_local_datetime(&stop_naive)
                    .single()
                    .ok_or_else(|| {
                        format!("Non-existent local time at period boundary: {}", stop_naive)
                    })
            }
        }
        PeriodUnit::Day => Ok(l + Duration::days(1)),
        PeriodUnit::Hour => Ok(l + Duration::hours(1)),
        PeriodUnit::Minute => Ok(l + Duration::minutes(1)),
        PeriodUnit::Second => Ok(l + Duration::seconds(1)),
    }
}

/// Internal representation of a single `..`-operand or bare-input piece.
enum Operand {
    /// Empty string (e.g. the right half of `today..`).
    Empty,
    /// The literal `now` token (case-insensitive).
    Now,
    /// A parsed period `[L, R)`.
    Period(DateTime<FixedOffset>, DateTime<FixedOffset>),
}

/// Resolve a single timespan operand to its half-open natural period.
///
/// Two references are accepted because the `..` branch needs different
/// anchoring for different parser stages:
///
///   - `format_ref` is used for the format-table parsers (suffix
///     formats AND `TIMEPARSER_FORMATS`). For the right operand of
///     `P..Q`, this is the resolved start instant — so terse inputs
///     like `30` in `10:15..30` inherit start's fields.
///   - `nlp_ref` is used for `two_timer` and `chrono-english`. These
///     interpret natural-language tokens like `today` relative to the
///     **user's current moment**, not relative to start; otherwise
///     `2026-04-30..today` would resolve `today` against
///     `2026-04-30` and mean "the day of start" (a useless tautology).
///
/// For bare inputs (no `..`), the caller passes the same reference for
/// both.
///
/// Resolution order (first match wins):
///   1. Empty string → `Operand::Empty` (sentinel).
///   2. Case-insensitive `now` → `Operand::Now` (sentinel).
///   3. `extra_formats` (suffix formats; uses `format_ref`).
///   4. `TIMEPARSER_FORMATS` (uses `format_ref`).
///   5. `two_timer` (uses `nlp_ref`).
///   6. `chrono-english` (uses `nlp_ref`).
///
/// Returns `(Operand, Option<&'static str>)` where the second element
/// is the matched static format string when step 4 succeeded.
fn resolve_operand<Tz: TimeZone + 'static, Tn: TimeZone + 'static>(
    s: &str,
    format_ref: &DateTime<Tz>,
    nlp_ref: &DateTime<Tn>,
    extra_formats: &[&str],
) -> Result<(Operand, Option<&'static str>), String> {
    if s.is_empty() {
        return Ok((Operand::Empty, None));
    }
    if s.eq_ignore_ascii_case("now") {
        return Ok((Operand::Now, None));
    }

    // 1. Suffix formats from the left operand (tried FIRST).
    for fmt in extra_formats.iter() {
        match parse::parse_partial(s, fmt, format_ref, true) {
            Ok(dt) => {
                let unit = unit_from_format(fmt);
                let r = add_period_unit(unit, dt, format_ref)?;
                return Ok((Operand::Period(dt, r), None));
            }
            Err(e) if e.contains("Ambiguous") || e.contains("Non-existent") => {
                return Err(e);
            }
            Err(_) => continue,
        }
    }

    // 2. Static format table.
    for fmt in TIMEPARSER_FORMATS.iter() {
        match parse::parse_partial(s, fmt, format_ref, true) {
            Ok(dt) => {
                let unit = unit_from_format(fmt);
                let r = add_period_unit(unit, dt, format_ref)?;
                return Ok((Operand::Period(dt, r), Some(*fmt)));
            }
            Err(e) if e.contains("Ambiguous") || e.contains("Non-existent") => {
                return Err(e);
            }
            Err(_) => continue,
        }
    }

    // 3. two-timer (calendar-aligned NLP periods, anchored at nlp_ref).
    let nlp_offset = nlp_ref.offset().fix();
    let naive_now = nlp_ref.with_timezone(&nlp_offset).naive_local();
    let config = TwoTimerConfig::new().now(naive_now);
    if let Ok((start_naive, end_naive, _)) = two_timer_parse(s, Some(config)) {
        let l = nlp_offset
            .from_local_datetime(&start_naive)
            .single()
            .ok_or_else(|| {
                format!(
                    "Ambiguous or non-existent local time for start: {}",
                    start_naive
                )
            })?;
        let r = nlp_offset
            .from_local_datetime(&end_naive)
            .single()
            .ok_or_else(|| {
                format!(
                    "Ambiguous or non-existent local time for end: {}",
                    end_naive
                )
            })?;
        return Ok((Operand::Period(l, r), None));
    }

    // 4. chrono-english (legacy single-instant NLP, anchored at nlp_ref).
    let nlp_fixed: DateTime<FixedOffset> = nlp_ref.with_timezone(&nlp_offset);
    if let Ok(dt) = parse_date_string(s, nlp_fixed, Dialect::Uk) {
        return Ok((Operand::Period(dt, dt + Duration::seconds(1)), None));
    }

    Err(format!("Could not parse timespan operand: {:?}", s))
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

/// Parse a single time string into a `DateTime<FixedOffset>`, filling
/// missing fields from `reference`.
///
/// The input is matched against an internal format table (ISO 8601
/// variants, partial dates, time-only, terse forms like `9h`/`30m`,
/// `@<unix>`, …) and falls back to `chrono-english` for natural-language
/// expressions (`yesterday`, `next monday`, `2 days ago`, …).
///
/// Missing fields are filled from `reference` from coarsest to finest,
/// stopping at the first explicitly-specified field. So `9h` against
/// `2024-03-20 11:45:30+05:00` keeps the date and yields
/// `2024-03-20 09:00:00+05:00`. The output offset comes from any
/// explicit `%z`/`%:z`/`Z` in the input; otherwise it is derived from
/// `reference`'s timezone (DST-aware when `reference: DateTime<Local>`,
/// DST-blind for `DateTime<FixedOffset>`/`DateTime<Utc>`).
///
/// # Errors
///
/// Returns `Err` if the input matches no known format, or if the
/// resulting local time is ambiguous / non-existent under DST when the
/// reference is `DateTime<Local>`.
///
/// # Example
///
/// ```
/// use chrono::{TimeZone, Utc};
/// use kal_time::parse_with_reference;
///
/// let reference = Utc.with_ymd_and_hms(2025, 10, 22, 9, 10, 11).unwrap();
/// let parsed = parse_with_reference("30m", &reference).unwrap();
/// assert_eq!(parsed.to_rfc3339(), "2025-10-22T09:30:00+00:00");
/// ```
pub fn parse_with_reference<Tz: TimeZone + 'static>(
    timestr: &str,
    reference: &DateTime<Tz>,
) -> Result<DateTime<FixedOffset>, String> {
    parse_with_reference_internal(timestr, reference).map(|(dt, _fmt)| dt)
}

/// Parse a time string against the current local-time reference.
///
/// Equivalent to [`parse_with_reference`] using `chrono::Local::now()`
/// as the reference. The output offset matches the system timezone for
/// the resolved instant (DST-aware).
///
/// # Errors
///
/// Same as [`parse_with_reference`].
pub fn parse(timespan: &str) -> Result<DateTime<FixedOffset>, String> {
    let now = chrono::Local::now();
    parse_with_reference(timespan, &now)
}

/// Parse a time string against the current UTC reference.
///
/// Equivalent to [`parse_with_reference`] using `chrono::Utc::now()`
/// as the reference. Missing fields and missing offsets default to UTC
/// (`+00:00`).
///
/// # Errors
///
/// Same as [`parse_with_reference`].
pub fn parse_utc(timespan: &str) -> Result<DateTime<FixedOffset>, String> {
    let now = chrono::Utc::now();
    parse_with_reference(timespan, &now)
}

/// Half-open interval `[start, stop)` returned by
/// [`parse_timespan`] / [`parse_timespan_with_reference`].
pub type Timespan = (DateTime<FixedOffset>, DateTime<FixedOffset>);

/// Parse a timespan string into a half-open `[start, stop)` interval,
/// using `default` as the reference for relative inputs.
///
/// Each operand resolves to its own natural period derived from the
/// smallest explicitly-specified field (the *precision rule*):
///
/// | Smallest specifier | Period length |
/// | `%Y`               | 1 calendar year  |
/// | `%m`               | 1 calendar month |
/// | `%d`               | 1 day            |
/// | `%H`               | 1 hour           |
/// | `%M`               | 1 minute         |
/// | `%S` / `%s`        | 1 second         |
///
/// Calendar tokens (`today`, `yesterday`, `last friday`, `this week`,
/// …) keep their `two-timer` boundaries; `now` (case-insensitive)
/// resolves to the reference instant.
///
/// Composition (with `..`):
///
/// | Input shape | Result                            |
/// | bare `P`    | `[L(P), R(P))`                    |
/// | `P..Q`      | `[L(P), L(Q))` — right uses LEFT edge |
/// | `X..`       | `[L(X), now)`                     |
///
/// # Errors
///
/// Returns `Err` for: unparseable operands, leading-empty `..X`, bare
/// `now` / empty input, or any span where `start >= stop`.
///
/// # Example
///
/// ```
/// use chrono::{TimeZone, Utc};
/// use kal_time::parse_timespan_with_reference;
///
/// let reference = Utc.with_ymd_and_hms(2025, 10, 22, 9, 10, 11).unwrap();
/// let (start, stop) = parse_timespan_with_reference("9h..10h", &reference).unwrap();
/// assert_eq!(start.to_rfc3339(), "2025-10-22T09:00:00+00:00");
/// assert_eq!(stop.to_rfc3339(), "2025-10-22T10:00:00+00:00");
/// ```
pub fn parse_timespan_with_reference<Tz: TimeZone + 'static>(
    timespan: &str,
    default: &DateTime<Tz>,
) -> Result<Timespan, String> {
    let ref_instant: DateTime<FixedOffset> = default.with_timezone(&default.offset().fix());

    let (start, stop) = match timespan.split_once("..") {
        Some((left_str, right_str)) => {
            // Left operand: use `default` for both format-table and NLP
            // (no field-inheritance possible — it's the leftmost piece).
            let (left, left_matched_fmt) = resolve_operand(left_str, default, default, &[])?;
            let start = match left {
                Operand::Empty => {
                    return Err(format!(
                        "Invalid timespan {:?}: leading-empty (`..X`) is not supported",
                        timespan
                    ));
                }
                Operand::Now => ref_instant,
                Operand::Period(l, _) => l,
            };

            // Right operand: format-table parsers anchor at `start` so
            // terse formats like `30` in `10:15..30` inherit start's
            // hour; NLP parsers anchor at `default` so `today` means
            // "today relative to the user's now", not "relative to
            // start". Suffix formats derived from the left's matched
            // format are tried FIRST inside resolve_operand.
            let suffix_owned: Vec<String> = match left_matched_fmt {
                Some(fmt) => generate_suffix_formats(fmt),
                None => Vec::new(),
            };
            let suffix_refs: Vec<&str> = suffix_owned.iter().map(|s| s.as_str()).collect();
            let (right, _) = resolve_operand(right_str, &start, default, &suffix_refs)?;
            let stop = match right {
                Operand::Empty => ref_instant, // X.. shorthand for X..now
                Operand::Now => ref_instant,
                Operand::Period(l, _) => l, // RIGHT uses LEFT edge
            };

            (start, stop)
        }
        None => {
            let (op, _) = resolve_operand(timespan, default, default, &[])?;
            match op {
                Operand::Empty | Operand::Now => {
                    return Err(format!(
                        "Invalid timespan {:?}: bare `now` / empty input is not a period",
                        timespan
                    ));
                }
                Operand::Period(l, r) => (l, r),
            }
        }
    };

    if start >= stop {
        return Err(format!(
            "Invalid timespan {:?}: start ({}) is not strictly before stop ({})",
            timespan,
            start.format("%Y-%m-%d %H:%M:%S %z"),
            stop.format("%Y-%m-%d %H:%M:%S %z")
        ));
    }

    Ok((start, stop))
}

/// Parse a timespan string against the current local-time reference.
///
/// Equivalent to [`parse_timespan_with_reference`] using
/// `chrono::Local::now()` as the reference.
///
/// # Errors
///
/// Same as [`parse_timespan_with_reference`].
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
        assert_eq!(
            pp("2025-01-12T14:30:00+01:00", &dt),
            "Ok(2025-01-12T14:30:00+01:00)"
        );
        assert_eq!(
            pp("2025-01-12T14:30:00Z", &dt),
            "Ok(2025-01-12T14:30:00+00:00)"
        );
        assert_eq!(
            pp("2025-01-12T14:30+01:00", &dt),
            "Ok(2025-01-12T14:30:00+01:00)"
        );
        assert_eq!(
            pp("2025-01-12T14:30Z", &dt),
            "Ok(2025-01-12T14:30:00+00:00)"
        );
        assert_eq!(
            pp("2025-01-12 14:30:00+01:00", &dt),
            "Ok(2025-01-12T14:30:00+01:00)"
        );
        assert_eq!(
            pp("2025-01-12 14:30:00Z", &dt),
            "Ok(2025-01-12T14:30:00+00:00)"
        );

        // === ISO 8601 no timezone (offset +05:00 from reference) ===
        assert_eq!(
            pp("2025-01-12T14:30:00", &dt),
            "Ok(2025-01-12T14:30:00+05:00)"
        );
        assert_eq!(pp("2025-01-12T14:30", &dt), "Ok(2025-01-12T14:30:00+05:00)");
        assert_eq!(
            pp("2025-01-12 14:30:00", &dt),
            "Ok(2025-01-12T14:30:00+05:00)"
        );
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
        assert_eq!(pp("9h", &dt), "Ok(2024-03-20T09:00:00+05:00)"); // date from ref
        assert_eq!(pp("30m", &dt), "Ok(2024-03-20T11:30:00+05:00)"); // date+hour(11) from ref
        assert_eq!(pp("15 14h30", &dt), "Ok(2024-03-15T14:30:00+05:00)"); // year+month from ref
        assert_eq!(pp("15 9h", &dt), "Ok(2024-03-15T09:00:00+05:00)"); // year+month from ref

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
        assert_eq!(
            a.offset().local_minus_utc(),
            5 * 3600,
            "should use +05:00 from ref"
        );
        assert_eq!(
            b.offset().local_minus_utc(),
            5 * 3600 + 1800,
            "should use +05:30 from ref"
        );
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
        let reference = chrono::Local
            .with_ymd_and_hms(2025, 10, 26, 0, 0, 0)
            .unwrap();

        // Check what Local thinks about this time
        let naive =
            chrono::NaiveDateTime::parse_from_str("2025-10-26 02:30:00", "%Y-%m-%d %H:%M:%S")
                .unwrap();
        let local_result = chrono::Local.from_local_datetime(&naive);
        eprintln!(
            "TZ={:?}, Local result for {}: {:?}",
            std::env::var("TZ"),
            naive,
            local_result
        );

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
        assert!(
            before.is_ok(),
            "Time before DST transition should parse: {:?}",
            before
        );

        // 3:00:00 AM is also ambiguous (it exists as both 3:00 CEST before fallback and 3:00 CET after)
        let at_three = super::parse_with_reference("2025-10-26 03:00:00", &reference);
        assert!(
            at_three.is_err(),
            "3:00 AM should also be ambiguous: {:?}",
            at_three
        );

        // 3:00:01 AM is unambiguous (just after the ambiguous window ends)
        let after = super::parse_with_reference("2025-10-26 03:00:01", &reference);
        assert!(
            after.is_ok(),
            "Time after DST transition should parse: {:?}",
            after
        );
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
        let reference = chrono::Local
            .with_ymd_and_hms(2025, 3, 30, 0, 0, 0)
            .unwrap();

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
        assert!(
            before.is_ok(),
            "Time before DST gap should parse: {:?}",
            before
        );

        // 3:00:00 AM is fine (after the gap)
        let after = super::parse_with_reference("2025-03-30 03:00:00", &reference);
        assert!(
            after.is_ok(),
            "Time after DST gap should parse: {:?}",
            after
        );
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

    #[test]
    fn test_timespan_natural_language_full_periods() {
        // Reference: Wednesday 2024-03-20 11:45:30+05:00
        let offset = chrono::FixedOffset::east_opt(5 * 3600).unwrap();
        let dt = offset.with_ymd_and_hms(2024, 3, 20, 11, 45, 30).unwrap();

        // "today" should be full day: 2024-03-20 00:00:00 .. 2024-03-21 00:00:00
        let (start, stop) = parse_timespan_with_reference("today", &dt).unwrap();
        assert_eq!(
            start,
            offset.with_ymd_and_hms(2024, 3, 20, 0, 0, 0).unwrap()
        );
        assert_eq!(stop, offset.with_ymd_and_hms(2024, 3, 21, 0, 0, 0).unwrap());

        // "yesterday" should be full day: 2024-03-19 00:00:00 .. 2024-03-20 00:00:00
        let (start, stop) = parse_timespan_with_reference("yesterday", &dt).unwrap();
        assert_eq!(
            start,
            offset.with_ymd_and_hms(2024, 3, 19, 0, 0, 0).unwrap()
        );
        assert_eq!(stop, offset.with_ymd_and_hms(2024, 3, 20, 0, 0, 0).unwrap());

        // "tomorrow" should be full day: 2024-03-21 00:00:00 .. 2024-03-22 00:00:00
        let (start, stop) = parse_timespan_with_reference("tomorrow", &dt).unwrap();
        assert_eq!(
            start,
            offset.with_ymd_and_hms(2024, 3, 21, 0, 0, 0).unwrap()
        );
        assert_eq!(stop, offset.with_ymd_and_hms(2024, 3, 22, 0, 0, 0).unwrap());

        // "last friday" should be full day: 2024-03-15
        let (start, stop) = parse_timespan_with_reference("last friday", &dt).unwrap();
        assert_eq!(
            start,
            offset.with_ymd_and_hms(2024, 3, 15, 0, 0, 0).unwrap()
        );
        assert_eq!(stop, offset.with_ymd_and_hms(2024, 3, 16, 0, 0, 0).unwrap());
    }

    #[test]
    fn test_timespan_natural_language_periods() {
        // Reference: Wednesday 2024-03-20 11:45:30+05:00
        let offset = chrono::FixedOffset::east_opt(5 * 3600).unwrap();
        let dt = offset.with_ymd_and_hms(2024, 3, 20, 11, 45, 30).unwrap();

        // "this week" = Monday 2024-03-18 00:00 .. Monday 2024-03-25 00:00
        let (start, stop) = parse_timespan_with_reference("this week", &dt).unwrap();
        assert_eq!(
            start,
            offset.with_ymd_and_hms(2024, 3, 18, 0, 0, 0).unwrap()
        );
        assert_eq!(stop, offset.with_ymd_and_hms(2024, 3, 25, 0, 0, 0).unwrap());

        // "this month" = 2024-03-01 00:00 .. 2024-04-01 00:00
        let (start, stop) = parse_timespan_with_reference("this month", &dt).unwrap();
        assert_eq!(start, offset.with_ymd_and_hms(2024, 3, 1, 0, 0, 0).unwrap());
        assert_eq!(stop, offset.with_ymd_and_hms(2024, 4, 1, 0, 0, 0).unwrap());

        // "this year" = 2024-01-01 00:00 .. 2025-01-01 00:00
        let (start, stop) = parse_timespan_with_reference("this year", &dt).unwrap();
        assert_eq!(start, offset.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap());
        assert_eq!(stop, offset.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap());
    }
}

/// Tests for the strengthened timespan semantics (precision-period rule,
/// `now` token, `X..` open-ended shorthand, position-based L/R rule).
/// Locked spec lives at `.sisyphus/plans/strengthen-timespan.md` §1.
#[cfg(test)]
mod timespan_strengthening {
    use super::*;
    use chrono::FixedOffset;

    /// Canonical reference: Friday 2026-05-01 15:00 +02:00.
    fn friday_ref() -> DateTime<FixedOffset> {
        FixedOffset::east_opt(2 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 5, 1, 15, 0, 0)
            .unwrap()
    }

    /// Construct `[start, stop)` from year/month/day/hour/minute/second
    /// pairs at the given offset (seconds east of UTC).
    fn span(
        offset_secs: i32,
        s: (i32, u32, u32, u32, u32, u32),
        e: (i32, u32, u32, u32, u32, u32),
    ) -> (DateTime<FixedOffset>, DateTime<FixedOffset>) {
        let off = FixedOffset::east_opt(offset_secs).unwrap();
        (
            off.with_ymd_and_hms(s.0, s.1, s.2, s.3, s.4, s.5).unwrap(),
            off.with_ymd_and_hms(e.0, e.1, e.2, e.3, e.4, e.5).unwrap(),
        )
    }

    fn ts(input: &str) -> (DateTime<FixedOffset>, DateTime<FixedOffset>) {
        parse_timespan_with_reference(input, &friday_ref())
            .unwrap_or_else(|e| panic!("parse {:?} failed: {}", input, e))
    }

    fn ts_err(input: &str) -> String {
        parse_timespan_with_reference(input, &friday_ref())
            .err()
            .unwrap_or_else(|| panic!("parse {:?} should have errored", input))
    }

    // ===== Bare-period: precision rule =====

    #[test]
    fn test_bare_year() {
        // friday_ref offset is +02:00 (7200s); bare instants inherit it.
        assert_eq!(
            ts("2024"),
            span(7200, (2024, 1, 1, 0, 0, 0), (2025, 1, 1, 0, 0, 0))
        );
    }

    #[test]
    fn test_bare_year_month() {
        assert_eq!(
            ts("2024-10"),
            span(7200, (2024, 10, 1, 0, 0, 0), (2024, 11, 1, 0, 0, 0))
        );
    }

    #[test]
    fn test_bare_full_date() {
        assert_eq!(
            ts("2024-10-01"),
            span(7200, (2024, 10, 1, 0, 0, 0), (2024, 10, 2, 0, 0, 0))
        );
    }

    #[test]
    fn test_bare_hour_terse() {
        assert_eq!(
            ts("9h"),
            span(7200, (2026, 5, 1, 9, 0, 0), (2026, 5, 1, 10, 0, 0))
        );
    }

    #[test]
    fn test_bare_minute_colon() {
        assert_eq!(
            ts("14:30"),
            span(7200, (2026, 5, 1, 14, 30, 0), (2026, 5, 1, 14, 31, 0))
        );
    }

    #[test]
    fn test_bare_second_colon() {
        assert_eq!(
            ts("14:30:45"),
            span(7200, (2026, 5, 1, 14, 30, 45), (2026, 5, 1, 14, 30, 46))
        );
    }

    #[test]
    fn test_bare_minute_terse() {
        // "30m" = minute=30, hour inherited from ref (15). Smallest specifier %M → 1 minute.
        assert_eq!(
            ts("30m"),
            span(7200, (2026, 5, 1, 15, 30, 0), (2026, 5, 1, 15, 31, 0))
        );
    }

    #[test]
    fn test_bare_unix_timestamp() {
        // @1736692200 = 2025-01-12 14:30:00 UTC. Always UTC, ignored ref offset.
        assert_eq!(
            ts("@1736692200"),
            span(0, (2025, 1, 12, 14, 30, 0), (2025, 1, 12, 14, 30, 1))
        );
    }

    // ===== Open-ended right =====

    #[test]
    fn test_today_open_right() {
        // today.. → [today 00:00, ref instant)
        assert_eq!(
            ts("today.."),
            span(7200, (2026, 5, 1, 0, 0, 0), (2026, 5, 1, 15, 0, 0))
        );
    }

    #[test]
    fn test_yesterday_open_right() {
        assert_eq!(
            ts("yesterday.."),
            span(7200, (2026, 4, 30, 0, 0, 0), (2026, 5, 1, 15, 0, 0))
        );
    }

    // ===== Two-operand =====

    #[test]
    fn test_yesterday_to_today() {
        // right=LEFT(today): stop is today's left edge = 2026-05-01 00:00
        assert_eq!(
            ts("yesterday..today"),
            span(7200, (2026, 4, 30, 0, 0, 0), (2026, 5, 1, 0, 0, 0))
        );
    }

    #[test]
    fn test_explicit_date_to_now() {
        assert_eq!(
            ts("2026-04-30..now"),
            span(7200, (2026, 4, 30, 0, 0, 0), (2026, 5, 1, 15, 0, 0))
        );
    }

    #[test]
    fn test_yesterday_to_now() {
        assert_eq!(
            ts("yesterday..now"),
            span(7200, (2026, 4, 30, 0, 0, 0), (2026, 5, 1, 15, 0, 0))
        );
    }

    #[test]
    fn test_explicit_date_to_calendar_period() {
        // right=LEFT(today): stop is today's left edge.
        assert_eq!(
            ts("2026-04-30..today"),
            span(7200, (2026, 4, 30, 0, 0, 0), (2026, 5, 1, 0, 0, 0))
        );
    }

    #[test]
    fn test_yesterday_to_tomorrow() {
        // right=LEFT(tomorrow) = 2026-05-02 00:00 → covers two days.
        assert_eq!(
            ts("yesterday..tomorrow"),
            span(7200, (2026, 4, 30, 0, 0, 0), (2026, 5, 2, 0, 0, 0))
        );
    }

    // ===== Calendar-period boundary tests (own references, separate TZ logic) =====

    fn tzset_refresh() {
        unsafe extern "C" {
            fn tzset();
        }
        unsafe {
            tzset();
        }
    }

    #[test]
    fn test_bare_month_no_dst() {
        // Bare "2024-01" under TZ=Europe/Paris with Local reference.
        // No DST transition in window (Jan 1..Feb 1 are both +01:00).
        unsafe {
            std::env::set_var("TZ", "Europe/Paris");
        }
        tzset_refresh();
        let reference = chrono::Local
            .with_ymd_and_hms(2024, 6, 15, 12, 0, 0)
            .unwrap();
        let (start, stop) = parse_timespan_with_reference("2024-01", &reference).unwrap();
        let off = FixedOffset::east_opt(3600).unwrap();
        assert_eq!(start, off.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap());
        assert_eq!(stop, off.with_ymd_and_hms(2024, 2, 1, 0, 0, 0).unwrap());
    }

    #[test]
    fn test_bare_month_spans_dst() {
        // Bare "2024-03" under TZ=Europe/Paris with Local reference.
        // March 31 2024 is spring-forward; stop must pick up +02:00.
        unsafe {
            std::env::set_var("TZ", "Europe/Paris");
        }
        tzset_refresh();
        let reference = chrono::Local
            .with_ymd_and_hms(2024, 6, 15, 12, 0, 0)
            .unwrap();
        let (start, stop) = parse_timespan_with_reference("2024-03", &reference).unwrap();
        let off_winter = FixedOffset::east_opt(3600).unwrap();
        let off_summer = FixedOffset::east_opt(2 * 3600).unwrap();
        assert_eq!(
            start,
            off_winter.with_ymd_and_hms(2024, 3, 1, 0, 0, 0).unwrap()
        );
        assert_eq!(
            stop,
            off_summer.with_ymd_and_hms(2024, 4, 1, 0, 0, 0).unwrap()
        );
    }

    #[test]
    fn test_bare_month_fixed_offset_ref() {
        // Bare "2024-03" with FixedOffset(+01:00) reference.
        // DST-blind path: stop preserves +01:00.
        let off = FixedOffset::east_opt(3600).unwrap();
        let reference = off.with_ymd_and_hms(2024, 6, 15, 12, 0, 0).unwrap();
        let (start, stop) = parse_timespan_with_reference("2024-03", &reference).unwrap();
        assert_eq!(start, off.with_ymd_and_hms(2024, 3, 1, 0, 0, 0).unwrap());
        assert_eq!(stop, off.with_ymd_and_hms(2024, 4, 1, 0, 0, 0).unwrap());
    }

    // ===== Validation / errors =====

    #[test]
    fn test_reverse_span_rejected() {
        let err = ts_err("today..yesterday");
        assert!(
            err.contains("not strictly before") || err.contains("Invalid"),
            "expected reverse-span error, got: {}",
            err
        );
    }

    #[test]
    fn test_zero_width_explicit_rejected() {
        let err = ts_err("2026-05-01..2026-05-01");
        assert!(
            err.contains("not strictly before") || err.contains("Invalid"),
            "expected zero-width error, got: {}",
            err
        );
    }

    #[test]
    fn test_leading_empty_rejected() {
        let err = ts_err("..today");
        assert!(
            err.contains("leading-empty") || err.contains("..X"),
            "expected leading-empty error, got: {}",
            err
        );
    }

    #[test]
    fn test_bare_now_rejected() {
        let err = ts_err("now");
        assert!(
            err.contains("not a period") || err.contains("bare"),
            "expected bare-now error, got: {}",
            err
        );
    }

    #[test]
    fn test_now_token_case_insensitive() {
        // Yesterday..NOW = same as yesterday..now
        assert_eq!(
            ts("Yesterday..NOW"),
            span(7200, (2026, 4, 30, 0, 0, 0), (2026, 5, 1, 15, 0, 0))
        );
    }

    // ===== unit_from_format derivation tests =====

    #[test]
    fn test_unit_from_format_derivations() {
        use super::{unit_from_format, PeriodUnit};
        assert_eq!(unit_from_format("%Y"), PeriodUnit::Year);
        assert_eq!(unit_from_format("%Y-%m"), PeriodUnit::Month);
        assert_eq!(unit_from_format("%Y-%m-%d"), PeriodUnit::Day);
        assert_eq!(unit_from_format("%Y-%m-%d %H:%M:%S"), PeriodUnit::Second);
        // Skips offset specifiers %:z and %#z
        assert_eq!(unit_from_format("%Y-%m-%dT%H:%M%:z"), PeriodUnit::Minute);
        assert_eq!(unit_from_format("%Y-%m-%dT%H:%M:%S%#z"), PeriodUnit::Second);
        assert_eq!(unit_from_format("%Hh"), PeriodUnit::Hour);
        // Runtime suffix formats
        assert_eq!(unit_from_format("%M:%S"), PeriodUnit::Second);
        assert_eq!(unit_from_format("%S"), PeriodUnit::Second);
        // Unix timestamp
        assert_eq!(unit_from_format("@%s"), PeriodUnit::Second);
    }

    // ===== trim_leftmost_specifier tests =====

    #[test]
    fn test_trim_leftmost_specifier_basic() {
        use super::trim_leftmost_specifier;
        // Drops "%Y-" → "%m-%d %H:%M:%S"
        assert_eq!(
            trim_leftmost_specifier("%Y-%m-%d %H:%M:%S"),
            Some("%m-%d %H:%M:%S")
        );
        // Drops "%m-" → "%d %H:%M:%S"
        assert_eq!(
            trim_leftmost_specifier("%m-%d %H:%M:%S"),
            Some("%d %H:%M:%S")
        );
        // Drops "%d " → "%H:%M:%S"
        assert_eq!(trim_leftmost_specifier("%d %H:%M:%S"), Some("%H:%M:%S"));
        // Drops "%H:" → "%M:%S"
        assert_eq!(trim_leftmost_specifier("%H:%M:%S"), Some("%M:%S"));
        // Drops "%M:" → "%S"
        assert_eq!(trim_leftmost_specifier("%M:%S"), Some("%S"));
        // No more separator after spec → empty suffix
        assert_eq!(trim_leftmost_specifier("%S"), Some(""));
    }

    #[test]
    fn test_trim_leftmost_specifier_terse_separators() {
        use super::trim_leftmost_specifier;
        // 'h' separator after %H
        assert_eq!(trim_leftmost_specifier("%Hh%M"), Some("%M"));
        // 'h' suffix-only (no further specifier)
        assert_eq!(trim_leftmost_specifier("%Hh"), Some(""));
        // '/' separator
        assert_eq!(trim_leftmost_specifier("%m/%d"), Some("%d"));
        // 'T' (ISO) separator between date and time
        assert_eq!(
            trim_leftmost_specifier("%Y-%m-%dT%H:%M:%S"),
            Some("%m-%dT%H:%M:%S")
        );
    }

    #[test]
    fn test_trim_leftmost_specifier_no_specifier() {
        use super::trim_leftmost_specifier;
        // No '%' at all
        assert_eq!(trim_leftmost_specifier(""), None);
        assert_eq!(trim_leftmost_specifier("plain text"), None);
    }

    // ===== generate_suffix_formats tests =====

    #[test]
    fn test_generate_suffix_formats_full_iso() {
        use super::generate_suffix_formats;
        assert_eq!(
            generate_suffix_formats("%Y-%m-%d %H:%M:%S"),
            vec![
                "%Y-%m-%d %H:%M:%S",
                "%m-%d %H:%M:%S",
                "%d %H:%M:%S",
                "%H:%M:%S",
                "%M:%S",
                "%S",
            ]
        );
    }

    #[test]
    fn test_generate_suffix_formats_terse() {
        use super::generate_suffix_formats;
        // %Hh%M → %M (only one suffix; trimming %H gives "h%M" → after
        // skipping 'h' separator we land on "%M")
        assert_eq!(generate_suffix_formats("%Hh%M"), vec!["%Hh%M", "%M"]);
    }

    #[test]
    fn test_generate_suffix_formats_single_specifier() {
        use super::generate_suffix_formats;
        // Single specifier: only the original entry.
        assert_eq!(generate_suffix_formats("%Y"), vec!["%Y"]);
    }

    #[test]
    fn test_generate_suffix_formats_iso_with_offset() {
        use super::generate_suffix_formats;
        assert_eq!(
            generate_suffix_formats("%Y-%m-%dT%H:%M%:z"),
            vec![
                "%Y-%m-%dT%H:%M%:z",
                "%m-%dT%H:%M%:z",
                "%dT%H:%M%:z",
                "%H:%M%:z",
                "%M%:z",
                "%:z",
            ]
        );
    }
}
