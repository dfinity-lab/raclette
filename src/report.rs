use crate::{
    config::When,
    execution::{CompletedTask, Report, Status, Task},
};
use std::io::{self, Write};
use term::color::{Color, BRIGHT_GREEN, BRIGHT_RED, BRIGHT_YELLOW};

pub struct ColorWriter {
    out: Option<Box<term::StdoutTerminal>>,
    use_color: bool,
}

impl ColorWriter {
    pub fn new(color: When) -> Self {
        let out = term::stdout();
        let use_color = match color {
            When::Never => false,
            When::Always | When::Auto => match out {
                Some(ref t) => t.supports_color() && t.supports_reset(),
                None => false,
            },
        };
        Self { out, use_color }
    }

    pub fn newline(&mut self) {
        writeln!(self).unwrap();
    }

    pub fn with_color(&mut self, color: Color, f: impl FnOnce(&mut dyn Write)) {
        match self.out {
            Some(ref mut t) => {
                if self.use_color {
                    t.fg(color).unwrap();
                    f(t.get_mut());
                    t.reset().unwrap();
                } else {
                    f(t.get_mut());
                }
            }
            None => f(&mut io::stdout()),
        }
    }
}

impl Write for ColorWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.out {
            Some(ref mut t) => t.write(buf),
            None => io::stdout().write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.out {
            Some(ref mut t) => t.flush(),
            None => io::stdout().flush(),
        }
    }
}

/// This reporter displays results in http://testanything.org/ format.
///
/// This reporter can be enabled by `--format=tap` option.
pub struct TapReport {
    writer: ColorWriter,
    count: usize,
    total: usize,
}

impl TapReport {
    pub fn new(writer: ColorWriter) -> Self {
        Self {
            writer,
            total: 0,
            count: 0,
        }
    }
}

impl Report for TapReport {
    fn init(&mut self, plan: &[Task]) {
        writeln!(self.writer, "TAP version 13").unwrap();
        writeln!(self.writer, "1..{}", plan.len()).unwrap();
        self.total = plan.len();
    }

    fn report(&mut self, task: CompletedTask) {
        self.count += 1;
        let (ok, suffix) = match &task.status {
            Status::Success => (true, None),
            Status::Skipped(reason) => (true, Some(format!(" # SKIP {}", reason))),
            _ => (false, None),
        };

        let (msg, color) = if ok {
            ("ok", BRIGHT_GREEN)
        } else {
            ("not ok", BRIGHT_RED)
        };

        self.writer.with_color(color, |out| {
            write!(out, "{} ", msg).unwrap();
        });

        writeln!(
            self.writer,
            "{} - {}{}",
            self.count,
            task.name(),
            suffix.unwrap_or_default()
        )
        .unwrap();

        match task.status {
            Status::Success => {
                writeln!(self.writer, "# completed in {:?}", task.duration).unwrap();
            }
            Status::Failure(code) => {
                writeln!(
                    self.writer,
                    "# process returned {} after {:?}",
                    code, task.duration
                )
                .unwrap();
            }
            Status::Signaled(signame) => {
                writeln!(
                    self.writer,
                    "# process was killed with {} after {:?}",
                    signame, task.duration
                )
                .unwrap();
            }
            Status::Timeout => {
                writeln!(self.writer, "# timed out after {:?}", task.duration).unwrap();
            }
            Status::Skipped(_) => (),
        }

        if !ok {
            if !task.stdout.is_empty() {
                writeln!(self.writer, "# --- stdout ---").unwrap();
                for line in task.stdout_as_string().lines() {
                    writeln!(self.writer, "# {}", line).unwrap();
                }
            }
            if !task.stderr.is_empty() {
                writeln!(self.writer, "# --- stderr ---").unwrap();
                for line in task.stderr_as_string().lines() {
                    writeln!(self.writer, "# {}", line).unwrap();
                }
            }
        }
    }

    fn done(&mut self) {}
}

/// This reporter tries to imitate the format used by
/// https://github.com/rust-lang/libtest by default.
///
/// This reporter can be explicitly enabled by `--format=libtest`
/// option, but it's also the default one.
pub struct LibTestReport {
    writer: ColorWriter,
    passed: usize,
    failed: Vec<CompletedTask>,
    ignored: usize,
}

impl LibTestReport {
    pub fn new(writer: ColorWriter) -> Self {
        Self {
            writer,
            passed: 0,
            failed: vec![],
            ignored: 0,
        }
    }
}

impl Report for LibTestReport {
    fn init(&mut self, plan: &[Task]) {
        let n = plan.len();
        writeln!(
            self.writer,
            "running {} test{}",
            n,
            if n == 1 { "" } else { "s" }
        )
        .unwrap();
    }
    fn report(&mut self, task: CompletedTask) {
        enum S {
            Ok,
            Ignored,
            Failed,
        };

        let (ok, status, color) = match task.status {
            Status::Success => (S::Ok, "ok", BRIGHT_GREEN),
            Status::Skipped(_) => (S::Ignored, "ignored", BRIGHT_YELLOW),
            _ => (S::Failed, "FAILED", BRIGHT_RED),
        };

        write!(self.writer, "test {} ... ", task.name()).unwrap();
        self.writer.with_color(color, |out| {
            writeln!(out, "{}", status).unwrap();
        });

        match ok {
            S::Ok => {
                self.passed += 1;
            }
            S::Ignored => {
                self.ignored += 1;
            }
            S::Failed => {
                self.failed.push(task);
            }
        }
    }
    fn done(&mut self) {
        if !self.failed.is_empty() {
            writeln!(self.writer, "\nfailures:\n").unwrap();

            for task in self.failed.iter() {
                if !task.stdout.is_empty() {
                    let out = task.stdout_as_string();
                    writeln!(
                        self.writer,
                        "---- test {} stdout ----\n{}",
                        task.name(),
                        out
                    )
                    .unwrap();
                    if !out.ends_with('\n') {
                        self.writer.newline();
                    }
                }
                if !task.stderr.is_empty() {
                    let err = task.stderr_as_string();
                    writeln!(
                        self.writer,
                        "---- test {} stderr ----\n{}",
                        task.name(),
                        err,
                    )
                    .unwrap();
                    if !err.ends_with('\n') {
                        self.writer.newline();
                    }
                }
            }

            writeln!(self.writer, "\nfailures:").unwrap();

            for task in self.failed.iter() {
                writeln!(self.writer, "    {}", task.name()).unwrap();
            }
        }

        self.writer.newline();
        write!(self.writer, "test result: ").unwrap();
        let (status, color) = if !self.failed.is_empty() {
            ("FAILED", BRIGHT_RED)
        } else {
            ("ok", BRIGHT_GREEN)
        };

        self.writer
            .with_color(color, |out| write!(out, "{}", status).unwrap());

        writeln!(
            self.writer,
            ". {} passed; {} failed; {} ignored;\n",
            self.passed,
            self.failed.len(),
            self.ignored
        )
        .unwrap();
    }
}
