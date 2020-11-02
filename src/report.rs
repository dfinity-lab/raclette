use crate::execution::{CompletedTask, Report, Status, Task};

pub struct TapReport {
    count: usize,
    total: usize,
}

impl TapReport {
    pub fn new() -> Self {
        Self { total: 0, count: 0 }
    }
}

impl Report for TapReport {
    fn init(&mut self, plan: &[Task]) {
        println!("TAP version 13");
        println!("1..{}", plan.len());
        self.total = plan.len();
    }

    fn report(&mut self, task: CompletedTask) {
        self.count += 1;
        let ok = task.status == Status::Success;

        println!(
            "{} {} - {}",
            if ok { "ok" } else { "not ok" },
            self.count,
            task.full_name.join("/")
        );

        match task.status {
            Status::Success => {
                println!("# completed in {:?}", task.duration);
            }
            Status::Failure(code) => {
                println!("# process returned {} after {:?}", code, task.duration);
            }
            Status::Signaled(signame) => {
                println!(
                    "# process was killed with {} after {:?}",
                    signame, task.duration
                );
            }
            Status::Timeout => {
                println!("# timed out after {:?}", task.duration);
            }
        }

        if !ok {
            if !task.stdout.is_empty() {
                println!("# --- stdout ---");
                for line in task.stdout_as_string().lines() {
                    println!("# {}", line);
                }
            }
            if !task.stderr.is_empty() {
                println!("# --- stderr ---");
                for line in task.stderr_as_string().lines() {
                    println!("# {}", line);
                }
            }
        }
    }

    fn done(&mut self) {}
}
