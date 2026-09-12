use std::ffi::OsString;
use std::fmt;

use chrono::{DateTime, Utc};

use crate::day::{
    DaySelection, DaySelectionError, LocalTimezoneDetector, SystemTimezoneDetector, TimezoneSource,
    detected_timezone, parse_date, parse_timezone, select_day,
};

pub const USAGE: &str = "ReviewBox daily GitHub inbox\n\nUsage:\n  reviewbox [--date YYYY-MM-DD] [--timezone IANA_NAME]\n  reviewbox --demo\n  reviewbox --demo-smoke\n  reviewbox --help\n\nOptions:\n  --date DATE        Select an ISO calendar date for the live inbox\n  --timezone ZONE     Select an IANA timezone for the live inbox\n  --demo              Run the fictional, offline terminal demo\n  --demo-smoke        Verify the demo noninteractively with an in-memory terminal\n  -h, --help          Show this help\n\nWithout a mode, ReviewBox starts the live inbox for today in the detected local timezone. If local timezone detection is unavailable, it uses Etc/UTC.\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Live(DaySelection),
    Demo,
    DemoSmoke,
    Help,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    UnexpectedArgument(OsString),
    MissingValue(&'static str),
    DuplicateOption(&'static str),
    IncompatibleOption {
        mode: &'static str,
        option: &'static str,
    },
    Day(DaySelectionError),
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedArgument(argument) => write!(
                formatter,
                "unexpected argument: {}",
                argument.to_string_lossy()
            ),
            Self::MissingValue(option) => write!(formatter, "missing value for {option}"),
            Self::DuplicateOption(option) => {
                write!(formatter, "{option} may only be provided once")
            }
            Self::IncompatibleOption { mode, option } => {
                write!(formatter, "{option} cannot be used with {mode}")
            }
            Self::Day(error) => error.fmt(formatter),
        }
    }
}

pub fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Command, ParseError> {
    parse_with(arguments, &SystemTimezoneDetector, Utc::now())
}

pub fn parse_with(
    arguments: impl IntoIterator<Item = OsString>,
    detector: &impl LocalTimezoneDetector,
    now: DateTime<Utc>,
) -> Result<Command, ParseError> {
    let mut arguments = arguments.into_iter();
    let mut mode = None;
    let mut date = None;
    let mut timezone = None;

    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--demo") => set_mode(&mut mode, "--demo", Command::Demo)?,
            Some("--demo-smoke") => set_mode(&mut mode, "--demo-smoke", Command::DemoSmoke)?,
            Some("--help") | Some("-h") => set_mode(&mut mode, "--help", Command::Help)?,
            Some("--date") => {
                if date.is_some() {
                    return Err(ParseError::DuplicateOption("--date"));
                }
                let value = arguments.next().ok_or(ParseError::MissingValue("--date"))?;
                let value = value
                    .into_string()
                    .map_err(ParseError::UnexpectedArgument)?;
                date = Some(parse_date(&value).map_err(ParseError::Day)?);
            }
            Some("--timezone") => {
                if timezone.is_some() {
                    return Err(ParseError::DuplicateOption("--timezone"));
                }
                let value = arguments
                    .next()
                    .ok_or(ParseError::MissingValue("--timezone"))?;
                let value = value
                    .into_string()
                    .map_err(ParseError::UnexpectedArgument)?;
                timezone = Some(parse_timezone(&value).map_err(ParseError::Day)?);
            }
            _ => return Err(ParseError::UnexpectedArgument(argument)),
        }
    }

    match mode {
        Some((mode_name, command @ (Command::Demo | Command::DemoSmoke))) => {
            if date.is_some() {
                return Err(ParseError::IncompatibleOption {
                    mode: mode_name,
                    option: "--date",
                });
            }
            if timezone.is_some() {
                return Err(ParseError::IncompatibleOption {
                    mode: mode_name,
                    option: "--timezone",
                });
            }
            Ok(command)
        }
        Some((_, Command::Help)) => {
            if date.is_some() {
                return Err(ParseError::IncompatibleOption {
                    mode: "--help",
                    option: "--date",
                });
            }
            if timezone.is_some() {
                return Err(ParseError::IncompatibleOption {
                    mode: "--help",
                    option: "--timezone",
                });
            }
            Ok(Command::Help)
        }
        Some(_) => unreachable!("only user-selectable modes are stored"),
        None => {
            let (detected_timezone, detected_source) = detected_timezone(detector);
            let (timezone, timezone_source) = match timezone {
                Some(timezone) => (timezone, TimezoneSource::Explicit),
                None => (detected_timezone, detected_source),
            };
            let date = date.unwrap_or_else(|| now.with_timezone(&timezone).date_naive());
            select_day(date, timezone, timezone_source)
                .map(Command::Live)
                .map_err(ParseError::Day)
        }
    }
}

