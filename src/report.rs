use crate::{
    config::When,
    execution::{CompletedTask, Report, Status, Task},
};
use std::borrow::Cow;
use std::io::{self, Write};
use std::time::Duration;
use term::color::{Color, BRIGHT_GREEN, BRIGHT_RED, BRIGHT_YELLOW};

#[derive(Default)]
pub struct TestStats {
    pub total: usize,
    pub ok: usize,
    pub failed: usize,
    pub ignored: usize,
}

impl TestStats {
    pub fn update(&mut self, task: &CompletedTask) {
        self.total += 1;
        match task.status {
            Status::Success => {
                self.ok += 1;
            }
            Status::Failure(_) | Status::Signaled(_) | Status::Timeout => {
                self.failed += 1;
            }
            Status::Skipped(_) => {
                self.ignored += 1;
            }
        }
    }

    pub fn ok(&self) -> bool {
        self.failed == 0
    }
}

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

    fn start(&mut self, _name: String) {}

    fn report(&mut self, task: &CompletedTask) {
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

    fn start(&mut self, _name: String) {}

    fn report(&mut self, task: &CompletedTask) {
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
                self.failed.push(task.clone());
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

pub struct JsonReport {
    writer: ColorWriter,
    stats: TestStats,
}

impl JsonReport {
    pub fn new(writer: ColorWriter) -> Self {
        Self {
            writer,
            stats: Default::default(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn write_event(
        &mut self,
        ty: &str,
        name: &str,
        evt: &str,
        exec_time: Duration,
        stdout: Cow<'_, str>,
        stderr: Cow<'_, str>,
        extra: Option<&str>,
    ) {
        // A doc test's name includes a filename which must be escaped for correct json.
        write!(
            self.writer,
            r#"{{ "type": "{}", "name": "{}", "event": "{}", "exec_time": "{:.4}s""#,
            ty,
            EscapedString(name),
            evt,
            exec_time.as_secs_f64(),
        )
        .unwrap();

        if !stdout.is_empty() {
            write!(self.writer, r#", "stdout": "{}""#, EscapedString(stdout)).unwrap();
        }
        if !stderr.is_empty() {
            write!(self.writer, r#", "stderr": "{}""#, EscapedString(stderr)).unwrap();
        }
        if let Some(extra) = extra {
            write!(self.writer, r#", {}"#, extra).unwrap();
        }
        writeln!(self.writer, "}}").unwrap();
    }
}

impl Report for JsonReport {
    fn init(&mut self, plan: &[Task]) {
        write!(
            self.writer,
            r#"{{ "type": "suite", "event": "started", "test_count": {} }}"#,
            plan.len()
        )
        .unwrap();
        writeln!(self.writer).unwrap();
    }

    fn start(&mut self, name: String) {
        write!(
            self.writer,
            r#"{{ "type": "test", "event": "started", "name": "{}" }}"#,
            EscapedString(name),
        )
        .unwrap();
        writeln!(self.writer).unwrap();
    }

    fn report(&mut self, task: &CompletedTask) {
        self.stats.update(&task);
        match task.status {
            Status::Success => {
                self.write_event(
                    "test",
                    task.name().as_str(),
                    "ok",
                    task.duration,
                    task.stdout_as_string(),
                    task.stderr_as_string(),
                    None,
                );
            }
            Status::Failure(ref code) => {
                self.write_event(
                    "test",
                    task.name().as_str(),
                    "failed",
                    task.duration,
                    task.stdout_as_string(),
                    task.stderr_as_string(),
                    Some(&format!(
                        r#""reason": "test process exited with code {}""#,
                        code
                    )),
                );
            }
            Status::Signaled(ref signame) => {
                self.write_event(
                    "test",
                    task.name().as_str(),
                    "failed",
                    task.duration,
                    task.stdout_as_string(),
                    task.stderr_as_string(),
                    Some(&format!(r#""reason": "killed by signal {}""#, signame)),
                );
            }
            Status::Timeout => {
                self.write_event(
                    "test",
                    task.name().as_str(),
                    "failed",
                    task.duration,
                    task.stdout_as_string(),
                    task.stderr_as_string(),
                    Some(r#""reason": "time limit exceeded""#),
                );
            }
            Status::Skipped(ref reason) => {
                self.write_event(
                    "test",
                    task.name().as_str(),
                    "ignored",
                    task.duration,
                    task.stdout_as_string(),
                    task.stderr_as_string(),
                    Some(&format!(r#""reason": "{}""#, EscapedString(reason),)),
                );
            }
        }
    }

    fn done(&mut self) {
        write!(
            self.writer,
            r#"{{ "type": "suite", "event": "{}", "passed": {}, "failed": {}, "ignored": {} }}"#,
            if self.stats.ok() { "ok" } else { "failed" },
            self.stats.ok,
            self.stats.failed,
            self.stats.ignored,
        )
        .unwrap();
        writeln!(self.writer).unwrap();
    }
}

struct EscapedString<S: AsRef<str>>(S);

impl<S: AsRef<str>> std::fmt::Display for EscapedString<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        let mut start = 0;

        for (i, byte) in self.0.as_ref().bytes().enumerate() {
            let escaped = match byte {
                b'"' => "\\\"",
                b'\\' => "\\\\",
                b'\x00' => "\\u0000",
                b'\x01' => "\\u0001",
                b'\x02' => "\\u0002",
                b'\x03' => "\\u0003",
                b'\x04' => "\\u0004",
                b'\x05' => "\\u0005",
                b'\x06' => "\\u0006",
                b'\x07' => "\\u0007",
                b'\x08' => "\\b",
                b'\t' => "\\t",
                b'\n' => "\\n",
                b'\x0b' => "\\u000b",
                b'\x0c' => "\\f",
                b'\r' => "\\r",
                b'\x0e' => "\\u000e",
                b'\x0f' => "\\u000f",
                b'\x10' => "\\u0010",
                b'\x11' => "\\u0011",
                b'\x12' => "\\u0012",
                b'\x13' => "\\u0013",
                b'\x14' => "\\u0014",
                b'\x15' => "\\u0015",
                b'\x16' => "\\u0016",
                b'\x17' => "\\u0017",
                b'\x18' => "\\u0018",
                b'\x19' => "\\u0019",
                b'\x1a' => "\\u001a",
                b'\x1b' => "\\u001b",
                b'\x1c' => "\\u001c",
                b'\x1d' => "\\u001d",
                b'\x1e' => "\\u001e",
                b'\x1f' => "\\u001f",
                b'\x7f' => "\\u007f",
                _ => {
                    continue;
                }
            };

            if start < i {
                f.write_str(&self.0.as_ref()[start..i])?;
            }

            f.write_str(escaped)?;

            start = i + 1;
        }

        if start != self.0.as_ref().len() {
            f.write_str(&self.0.as_ref()[start..])?;
        }

        Ok(())
    }
}
