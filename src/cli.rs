use std::ffi::OsString;
use std::fmt;

pub const USAGE: &str = "ReviewBox terminal foundation\n\nUsage:\n  reviewbox --demo\n  reviewbox --help\n\nOptions:\n  --demo  Run the fictional, offline terminal demo\n  -h, --help  Show this help\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Demo,
    Help,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    MissingMode,
    UnexpectedArgument(OsString),
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingMode => write!(formatter, "choose a launch mode (try --demo)"),
            Self::UnexpectedArgument(argument) => {
                write!(
                    formatter,
                    "unexpected argument: {}",
                    argument.to_string_lossy()
                )
            }
        }
    }
}

pub fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Command, ParseError> {
    let mut arguments = arguments.into_iter();
    let command = match arguments.next() {
        Some(argument) if argument == "--demo" => Command::Demo,
        Some(argument) if argument == "--help" || argument == "-h" => Command::Help,
        Some(argument) => return Err(ParseError::UnexpectedArgument(argument)),
        None => return Err(ParseError::MissingMode),
    };

    if let Some(argument) = arguments.next() {
        return Err(ParseError::UnexpectedArgument(argument));
    }

    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_mode_is_explicit() {
        assert_eq!(parse([OsString::from("--demo")]), Ok(Command::Demo));
        assert_eq!(parse([]), Err(ParseError::MissingMode));
    }

    #[test]
    fn rejects_extra_arguments() {
        assert_eq!(
            parse([OsString::from("--demo"), OsString::from("extra")]),
            Err(ParseError::UnexpectedArgument(OsString::from("extra")))
        );
    }
}
