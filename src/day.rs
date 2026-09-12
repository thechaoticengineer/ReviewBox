use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Days, LocalResult, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

pub const FALLBACK_TIMEZONE: &str = "Etc/UTC";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimezoneSource {
    Detected,
    Fallback,
    Explicit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UtcInterval {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl UtcInterval {
    pub fn contains(&self, instant: DateTime<Utc>) -> bool {
        self.start <= instant && instant < self.end
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaySelection {
    pub date: NaiveDate,
    pub timezone: Tz,
    pub timezone_name: String,
    pub timezone_source: TimezoneSource,
    pub interval: UtcInterval,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaySelectionError {
    InvalidTimezone(String),
    InvalidDate(String),
    DateOutOfRange(NaiveDate),
    AmbiguousMidnight { date: NaiveDate, timezone: String },
    NonexistentMidnight { date: NaiveDate, timezone: String },
}

impl fmt::Display for DaySelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTimezone(value) => write!(formatter, "invalid IANA timezone: {value}"),
            Self::InvalidDate(value) => {
                write!(formatter, "invalid ISO date (expected YYYY-MM-DD): {value}")
            }
            Self::DateOutOfRange(date) => write!(formatter, "cannot select the day after {date}"),
            Self::AmbiguousMidnight { date, timezone } => {
                write!(formatter, "midnight on {date} is ambiguous in {timezone}")
            }
            Self::NonexistentMidnight { date, timezone } => {
                write!(formatter, "midnight on {date} does not exist in {timezone}")
            }
        }
    }
}

pub trait LocalTimezoneDetector {
    fn detect(&self) -> Result<String, String>;
}

pub struct SystemTimezoneDetector;

impl LocalTimezoneDetector for SystemTimezoneDetector {
    fn detect(&self) -> Result<String, String> {
        iana_time_zone::get_timezone().map_err(|error| error.to_string())
    }
}

pub fn parse_timezone(value: &str) -> Result<Tz, DaySelectionError> {
    Tz::from_str(value).map_err(|_| DaySelectionError::InvalidTimezone(value.to_owned()))
}

pub fn parse_date(value: &str) -> Result<NaiveDate, DaySelectionError> {
    NaiveDate::parse_from_str(value, "%F")
        .map_err(|_| DaySelectionError::InvalidDate(value.to_owned()))
}

pub fn detected_timezone(detector: &impl LocalTimezoneDetector) -> (Tz, TimezoneSource) {
    detector
        .detect()
        .ok()
        .and_then(|value| parse_timezone(&value).ok())
        .map(|timezone| (timezone, TimezoneSource::Detected))
        .unwrap_or_else(|| {
            (
                parse_timezone(FALLBACK_TIMEZONE).expect("the UTC fallback is a valid timezone"),
                TimezoneSource::Fallback,
            )
        })
}

pub fn select_day(
    date: NaiveDate,
    timezone: Tz,
    timezone_source: TimezoneSource,
) -> Result<DaySelection, DaySelectionError> {
    let next_date = date
        .checked_add_days(Days::new(1))
        .ok_or(DaySelectionError::DateOutOfRange(date))?;
    let timezone_name = timezone.name().to_owned();
    let interval = UtcInterval {
        start: resolve_midnight(date, timezone, &timezone_name)?,
        end: resolve_midnight(next_date, timezone, &timezone_name)?,
    };
    debug_assert!(interval.start < interval.end);
    debug_assert!(interval.contains(interval.start));
    debug_assert!(!interval.contains(interval.end));
    Ok(DaySelection {
        date,
        timezone,
        timezone_name,
        timezone_source,
        interval,
    })
}

fn resolve_midnight(
    date: NaiveDate,
    timezone: Tz,
    timezone_name: &str,
) -> Result<DateTime<Utc>, DaySelectionError> {
    let midnight = date.and_hms_opt(0, 0, 0).expect("midnight is valid");
    match timezone.from_local_datetime(&midnight) {
        LocalResult::Single(value) => Ok(value.with_timezone(&Utc)),
        LocalResult::Ambiguous(_, _) => Err(DaySelectionError::AmbiguousMidnight {
            date,
            timezone: timezone_name.to_owned(),
        }),
        LocalResult::None => Err(DaySelectionError::NonexistentMidnight {
            date,
            timezone: timezone_name.to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone};

    use super::*;

    #[test]
    fn converts_a_normal_local_day_to_a_half_open_utc_interval() {
        let selection = select_day(
            parse_date("2024-01-15").unwrap(),
            parse_timezone("Europe/Warsaw").unwrap(),
            TimezoneSource::Explicit,
        )
        .unwrap();
        assert_eq!(
            selection.interval.start,
            Utc.with_ymd_and_hms(2024, 1, 14, 23, 0, 0).unwrap()
        );
        assert_eq!(
            selection.interval.end,
            Utc.with_ymd_and_hms(2024, 1, 15, 23, 0, 0).unwrap()
        );
        assert!(selection.interval.contains(selection.interval.start));
        assert!(!selection.interval.contains(selection.interval.end));
    }

    #[test]
    fn spring_forward_day_is_twenty_three_hours() {
        let selection = select_day(
            parse_date("2024-03-10").unwrap(),
            parse_timezone("America/New_York").unwrap(),
            TimezoneSource::Explicit,
        )
        .unwrap();
        assert_eq!(
            selection.interval.end - selection.interval.start,
            Duration::hours(23)
        );
    }

    #[test]
    fn fall_back_day_is_twenty_five_hours() {
        let selection = select_day(
            parse_date("2024-11-03").unwrap(),
            parse_timezone("America/New_York").unwrap(),
            TimezoneSource::Explicit,
        )
        .unwrap();
        assert_eq!(
            selection.interval.end - selection.interval.start,
            Duration::hours(25)
        );
    }
}
