use pico_args::Error as ArgsError;
use std::time::Duration;

#[derive(PartialEq, Clone, Copy)]
pub enum When {
    /// Automatically detect if color support is available on the terminal.
    Auto,
    /// Always display colors.
    Always,
    /// Never display colors.
    Never,
}

impl Default for When {
    fn default() -> Self {
        Self::Auto
    }
}

/// Enumerates all the formats that can be used to report test results.
#[derive(PartialEq, Clone, Copy)]
pub enum Format {
    /// Default formatter.
    Auto,
    /// Use the same format that libtest uses.
    LibTest,
    /// Use the format specified on http://testanything.org.
    Tap,
}

impl Default for Format {
    fn default() -> Self {
        Self::Auto
    }
}

#[derive(Default)]
pub struct Config {
    /// Filter controls which tests are to be executed.
    ///
    /// If set, only tests having name containing the filter (in at
    /// least one component of the name: either suite or test name)
    /// will be executed.
    pub filter: Option<String>,
    /// The time limit for execution of a single test.  If specified,
    /// this time limit is universal: all tests will inherit this time
    /// limit, even if some of them have a different value specified
    /// explicitly in code.
    pub timeout: Option<Duration>,
    /// Control when colored output is used.
    pub color: When,
    /// How many tests to execute in parallel.
    pub jobs: Option<usize>,
    /// Which format to use to report test results.
    pub format: Format,
}

pub enum ConfigParseError {
    HelpRequested,
    OptionError(String),
    UnknownArgs(Vec<String>),
    Unknown(String),
}

pub(crate) fn produce_help() -> String {
    format!(
        r#"Usage: {} [OPTIONS] [TESTNAME]

Options:
  -t, --timeout[=NSEC]     specify test execution timeout to be NSEC seconds
  -c, --color[=WHEN]       colorize the output, WHEN can be
                           'auto' (default), 'always' or 'never'
  -f, --format=[FMT]       output the test report in the specified format,
                           FMT can be 'auto' (default), 'libtest' or 'tap'
  -j, --jobs
  -h, --help               display this help and exit
"#,
        std::env::args().next().unwrap()
    )
}

fn parse_when(input: &str) -> Result<When, String> {
    match input {
        "auto" => Ok(When::Auto),
        "always" => Ok(When::Always),
        "never" => Ok(When::Never),
        _ => Err(format!("unsupported WHEN value: {}", input)),
    }
}

fn parse_format(input: &str) -> Result<Format, String> {
    match input {
        "auto" => Ok(Format::Auto),
        "libtest" => Ok(Format::LibTest),
        "tap" => Ok(Format::Tap),
        _ => Err(format!("unsupported FMT value: {}", input)),
    }
}

fn convert_error(err: ArgsError, what: &str) -> ConfigParseError {
    match err {
        ArgsError::OptionWithoutAValue(opt) => {
            ConfigParseError::OptionError(format!("Please specify a value for option {}", opt))
        }
        ArgsError::ArgumentParsingFailed { cause } => {
            ConfigParseError::OptionError(format!("failed to parse {}: {}", what, cause))
        }
        err => ConfigParseError::Unknown(err.to_string()),
    }
}

impl Config {
    pub fn from_args() -> Result<Self, ConfigParseError> {
        let mut args = pico_args::Arguments::from_env();
        if args.contains(["-h", "--help"]) {
            return Err(ConfigParseError::HelpRequested);
        }

        let timeout = args
            .opt_value_from_str(["-t", "--timeout"])
            .map_err(|err| convert_error(err, "timeout"))?
            .map(|secs: u64| Duration::from_secs(secs));

        let color = args
            .opt_value_from_fn(["-c", "--color"], parse_when)
            .map_err(|err| convert_error(err, "color"))?
            .unwrap_or(When::Auto);

        let format = args
            .opt_value_from_fn(["-f", "--format"], parse_format)
            .map_err(|err| convert_error(err, "format"))?
            .unwrap_or(Format::Auto);

        let jobs = args
            .opt_value_from_str(["-j", "--jobs"])
            .map_err(|err| convert_error(err, "jobs"))?;

        let positional_args = args.free().map_err(|err| match err {
            ArgsError::UnusedArgsLeft(args) => ConfigParseError::UnknownArgs(args),
            other => convert_error(other, "filter"),
        })?;

        let filter = match positional_args.len() {
            0 => Ok(None),
            1 => Ok(Some(positional_args[0].clone())),
            more => Err(ConfigParseError::OptionError(format!(
                "At most one TESTNAME can be specified, got {}: {}",
                more,
                positional_args.join(" ")
            ))),
        }?;

        Ok(Self {
            filter,
            timeout,
            color,
            jobs,
            format,
        })
    }
}
