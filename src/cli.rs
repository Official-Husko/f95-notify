//! Command-line parsing, done by hand to avoid a dependency.

pub const USAGE: &str = concat!(
    "F95 Notify v",
    env!("CARGO_PKG_VERSION"),
    "\nDesktop notifications for F95Zone alerts.\n\n",
    "Usage: f95-notify [OPTION]\n\n",
    "Options:\n",
    "  --login     Open the login window and save the session to the config\n",
    "  --source    Open the source repository in the default browser\n",
    "  --version   Print the version\n",
    "  --help      Print this help\n\n",
    "Environment:\n",
    "  F95_NOTIFY_CONFIG   Path to config.json\n",
    "  F95_NOTIFY_DEBUG    Set to 1 for verbose logging\n",
);

#[derive(Debug, PartialEq, Eq)]
pub enum Mode {
    Run,
    Login,
    Source,
    Version,
    Help,
}

pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Mode, String> {
    let mode = match args.next().as_deref() {
        None => Mode::Run,
        Some("--login") => Mode::Login,
        Some("--source") => Mode::Source,
        Some("-V" | "--version") => Mode::Version,
        Some("-h" | "--help") => Mode::Help,
        Some(other) => return Err(format!("unknown option: {other}")),
    };
    match args.next() {
        None => Ok(mode),
        Some(extra) => Err(format!("unexpected argument: {extra}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_strs(args: &[&str]) -> Result<Mode, String> {
        parse(args.iter().map(ToString::to_string))
    }

    #[test]
    fn parses_modes() {
        assert_eq!(parse_strs(&[]), Ok(Mode::Run));
        assert_eq!(parse_strs(&["--login"]), Ok(Mode::Login));
        assert_eq!(parse_strs(&["--source"]), Ok(Mode::Source));
        assert_eq!(parse_strs(&["--version"]), Ok(Mode::Version));
        assert_eq!(parse_strs(&["-V"]), Ok(Mode::Version));
        assert_eq!(parse_strs(&["--help"]), Ok(Mode::Help));
    }

    #[test]
    fn rejects_unknown_and_extra() {
        assert!(parse_strs(&["--nope"]).is_err());
        assert!(parse_strs(&["--version", "--login"]).is_err());
    }
}
