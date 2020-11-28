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

impl When {
    fn merge(l: When, r: When) -> When {
        match l {
            When::Auto => r,
            _ => l,
        }
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

impl Format {
    fn merge(l: Format, r: Format) -> Format {
        match l {
            Format::Auto => r,
            _ => l,
        }
    }
}

#[derive(Default)]
pub struct Config {
    pub(crate) filter: Option<String>,
    pub(crate) skip_filters: Vec<String>,
    pub(crate) timeout: Option<Duration>,
    pub(crate) color: When,
    pub(crate) jobs: Option<usize>,
    pub(crate) format: Format,
    pub(crate) nocapture: bool,
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
      --skip FILTER        Skip tests whose names contain FILTER
                           (this flag can be used multiple times)

      --nocapture          Print output of each task directly as soon
                           as it arrives

  -t, --timeout NSEC       Specify test execution timeout to be NSEC seconds

  -c, --color WHEN         Colorize the output, WHEN can be
                           'auto' (default), 'always' or 'never'

  -f, --format FMT         Output the test report in the specified format,
                           FMT can be 'auto' (default), 'libtest' or 'tap'

  -j, --jobs NJOBS         Run at most NJOBS tests in parallel

  -h, --help               Display this help and exit
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
    /// Parses configuration from command line flags.
    pub fn from_args() -> Result<Self, ConfigParseError> {
        let mut args = pico_args::Arguments::from_env();
        if args.contains(["-h", "--help"]) {
            return Err(ConfigParseError::HelpRequested);
        }

        let timeout = args
            .opt_value_from_str(["-t", "--timeout"])
            .map_err(|err| convert_error(err, "timeout"))?
            .map(Duration::from_secs);

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

        let skip_filters = args
            .values_from_str("--skip")
            .map_err(|err| convert_error(err, "skip"))?;

        let nocapture = args.contains("--nocapture");

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
            skip_filters,
            timeout,
            color,
            jobs,
            format,
            nocapture,
        })
    }

    /// Merges two configurations by copying values of all unset
    /// fields in `self` from `other`.
    pub fn merge(mut self, mut other: Config) -> Config {
        self.skip_filters.append(&mut other.skip_filters);

        Config {
            filter: self.filter.or(other.filter),
            skip_filters: self.skip_filters,
            timeout: self.timeout.or(other.timeout),
            color: When::merge(self.color, other.color),
            jobs: self.jobs.or(other.jobs),
            format: Format::merge(self.format, other.format),
            nocapture: self.nocapture || other.nocapture,
        }
    }

    /// Sets the filter controlling which tests are to be executed.
    ///
    /// If set, only tests having name containing the filter (in at
    /// least one component of the name: either suite or test name)
    /// will be executed.
    pub fn filter(mut self, filter: String) -> Self {
        self.filter = Some(filter);
        self
    }

    /// Sets the filters controlling which tests DO NOT run.
    ///
    /// If set, all the tests having name containing on of the filters
    /// (in at least one component of the name) will be skipped.
    pub fn skip_filters(mut self, filters: Vec<String>) -> Self {
        self.skip_filters = filters;
        self
    }

    /// Sets the time limit for execution of a single test.  If
    /// specified, this time limit is universal: all tests will
    /// inherit this time limit, even if some of them have a different
    /// value specified explicitly in code.
    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = Some(d);
        self
    }

    /// Controls if colored output is used.
    pub fn color(mut self, when: When) -> Self {
        self.color = when;
        self
    }

    /// Sets the upper limit on the number tests that can be executed
    /// in parallel.
    pub fn jobs(mut self, num_jobs: usize) -> Self {
        self.jobs = Some(num_jobs);
        self
    }

    /// Sets the format that should be used for reporting test
    /// results.
    pub fn format(mut self, fmt: Format) -> Self {
        self.format = fmt;
        self
    }
}
