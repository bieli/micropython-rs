// ============================================================================
// CALENDAR: MicroPython's time.gmtime / time.mktime, natively
// ============================================================================
//
// Seconds since 2000-01-01 00:00 UTC (MicroPython's ESP32 epoch) to and from
// (year, month, mday, hour, minute, second, weekday, yearday), as MicroPython
// gives them: month and mday from 1, weekday 0 for Monday, yearday from 1.
// The day count uses Howard Hinnant's days-from-civil algorithm (exact for
// any proleptic Gregorian date), so a date takes a few divisions instead of a
// loop over years (the Python version took 28 ms a call on an ESP32-S3).

/// A broken-down time (`time.gmtime()`'s tuple).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tm {
    pub year: i64,
    pub month: i64,
    pub mday: i64,
    pub hour: i64,
    pub minute: i64,
    pub second: i64,
    /// 0 is Monday
    pub weekday: i64,
    /// 1 is January 1st
    pub yearday: i64,
}

/// Days from 1970-01-01 to 2000-01-01.
const DAYS_TO_2000: i64 = 10_957;

/// Days since 1970-01-01 of a date (any month: 13 is next January).
fn days_from_civil(year: i64, month: i64, mday: i64) -> i64 {
    // Months out of 1-12 move the year, as MicroPython's mktime does
    let year = year + (month - 1).div_euclid(12);
    let month = (month - 1).rem_euclid(12) + 1;
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400; // [0, 399]
    let mp = (month + 9) % 12; // March = 0
    let doy = (153 * mp + 2) / 5 + mday - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// The date of a day since 1970-01-01: (year, month, mday).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let mday = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, mday)
}

/// `time.gmtime(secs)`: seconds since 2000-01-01 UTC to a broken-down time.
pub fn gmtime(secs: i64) -> Tm {
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    let days_1970 = days + DAYS_TO_2000;
    let (year, month, mday) = civil_from_days(days_1970);
    Tm {
        year,
        month,
        mday,
        hour: rest / 3600,
        minute: rest / 60 % 60,
        second: rest % 60,
        // 2000-01-01 was a Saturday (5)
        weekday: (days + 5).rem_euclid(7),
        yearday: days_1970 - days_from_civil(year, 1, 1) + 1,
    }
}

/// `time.mktime((year, month, mday, hour, minute, second, ...))`: seconds
/// since 2000-01-01 UTC (weekday and yearday aren't needed; a month out of
/// 1-12 moves the year; hours, minutes and seconds may overflow too).
pub fn mktime(year: i64, month: i64, mday: i64, hour: i64, minute: i64, second: i64) -> i64 {
    let days = days_from_civil(year, month, mday) - DAYS_TO_2000;
    ((days * 24 + hour) * 60 + minute) * 60 + second
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tuple(t: Tm) -> (i64, i64, i64, i64, i64, i64, i64, i64) {
        (
            t.year, t.month, t.mday, t.hour, t.minute, t.second, t.weekday, t.yearday,
        )
    }

    #[test]
    fn known_dates() {
        assert_eq!(tuple(gmtime(0)), (2000, 1, 1, 0, 0, 0, 5, 1));
        assert_eq!(
            tuple(gmtime(848_964_606)),
            (2026, 11, 25, 23, 30, 6, 2, 329)
        );
        assert_eq!(tuple(gmtime(-1)), (1999, 12, 31, 23, 59, 59, 4, 365));
        // Leap days: 2024 is a leap year, 2100 isn't, 2000 is
        assert_eq!(
            tuple(gmtime(mktime(2024, 2, 29, 12, 0, 0))),
            (2024, 2, 29, 12, 0, 0, 3, 60)
        );
        // 2100 isn't a leap year: the day after Feb 28 is Mar 1
        let g = gmtime(mktime(2100, 2, 28, 12, 0, 0) + 86_400);
        assert_eq!((g.year, g.month, g.mday), (2100, 3, 1));
        assert_eq!(gmtime(mktime(2000, 12, 31, 0, 0, 0)).yearday, 366);
        assert_eq!(gmtime(mktime(2100, 12, 31, 0, 0, 0)).yearday, 365);
    }

    #[test]
    fn mktime_normalizes_months() {
        assert_eq!(mktime(2026, 13, 1, 0, 0, 0), mktime(2027, 1, 1, 0, 0, 0));
        assert_eq!(mktime(2026, 0, 1, 0, 0, 0), mktime(2025, 12, 1, 0, 0, 0));
        assert_eq!(mktime(2026, 1, 1, 25, 0, 0), mktime(2026, 1, 2, 1, 0, 0));
    }

    #[test]
    fn round_trips() {
        // Every 7654321 s from 1987 to 2095, and every day around a few
        // century and leap boundaries
        let mut t = -400_000_000i64;
        while t < 3_000_000_000 {
            let g = gmtime(t);
            assert_eq!(
                mktime(g.year, g.month, g.mday, g.hour, g.minute, g.second),
                t,
                "{}",
                t
            );
            t += 7_654_321;
        }
        let mut previous = gmtime(mktime(2099, 12, 1, 0, 0, 0));
        for d in 1..800 {
            let g = gmtime(mktime(2099, 12, 1, 0, 0, 0) + d * 86_400);
            assert_eq!(g.weekday, (previous.weekday + 1) % 7);
            assert!(
                g.yearday == previous.yearday + 1
                    || (g.yearday == 1 && g.year == previous.year + 1)
            );
            previous = g;
        }
    }
}
