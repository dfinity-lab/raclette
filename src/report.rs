use crate::execution::{CompletedTask, Report, Status, Task};
use term::color::{Color, BRIGHT_GREEN, BRIGHT_RED};

pub struct TapReport {
    out: Box<term::StdoutTerminal>,
    out_supports_color: bool,
    out_supports_reset: bool,
    count: usize,
    total: usize,
}

impl TapReport {
    pub fn new() -> Self {
        let out = term::stdout().unwrap();
        let out_supports_color = out.supports_color();
        let out_supports_reset = out.supports_reset();
        Self {
            out,
            out_supports_color,
            out_supports_reset,
            total: 0,
            count: 0,
        }
    }

    fn fg(&mut self, color: Color) {
        if self.out_supports_color {
            self.out.fg(color).unwrap();
        }
    }

    fn reset(&mut self) {
        if self.out_supports_reset {
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
        let ok = task.status == Status::Success;

        let (msg, color) = if ok { ("ok", BRIGHT_GREEN) } else { ("not ok", BRIGHT_RED) };

        self.fg(color);
        write!(self.out, "{} ", msg).unwrap();
        self.reset();

        writeln!(self.out, "{} - {}", self.count, task.full_name.join("/")).unwrap();

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
