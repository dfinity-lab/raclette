use crate::{
    config::When,
    execution::{CompletedTask, Report, Status, Task},
};
use term::color::{Color, BRIGHT_GREEN, BRIGHT_RED};

pub struct TapReport {
    out: Box<term::StdoutTerminal>,
    use_color: bool,
    count: usize,
    total: usize,
}

impl TapReport {
    pub fn new(color: When) -> Self {
        let out = term::stdout().unwrap();
        let use_color = match color {
            When::Never => false,
            When::Always | When::Auto => out.supports_color() && out.supports_reset(),
        };
        Self {
            out,
            use_color,
            total: 0,
            count: 0,
        }
    }

    fn fg(&mut self, color: Color) {
        if self.use_color {
            self.out.fg(color).unwrap();
        }
    }

    fn reset(&mut self) {
        if self.use_color {
            self.out.reset().unwrap();
        }
    }
}

impl Report for TapReport {
    fn init(&mut self, plan: &[Task]) {
        writeln!(self.out, "TAP version 13").unwrap();
        writeln!(self.out, "1..{}", plan.len()).unwrap();
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

        self.fg(color);
        write!(self.out, "{} ", msg).unwrap();
        self.reset();

        writeln!(
            self.out,
            "{} - {}{}",
            self.count,
            task.full_name.join("::"),
            suffix.unwrap_or_default()
        )
        .unwrap();

        match task.status {
            Status::Success => {
                writeln!(self.out, "# completed in {:?}", task.duration).unwrap();
            }
            Status::Failure(code) => {
                writeln!(
                    self.out,
                    "# process returned {} after {:?}",
                    code, task.duration
                )
                .unwrap();
            }
            Status::Signaled(signame) => {
                writeln!(
                    self.out,
                    "# process was killed with {} after {:?}",
                    signame, task.duration
                )
                .unwrap();
            }
            Status::Timeout => {
                writeln!(self.out, "# timed out after {:?}", task.duration).unwrap();
            }
            Status::Skipped(_) => (),
        }

        if !ok {
            if !task.stdout.is_empty() {
                writeln!(self.out, "# --- stdout ---").unwrap();
                for line in task.stdout_as_string().lines() {
                    writeln!(self.out, "# {}", line).unwrap();
                }
            }
            if !task.stderr.is_empty() {
                writeln!(self.out, "# --- stderr ---").unwrap();
                for line in task.stderr_as_string().lines() {
                    writeln!(self.out, "# {}", line).unwrap();
                }
            }
        }
    }

    fn done(&mut self) {}
}
