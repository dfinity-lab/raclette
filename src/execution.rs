use crate::{TestTree, TreeNode};
use mio::unix::pipe;
use mio::{Events, Interest, Poll, Token};
use nix::sys::signal::{kill, Signal};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{self, fork, ForkResult, Pid};
use std::io::{self, Read, Write};
use std::os::unix::io::AsRawFd;
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq)]
pub enum Status {
    Success,
    Failure(i32),
    Signaled(&'static str),
    Timeout,
}

pub struct Task {
    pub full_name: Vec<String>,
    work: super::GenericAssertion,
}

struct RunningTask {
    full_name: Vec<String>,
    pid: Pid,
    started_at: Instant,
    stdout_pipe: pipe::Receiver,
    stderr_pipe: pipe::Receiver,
    stdout_buf: Vec<u8>,
    stderr_buf: Vec<u8>,
}

#[derive(Debug)]
pub struct CompletedTask {
    pub full_name: Vec<String>,
    pub duration: Duration,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub status: Status,
}

impl CompletedTask {
    pub fn stdout_as_string(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.stdout)
    }

    pub fn stderr_as_string(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.stderr)
    }
}

pub trait Report {
    fn init(&mut self, plan: &[Task]);
    fn report(&mut self, result: CompletedTask);
    fn done(&mut self);
}

pub fn make_plan(t: TestTree) -> Vec<Task> {
    fn go(t: TestTree, mut path: Vec<String>, buf: &mut Vec<Task>) {
        match t {
            TestTree(TreeNode::Leaf { name, assertion }) => {
                path.push(name);
                buf.push(Task {
                    work: assertion,
                    full_name: path,
                })
            }
            TestTree(TreeNode::Fork { name, tests }) => {
                path.push(name);
                for t in tests {
                    go(t, path.clone(), buf);
                }
            }
        }
    }
    let mut plan = Vec::new();
    go(t, Vec::new(), &mut plan);
    plan
}

fn launch(task: Task) -> RunningTask {
    let (stdout_sender, stdout_receiver) = pipe::new().unwrap();
    let (stderr_sender, stderr_receiver) = pipe::new().unwrap();

    stdout_receiver.set_nonblocking(true).unwrap();
    stderr_receiver.set_nonblocking(true).unwrap();

    let full_name = task.full_name;

    io::stdout().lock().flush().unwrap();
    io::stderr().lock().flush().unwrap();

    let pid = match fork().expect("failed to fork") {
        ForkResult::Child => {
            std::mem::drop(stdout_receiver);
            std::mem::drop(stderr_receiver);

            let stdout_fd = std::io::stdout().as_raw_fd();
            let stderr_fd = std::io::stderr().as_raw_fd();

            unistd::close(stdout_fd).expect("child: failed to close stdout");
            unistd::dup2(stdout_sender.as_raw_fd(), stdout_fd).unwrap();

            unistd::close(stderr_fd).expect("child: failed to close stderr");
            unistd::dup2(stderr_sender.as_raw_fd(), stderr_fd).unwrap();

            (task.work)();
            std::process::exit(0)
        }
        ForkResult::Parent { child, .. } => child,
    };

    RunningTask {
        full_name,
        pid,
        started_at: Instant::now(),
        stdout_pipe: stdout_receiver,
        stderr_pipe: stderr_receiver,
        stdout_buf: Vec::new(),
        stderr_buf: Vec::new(),
    }
}

pub fn execute(tasks: Vec<Task>, report: &mut dyn Report) {
    let timeout = Duration::from_secs(10);

    let mut poll = Poll::new().expect("failed to create poll");
    let mut events = Events::with_capacity(10);

    report.init(&tasks);

    for task in tasks {
        let mut running_task = launch(task);

        poll.registry()
            .register(&mut running_task.stdout_pipe, Token(1), Interest::READABLE)
            .unwrap();
        poll.registry()
            .register(&mut running_task.stderr_pipe, Token(2), Interest::READABLE)
            .unwrap();

        loop {
            poll.poll(&mut events, Some(timeout))
                .expect("failed to poll");
            for event in &events {
                if event.token() == Token(1) && event.is_readable() {
                    running_task
                        .stdout_pipe
                        .read(&mut running_task.stdout_buf)
                        .expect("failed to read STDOUT");
                }
                if event.token() == Token(2) && event.is_readable() {
                    running_task
                        .stderr_pipe
                        .read(&mut running_task.stderr_buf)
                        .expect("failed to read STDERR");
                }
            }

            let mut maybe_status =
                match waitpid(Some(running_task.pid), Some(WaitPidFlag::WNOHANG)).unwrap() {
                    WaitStatus::Exited(_, code) => Some(if code == 0 {
                        Status::Success
                    } else {
                        Status::Failure(code)
                    }),
                    WaitStatus::Signaled(_, sig, _) => Some(Status::Signaled(sig.as_str())),
                    _ => None,
                };

            let duration = running_task.started_at.elapsed();

            if maybe_status.is_none() && duration >= timeout {
                kill(running_task.pid, Signal::SIGKILL).unwrap();
                maybe_status = Some(Status::Timeout);
            }

            if let Some(status) = maybe_status {
                running_task
                    .stdout_pipe
                    .read_to_end(&mut running_task.stdout_buf)
                    .expect("failed to completely read STDOUT of a dead process");
                running_task
                    .stderr_pipe
                    .read_to_end(&mut running_task.stderr_buf)
                    .expect("failed to completely read STDERR of a dead process");

                report.report(CompletedTask {
                    full_name: running_task.full_name,
                    duration: running_task.started_at.elapsed(),
                    stdout: running_task.stdout_buf,
                    stderr: running_task.stderr_buf,
                    status,
                });
                break;
            }
        }
    }
    report.done();
}
