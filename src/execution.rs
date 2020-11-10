use crate::{config::Config, Options, TestTree, TreeNode};
use mio::unix::pipe;
use mio::{Events, Interest, Poll, Token};
use nix::sys::signal::{kill, Signal};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{self, fork, ForkResult, Pid};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::os::unix::io::AsRawFd;
use std::time::{Duration, Instant};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, PartialEq)]
pub enum Status {
    Success,
    Failure(i32),
    Signaled(&'static str),
    Timeout,
    Skipped(String),
}

enum InputSource {
    Stdout,
    Stderr,
}

pub struct Task {
    pub full_name: Vec<String>,
    work: super::GenericAssertion,
    options: Options,
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

    pub fn name(&self) -> String {
        self.full_name.join("::")
    }
}

pub trait Report {
    fn init(&mut self, plan: &[Task]);
    fn report(&mut self, result: CompletedTask);
    fn done(&mut self);
}

pub fn make_plan(config: &Config, t: TestTree) -> Vec<Task> {
    fn matches(name: &str, filter: &Option<String>) -> bool {
        filter.as_ref().map(|f| name.contains(f)).unwrap_or(true)
    }

    fn go(
        f: &Option<String>,
        t: TestTree,
        mut path: Vec<String>,
        buf: &mut Vec<Task>,
        parent_opts: Options,
    ) {
        match t {
            TestTree(TreeNode::Leaf {
                name,
                assertion,
                options,
            }) => {
                if !matches(&name, f) {
                    return;
                }
                path.push(name);
                buf.push(Task {
                    work: assertion,
                    full_name: path,
                    options: Options::inherit(options, parent_opts),
                })
            }
            TestTree(TreeNode::Fork {
                name,
                tests,
                options,
            }) => {
                let effective_opts = Options::inherit(options, parent_opts);
                if matches(&name, f) {
                    path.push(name);
                    for t in tests {
                        go(&None, t, path.clone(), buf, effective_opts.clone());
                    }
                } else {
                    path.push(name);
                    for t in tests {
                        go(f, t, path.clone(), buf, effective_opts.clone());
                    }
                }
            }
        }
    }

    let mut plan = Vec::new();
    go(&config.filter, t, Vec::new(), &mut plan, Options::default());
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

fn make_token(pid: Pid, source: InputSource) -> Token {
    match source {
        InputSource::Stdout => Token((pid.as_raw() as usize) << 1),
        InputSource::Stderr => Token((pid.as_raw() as usize) << 1 | 1),
    }
}

fn split_token(token: Token) -> (Pid, InputSource) {
    let src = if token.0 & 1 == 0 {
        InputSource::Stdout
    } else {
        InputSource::Stderr
    };
    (Pid::from_raw((token.0 >> 1) as i32), src)
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
        .register(
            &mut stdout_pipe,
            make_token(pid, InputSource::Stdout),
            Interest::READABLE,
        )
        .unwrap();
    poll.registry()
        .register(
            &mut stderr_pipe,
            make_token(pid, InputSource::Stderr),
            Interest::READABLE,
        )
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

fn skip_task(task: Task, reason: String) -> CompletedTask {
    CompletedTask {
        full_name: task.full_name,
        duration: Duration::default(),
        stdout: vec![],
        stderr: vec![],
        status: Status::Skipped(reason),
    }
}

pub fn execute(config: &Config, mut tasks: Vec<Task>, report: &mut dyn Report) {
    let timeout = config.timeout.unwrap_or(DEFAULT_TIMEOUT);
    let jobs = config.jobs.unwrap_or_else(|| num_cpus::get());
    let poll_timeout = Duration::from_millis(100);

    let mut poll = Poll::new().expect("failed to create poll");
    let mut events = Events::with_capacity(jobs * 2);
    let mut buf = vec![0u8; 4096];

    report.init(&tasks);

    let mut observed_tasks = HashMap::<Pid, ObservedTask>::new();
    let mut completed_pids = Vec::<Pid>::new();

    tasks.reverse();

    while !tasks.is_empty() || !observed_tasks.is_empty() {
        while observed_tasks.len() < jobs {
            match tasks.pop() {
                Some(mut task) => {
                    if let Some(reason) = task.options.skip_reason.take() {
                        report.report(skip_task(task, reason));
                        continue;
                    }

                    let running_task = launch(task);
                    let observed_task = observe(running_task, &mut poll);
                    observed_tasks.insert(observed_task.pid, observed_task);
                }
                None => {
                    break;
                }
            }
        }

        poll.poll(&mut events, Some(poll_timeout))
            .expect("failed to poll");

        for event in &events {
            let (pid, src) = split_token(event.token());

            let observed_task = observed_tasks
                .get_mut(&pid)
                .expect("received an event for a process that is not observed");

            match src {
                InputSource::Stdout => {
                    if event.is_readable() {
                        if let Some(ref mut pipe) = observed_task.stdout_pipe {
                            let n = pipe.read(&mut buf).expect("failed to read STDOUT");
                            observed_task.stdout_buf.extend_from_slice(&buf[0..n]);
                        }
                    }
                    if event.is_read_closed() {
                        observed_task.stdout_pipe = None;
                    }
                }
                InputSource::Stderr => {
                    if event.is_readable() {
                        if let Some(ref mut pipe) = observed_task.stderr_pipe {
                            let n = pipe.read(&mut buf).expect("failed to read STDERR");
                            observed_task.stderr_buf.extend_from_slice(&buf[0..n]);
                        }
                    }
                    if event.is_read_closed() {
                        observed_task.stderr_pipe = None;
                    }
                }
            }
        }

        for (pid, observed_task) in observed_tasks.iter_mut() {
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
                status_and_duration: Some(_),
                stdout_pipe: None,
                stderr_pipe: None,
                ..
            } = observed_task
            {
                completed_pids.push(*pid);
            }
        }

        for pid in completed_pids.iter() {
            let observed_task = observed_tasks.remove(pid).unwrap();
            let (status, duration) = observed_task.status_and_duration.unwrap();

            report.report(CompletedTask {
                full_name: observed_task.full_name,
                duration,
                stdout: observed_task.stdout_buf,
                stderr: observed_task.stderr_buf,
                status,
            });
        }

        completed_pids.clear();
    }

    report.done();
}
