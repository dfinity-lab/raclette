use crate::{TestTree, TreeNode};
use mio::unix::pipe;
use mio::{Events, Interest, Poll, Token};
use nix::sys::signal::{kill, Signal};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{self, fork, ForkResult, Pid};
use std::io::{self, Read, Write};
use std::os::unix::io::AsRawFd;
use std::time::{Duration, Instant};

const STDOUT_TOKEN: Token = Token(0);
const STDERR_TOKEN: Token = Token(1);

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

struct ObservedTask {
    full_name: Vec<String>,
    pid: Pid,
    started_at: Instant,
    stdout_pipe: Option<pipe::Receiver>,
    stderr_pipe: Option<pipe::Receiver>,
    status_and_duration: Option<(Status, Duration)>,
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

fn observe(task: RunningTask, poll: &mut Poll) -> ObservedTask {
    let RunningTask {
        full_name,
        pid,
        started_at,
        mut stdout_pipe,
        mut stderr_pipe,
        stdout_buf,
        stderr_buf,
    } = task;

    poll.registry()
        .register(&mut stdout_pipe, STDOUT_TOKEN, Interest::READABLE)
        .unwrap();
    poll.registry()
        .register(&mut stderr_pipe, STDERR_TOKEN, Interest::READABLE)
        .unwrap();

    ObservedTask {
        full_name,
        pid,
        started_at,
        stdout_pipe: Some(stdout_pipe),
        stderr_pipe: Some(stderr_pipe),
        status_and_duration: None,
        stdout_buf,
        stderr_buf,
    }
}

pub fn execute(tasks: Vec<Task>, report: &mut dyn Report) {
    let timeout = Duration::from_secs(10);
    let poll_timeout = Duration::from_millis(100);

    let mut poll = Poll::new().expect("failed to create poll");
    let mut events = Events::with_capacity(10);

    report.init(&tasks);

    for task in tasks {
        let running_task = launch(task);
        let mut observed_task = observe(running_task, &mut poll);

        loop {
            poll.poll(&mut events, Some(poll_timeout))
                .expect("failed to poll");

            let mut buf = vec![0u8; 4096];

            for event in &events {
                if event.token() == STDOUT_TOKEN {
                    if event.is_readable() {
                        if let Some(ref mut pipe) = observed_task.stdout_pipe {
                            let n = pipe.read(&mut buf)
                                .expect("failed to read STDOUT");
                            observed_task.stdout_buf.extend_from_slice(&buf[0..n]);
                        }
                    }
                    if event.is_read_closed() {
                        observed_task.stdout_pipe = None;
                    }
                }

                if event.token() == STDERR_TOKEN {
                    if event.is_readable() {
                        if let Some(ref mut pipe) = observed_task.stderr_pipe {
                            let n = pipe.read(&mut buf)
                                .expect("failed to read STDERR");
                            observed_task.stderr_buf.extend_from_slice(&buf[0..n]);
                        }
                    }
                    if event.is_read_closed() {
                        observed_task.stderr_pipe = None;
                    }
                }
            }

            if observed_task.status_and_duration.is_none() {
                let duration = observed_task.started_at.elapsed();

                let mut maybe_status =
                    match waitpid(Some(observed_task.pid), Some(WaitPidFlag::WNOHANG)).unwrap() {
                        WaitStatus::Exited(_, code) => Some(if code == 0 {
                            (Status::Success, duration)
                        } else {
                            (Status::Failure(code), duration)
                        }),
                        WaitStatus::Signaled(_, sig, _) => {
                            Some((Status::Signaled(sig.as_str()), duration))
                        }
                        _ => None,
                    };

                if maybe_status.is_none() && duration >= timeout {
                    kill(observed_task.pid, Signal::SIGKILL).unwrap();
                    maybe_status = Some((Status::Timeout, duration));
                }

                observed_task.status_and_duration = maybe_status;
            }

            if let ObservedTask {
                full_name,
                status_and_duration: Some((status, duration)),
                stdout_pipe: None,
                stderr_pipe: None,
                stdout_buf,
                stderr_buf,
                ..
            } = observed_task
            {
                report.report(CompletedTask {
                    full_name,
                    duration,
                    stdout: stdout_buf,
                    stderr: stderr_buf,
                    status,
                });

                break;
            }
        }
    }
    report.done();
}