fn set_mode(
    slot: &mut Option<(&'static str, Command)>,
    name: &'static str,
    command: Command,
) -> Result<(), ParseError> {
    if slot.is_some() {
        return Err(ParseError::UnexpectedArgument(OsString::from(name)));
    }
    *slot = Some((name, command));
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    struct Detector(Result<String, String>);
    impl LocalTimezoneDetector for Detector {
        fn detect(&self) -> Result<String, String> {
            self.0.clone()
        }
    }
    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2024, 1, 15, 23, 30, 0).unwrap()
    }
    fn parse_at(arguments: &[&str], detector: Detector) -> Result<Command, ParseError> {
        parse_with(arguments.iter().map(OsString::from), &detector, now())
    }

    #[test]
    fn default_mode_is_live_today_in_detected_timezone() {
        let command = parse_at(&[], Detector(Ok("Europe/Warsaw".to_owned()))).unwrap();
        let Command::Live(selection) = command else {
            panic!("expected live mode")
        };
        assert_eq!(selection.date.to_string(), "2024-01-16");
        assert_eq!(selection.timezone_name, "Europe/Warsaw");
        assert_eq!(selection.timezone_source, TimezoneSource::Detected);
    }

    #[test]
    fn fallback_timezone_is_deterministic_when_detection_fails() {
        let command = parse_at(&[], Detector(Err("unavailable".to_owned()))).unwrap();
        let Command::Live(selection) = command else {
            panic!("expected live mode")
        };
        assert_eq!(selection.date.to_string(), "2024-01-15");
        assert_eq!(selection.timezone_name, "Etc/UTC");
        assert_eq!(selection.timezone_source, TimezoneSource::Fallback);
    }

    #[test]
    fn explicit_date_and_timezone_override_independently() {
        let command = parse_at(
            &["--date", "2024-03-10"],
            Detector(Ok("America/New_York".to_owned())),
        )
        .unwrap();
        let Command::Live(selection) = command else {
            panic!("expected live mode")
        };
        assert_eq!(selection.date.to_string(), "2024-03-10");
        assert_eq!(selection.timezone_name, "America/New_York");
        assert_eq!(selection.timezone_source, TimezoneSource::Detected);

        let command = parse_at(
            &["--timezone", "America/New_York"],
            Detector(Ok("Europe/Warsaw".to_owned())),
        )
        .unwrap();
        let Command::Live(selection) = command else {
            panic!("expected live mode")
        };
        assert_eq!(selection.date.to_string(), "2024-01-15");
        assert_eq!(selection.timezone_name, "America/New_York");
        assert_eq!(selection.timezone_source, TimezoneSource::Explicit);
    }

    #[test]
    fn demo_modes_remain_explicit_and_reject_live_options() {
        assert_eq!(
            parse_at(&["--demo"], Detector(Err("unused".to_owned()))),
            Ok(Command::Demo)
        );
        assert_eq!(
            parse_at(&["--demo-smoke"], Detector(Err("unused".to_owned()))),
            Ok(Command::DemoSmoke)
        );
        assert!(matches!(
            parse_at(
                &["--demo", "--date", "2024-01-15"],
                Detector(Ok("Etc/UTC".to_owned()))
            ),
            Err(ParseError::IncompatibleOption { .. })
        ));
        assert!(matches!(
            parse_at(
                &["--demo", "--demo-smoke"],
                Detector(Ok("Etc/UTC".to_owned()))
            ),
            Err(ParseError::UnexpectedArgument(_))
        ));
    }

    #[test]
    fn rejects_invalid_and_incomplete_arguments() {
        assert!(matches!(
            parse_at(
                &["--date", "2024-02-30"],
                Detector(Ok("Etc/UTC".to_owned()))
            ),
            Err(ParseError::Day(DaySelectionError::InvalidDate(_)))
        ));
        assert!(matches!(
            parse_at(
                &["--timezone", "Mars/Olympus"],
                Detector(Ok("Etc/UTC".to_owned()))
            ),
            Err(ParseError::Day(DaySelectionError::InvalidTimezone(_)))
        ));
        assert_eq!(
            parse_at(&["--date"], Detector(Ok("Etc/UTC".to_owned()))),
            Err(ParseError::MissingValue("--date"))
        );
    }
}
