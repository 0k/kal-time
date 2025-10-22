use chrono::format::{ParseResult, Parsed};
use chrono::offset::{LocalResult, Offset};
use chrono::prelude::{Datelike, Timelike};
use chrono::{DateTime, FixedOffset, TimeZone};
use core::str;

// Wrapper functions to standardize the return type to i64
fn year<Tz: TimeZone>(dt: &DateTime<Tz>) -> i64 {
    dt.year() as i64
}
fn month<Tz: TimeZone>(dt: &DateTime<Tz>) -> i64 {
    dt.month() as i64
}
fn day<Tz: TimeZone>(dt: &DateTime<Tz>) -> i64 {
    dt.day() as i64
}
fn hour<Tz: TimeZone>(dt: &DateTime<Tz>) -> i64 {
    dt.hour() as i64
}
fn minute<Tz: TimeZone>(dt: &DateTime<Tz>) -> i64 {
    dt.minute() as i64
}
fn second<Tz: TimeZone>(dt: &DateTime<Tz>) -> i64 {
    dt.second() as i64
}
fn nanosecond<Tz: TimeZone>(dt: &DateTime<Tz>) -> i64 {
    dt.nanosecond() as i64
}

pub fn parse_partial<Tz: TimeZone>(
    s: &str,
    fmt: &str,
    reference: &DateTime<Tz>,
    complete_with_zeroes: bool,
) -> ParseResult<DateTime<FixedOffset>> {
    use chrono::format::Numeric::{Day, Hour, Minute, Month, Nanosecond, Second, Year};

    let mut parsed = Parsed::new();
    log::trace!("before: {:#?}", parsed);
    chrono::format::parse(&mut parsed, s, chrono::format::StrftimeItems::new(fmt))?;
    log::trace!("after: {:#?}", parsed);

    type Getter<T, Tz> = fn(&DateTime<Tz>) -> T;
    type Setter = fn(&mut Parsed, i64) -> ParseResult<()>;

    if parsed.timestamp.is_none() {
        let nums = vec![Nanosecond, Second, Minute, Hour, Day, Month, Year];
        let mut complete_with_zeroes = complete_with_zeroes;
        for num in nums.iter() {
            let (get, set, replace, min): (Getter<i64, Tz>, Setter, bool, i64) = match num {
                Year => (year, Parsed::set_year, parsed.year.is_none(), 1970),
                Month => (month, Parsed::set_month, parsed.month.is_none(), 1),
                Day => (day, Parsed::set_day, parsed.day.is_none(), 1),
                Hour => (
                    hour,
                    Parsed::set_hour,
                    parsed.hour_div_12.is_none() || parsed.hour_mod_12.is_none(),
                    0,
                ),
                Minute => (minute, Parsed::set_minute, parsed.minute.is_none(), 0),
                Second => (second, Parsed::set_second, parsed.second.is_none(), 0),
                Nanosecond => (
                    nanosecond,
                    Parsed::set_nanosecond,
                    parsed.nanosecond.is_none(),
                    0,
                ),
                _ => unreachable!(),
            };
            if replace {
                if complete_with_zeroes {
                    set(&mut parsed, min)?;
                } else {
                    set(&mut parsed, get(reference))?;
                }
            } else {
                complete_with_zeroes = false;
            }
        }
    }
    // If the offset is not specified, use the reference offset
    let offset = reference.offset().fix().local_minus_utc(); // i32 from offset
    let datetime = parsed.to_naive_datetime_with_offset(offset)?; // NaiveDateTime
    let offset = match FixedOffset::east_opt(offset) {
        Some(off) => off,
        None => {
            // XXXvlab: workaround the fact that ParseError constructor is private
            parsed.set_year_mod_100(101)?;
            unreachable!();
        }
    };

    match offset.from_local_datetime(&datetime) {
        LocalResult::Single(t) => Ok(t),
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, Utc};

    fn pp<Tz: TimeZone>(
        s: &str,
        fmt: &str,
        dt: &DateTime<Tz>,
        complete_with_zeroes: bool,
    ) -> String {
        format!("{:?}", parse_partial(s, fmt, dt, complete_with_zeroes))
    }

    #[test]
    fn test_simple_utc() {
        let dt = Utc.with_ymd_and_hms(2014, 7, 8, 9, 10, 11).unwrap(); // `2014-07-08T09:10:11Z`
        assert_eq!(pp("", "", &dt, false), "Ok(2014-07-08T09:10:11+00:00)");
        assert_eq!(
            pp("2015", "%Y", &dt, false),
            "Ok(2015-07-08T09:10:11+00:00)"
        );
        assert_eq!(
            pp("2015-02", "%Y-%m", &dt, false),
            "Ok(2015-02-08T09:10:11+00:00)"
        );
        assert_eq!(
            pp("2015-02-01", "%Y-%m-%d", &dt, false),
            "Ok(2015-02-01T09:10:11+00:00)"
        );
        assert_eq!(
            pp("2015-02-01 23", "%Y-%m-%d %H", &dt, false),
            "Ok(2015-02-01T23:10:11+00:00)"
        );
        assert_eq!(
            pp("2015-02-01 23:22", "%Y-%m-%d %H:%M", &dt, false),
            "Ok(2015-02-01T23:22:11+00:00)"
        );
        assert_eq!(
            pp("2015-02-01 23:22:12", "%Y-%m-%d %H:%M:%S", &dt, false),
            "Ok(2015-02-01T23:22:12+00:00)"
        );
    }

    #[test]
    fn test_err() {
        let dt = Local.with_ymd_and_hms(2014, 7, 8, 9, 10, 11).unwrap(); // `2014-07-08T09:10:11Z`
        assert_eq!(
            pp("9999999999", "%Y", &dt, false),
            "Err(ParseError(TooLong))"
        );
        assert_eq!(
            pp("2015 toto", "%Y %M", &dt, false),
            "Err(ParseError(Invalid))"
        );
    }

    #[test]
    fn test_fill_right() {
        // Use Utc to have a predictable timezone offset (+00:00)
        let dt = Utc.with_ymd_and_hms(2014, 7, 8, 9, 10, 11).unwrap();
        assert_eq!(pp("2015", "%Y", &dt, true), "Ok(2015-01-01T00:00:00+00:00)");
        assert_eq!(pp("12", "%H", &dt, true), "Ok(2014-07-08T12:00:00+00:00)");
    }
}
