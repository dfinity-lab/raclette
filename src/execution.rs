use crate::{config::Config, Options, TestTree, TreeNode};
use mio::unix::pipe;
use mio::{Events, Interest, Poll, Token};
use mio_signals as msig;
use nix::sys::signal::{killpg, Signal};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{self, fork, ForkResult, Pid};
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::mem::size_of;
use std::os::unix::io::AsRawFd;
use std::time::{Duration, Instant, SystemTime};
use std::{collections::HashMap, convert::TryInto};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// The token used to catch signals.
const SIGNAL_TOKEN: Token = Token(0);

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Success,
    Failure(i32),
    Signaled(&'static str),
    Timeout,
    Skipped(String),
}

impl Status {
    /// Returns whether a [Status] represents a non-failure. This includes
    /// [Status::Success] and [Status::Skipped]. Anything else is a failure
    /// of some sort.
    pub fn is_ok(&self) -> bool {
        match self {
            Status::Success => true,
            Status::Skipped(_) => true,
            _ => false,
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
enum InputSource {
    Stdout,
    Stderr,
    Report,
}

/// A task to be executed as a test.
pub struct Task {
    pub full_name: Vec<String>,
    work: super::GenericAssertion,
    options: Options,
}

impl Task {
    pub fn name(&self) -> String {
        self.full_name.join("::")
    }
}

/// A task that has just been spawned and started executing.
struct RunningTask {
    full_name: Vec<String>,
    pid: Pid,
    started_at: Instant,
    stdout_pipe: pipe::Receiver,
    stderr_pipe: pipe::Receiver,
    report_pipe: pipe::Receiver,
    stdout_buf: Vec<u8>,
    stderr_buf: Vec<u8>,
}

/// A task that is being observed by the test driver.
struct ObservedTask {
    full_name: Vec<String>,
    pid: Pid,
    started_at: Instant,
    stdout_pipe: Option<pipe::Receiver>,
    stderr_pipe: Option<pipe::Receiver>,
    status_and_duration: Option<(Status, Duration)>,
    // Part of the stderr/stdout of the task that has already been
    // captured.
    stdout_buf: Vec<u8>,
    stderr_buf: Vec<u8>,
    // Offset of the first byte in the captured output that has not
    // been displayed yet.  Only used if "nocapture" option is
    // enabled.
    stdout_offset: usize,
    stderr_offset: usize,
    // Similarly to stdout/stderr; tasks have a dedicate pipe to send
    report_pipe: Option<pipe::Receiver>,
    report_decoder: StreamDecoder,
}

/// A task that finished executing and is ready to be reported.
#[derive(Debug, Clone)]
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
    fn start(&mut self, task_name: String);
    fn report(&mut self, result: &CompletedTask);
    fn done(&mut self);

    fn stage(&mut self, full_name: &[String], stage_rep: StageReport) {
        let mut full_name: Vec<String> = Vec::from(full_name);
        full_name.push(stage_rep.stage_name);
        let completed_task = CompletedTask {
            full_name,
            duration: stage_rep.duration,
            stdout: Vec::new(),
            stderr: Vec::new(),
            status: Status::from(stage_rep.status),
        };
        self.report(&completed_task);
    }
}

pub struct StageReportSender {
    sender: pipe::Sender,
    // Since we define the stages to be linear, we just need to
    // keep one timestamp to report a stage's duration.
    started_at: SystemTime,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum StageStatus {
    Success,
    Failure(i32),
    Skipped(String),
}

impl From<StageStatus> for Status {
    fn from(s: StageStatus) -> Self {
        match s {
            StageStatus::Success => Status::Success,
            StageStatus::Failure(code) => Status::Failure(code),
            StageStatus::Skipped(reason) => Status::Skipped(reason),
        }
    }
}

#[derive(PartialEq, Debug, Serialize, Deserialize)]
pub struct StageReport {
    stage_name: String,
    status: StageStatus,
    duration: Duration,
}

impl StageReportSender {
    pub fn report_stage_status<N: ToString>(&mut self, stage_name: N, status: StageStatus) {
        let stage_name = stage_name.to_string();
        let end = std::time::SystemTime::now();
        let start = self.started_at;
        self.started_at = end;

        let payload = StageReport {
            stage_name,
            status,
            duration: end.duration_since(start).unwrap_or(Duration::from_secs(0)),
        };

        serialize_and_write(&mut self.sender, &payload).expect("Couldn't send");
    }
}

fn serialize_and_write<W: Write, A: Serialize>(w: &mut W, payload: &A) -> io::Result<usize> {
    let payload = bincode::serialize(payload).unwrap();
    let n = w.write(&(payload.len() as usize).to_be_bytes())?;
    let m = w.write(&payload)?;
    Ok(n + m)
}

struct StreamDecoder {
    buf: Vec<u8>,
    offset: usize,
}

impl StreamDecoder {
    fn new() -> Self {
        StreamDecoder {
            buf: Vec::new(),
            offset: 0,
        }
    }

    fn append(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }
    // Decode a message if there is enough data in the buffer.
    fn try_decode(&mut self) -> Option<StageReport> {
        let avail = self.buf.len() - self.offset;

        if avail > size_of::<usize>() {
            let payload_size = &self.buf[self.offset..self.offset + size_of::<usize>()];
            let payload_size = usize::from_be_bytes(payload_size.try_into().unwrap());

            if avail < size_of::<usize>() + payload_size {
                return None;
            }

            let payload = &self.buf
                [self.offset + size_of::<usize>()..self.offset + size_of::<usize>() + payload_size];
            let res: StageReport =
                bincode::deserialize(&payload).unwrap_or_else(|e| panic!(format!("{:?}", e)));
            // Update the offset
            self.offset = self.offset + size_of::<usize>() + payload_size;
            Some(res)
        } else {
            None
        }
    }
}

pub fn make_plan(config: &Config, t: TestTree) -> Vec<Task> {
    fn matches(name: &str, filter: &Option<String>) -> bool {
        filter.as_ref().map(|f| name.contains(f)).unwrap_or(true)
    }

    fn go(
        filter: &Option<String>,
        config: &Config,
        t: TestTree,
        mut path: Vec<String>,
        buf: &mut Vec<Task>,
        parent_opts: Options,
    ) {
        let skip_filter_applies = config.skip_filters.iter().any(|f| t.name().contains(f));

        match t {
            TestTree(TreeNode::Leaf {
                name,
                assertion,
                options,
            }) => {
                if !matches(&name, filter) || skip_filter_applies {
                    return;
                }
                path.push(name);
                buf.push(Task {
                    work: assertion,
                    full_name: path,
                    options: options.inherit(parent_opts),
                })
            }
            TestTree(TreeNode::Fork {
                name,
                tests,
                options,
            }) => {
                let effective_opts = options.inherit(parent_opts);
                if matches(&name, filter) && !skip_filter_applies {
                    path.push(name);
                    for t in tests {
                        go(&None, config, t, path.clone(), buf, effective_opts.clone());
                    }
                } else if !skip_filter_applies {
                    path.push(name);
                    for t in tests {
                        go(filter, config, t, path.clone(), buf, effective_opts.clone());
                    }
                }
            }
        }
    }

    let mut plan = Vec::new();
    go(
        &config.filter,
        &config,
        t,
        Vec::new(),
        &mut plan,
        Options::default(),
    );
    plan
}

fn launch(task: Task) -> RunningTask {
    let (stdout_sender, stdout_receiver) = pipe::new().unwrap();
    let (stderr_sender, stderr_receiver) = pipe::new().unwrap();
    let (report_sender, report_receiver) = pipe::new().unwrap();

    stdout_receiver.set_nonblocking(true).unwrap();
    stderr_receiver.set_nonblocking(true).unwrap();
    report_receiver.set_nonblocking(true).unwrap();

    let full_name = task.full_name;

    io::stdout().lock().flush().unwrap();
    io::stderr().lock().flush().unwrap();

    let pid = match fork().expect("failed to fork") {
        ForkResult::Child => {
            let self_pid = unistd::getpid();
            unistd::setpgid(self_pid, self_pid).expect("child: failed to set PGID");

            std::mem::drop(stdout_receiver);
            std::mem::drop(stderr_receiver);
            std::mem::drop(report_receiver);

            let stdout_fd = std::io::stdout().as_raw_fd();
            let stderr_fd = std::io::stderr().as_raw_fd();

            unistd::close(stdout_fd).expect("child: failed to close stdout");
            unistd::dup2(stdout_sender.as_raw_fd(), stdout_fd).unwrap();

            unistd::close(stderr_fd).expect("child: failed to close stderr");
            unistd::dup2(stderr_sender.as_raw_fd(), stderr_fd).unwrap();

            let mut stage_reporter = StageReportSender {
                sender: report_sender,
                started_at: SystemTime::now(),
            };
            (task.work)(&mut stage_reporter);
            std::process::exit(0)
        }
        ForkResult::Parent { child, .. } => {
            // We create a new process group for the child to be able
            // to kill all the processes spawned by the test if the
            // test times out.
            match unistd::setpgid(child, child) {
                // It might happen that the child process completes
                // before parent calls setpgid.  In this case the call
                // will fail with ESRCH errno, which can be safely
                // ignored.
                Err(nix::Error::Sys(nix::errno::Errno::ESRCH)) => (),
                Err(e) => panic!("failed to set PGID of the child: {}", e),
                Ok(()) => (),
            }
            child
        }
    };

    RunningTask {
        full_name,
        pid,
        started_at: Instant::now(),
        stdout_pipe: stdout_receiver,
        stderr_pipe: stderr_receiver,
        report_pipe: report_receiver,
        stdout_buf: Vec::new(),
        stderr_buf: Vec::new(),
    }
}

fn make_token(pid: Pid, source: InputSource) -> Token {
    match source {
        InputSource::Stdout => Token((pid.as_raw() as usize) << 2),
        InputSource::Stderr => Token((pid.as_raw() as usize) << 2 | 1),
        InputSource::Report => Token((pid.as_raw() as usize) << 2 | 2),
    }
}

fn split_token(token: Token) -> (Pid, InputSource) {
    let src = if token.0 & 2 == 2 {
        InputSource::Report
    } else if token.0 & 1 == 1 {
        InputSource::Stderr
    } else {
        InputSource::Stdout
    };
    (Pid::from_raw((token.0 >> 2) as i32), src)
}

fn observe(task: RunningTask, poll: &mut Poll) -> ObservedTask {
    let RunningTask {
        full_name,
        pid,
        started_at,
        mut stdout_pipe,
        mut stderr_pipe,
        mut report_pipe,
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
    poll.registry()
        .register(
            &mut report_pipe,
            make_token(pid, InputSource::Report),
            Interest::READABLE,
        )
        .unwrap();

    ObservedTask {
        full_name,
        pid,
        started_at,
        stdout_pipe: Some(stdout_pipe),
        stderr_pipe: Some(stderr_pipe),
        report_pipe: Some(report_pipe),
        status_and_duration: None,
        stdout_buf,
        stderr_buf,
        stdout_offset: 0,
        stderr_offset: 0,
        report_decoder: StreamDecoder::new(),
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

/// Displays as many complete lines from "buf" as possible starting
/// from "pos".  The pos is advanced to the beginning of the last
/// incomplete line.
fn display_lines(wrt: &mut dyn Write, buf: &[u8], pos: &mut usize) {
    for i in (*pos..buf.len()).rev() {
        if buf[i] == b'\n' {
            write!(wrt, "{}", String::from_utf8_lossy(&buf[*pos..=i])).expect("Failed to write");
            *pos = i + 1;
            return;
        }
    }
}

/// Output the remaining part of the buffer, assuming that it ends
/// with an incomplete line.
fn flush_output(wrt: &mut dyn Write, buf: &[u8], pos: &mut usize) {
    let n = buf.len();
    if *pos < n {
        writeln!(wrt, "{}", String::from_utf8_lossy(&buf[*pos..n])).expect("Failed to writeln");
        *pos = n;
    }
}

pub fn execute(
    config: &Config,
    mut tasks: Vec<Task>,
    report: &mut dyn Report,
) -> Vec<CompletedTask> {
    let timeout = config.timeout.unwrap_or(DEFAULT_TIMEOUT);
    let jobs = config.jobs.unwrap_or_else(num_cpus::get);
    let poll_timeout = Duration::from_millis(100);

    let mut poll = Poll::new().expect("failed to create poll");
    let mut signals = msig::Signals::new(msig::SignalSet::all())
        .expect("failed to create mio_signals::Signals object");

    poll.registry()
        .register(&mut signals, SIGNAL_TOKEN, Interest::READABLE)
        .expect("failed to register signal handler in a Poll registry");

    let mut events = Events::with_capacity(jobs * 2);
    let mut buf = vec![0u8; 4096];

    report.init(&tasks);

    let mut observed_tasks = HashMap::<Pid, ObservedTask>::new();
    let mut completed_pids = Vec::<Pid>::new();
    let mut task_results = Vec::<CompletedTask>::new();

    tasks.reverse();

    while !tasks.is_empty() || !observed_tasks.is_empty() {
        while observed_tasks.len() < jobs {
            match tasks.pop() {
                Some(mut task) => {
                    report.start(task.name());
                    if let Some(reason) = task.options.skip_reason.take() {
                        report.report(&skip_task(task, reason));
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
            if event.token() == SIGNAL_TOKEN {
                match signals.receive().expect("failed to receive signal") {
                    Some(sig) => {
                        eprintln!(
                            "Received signal {:?}, canceling {} tasks...",
                            sig,
                            observed_tasks.len()
                        );

                        for pid in observed_tasks.keys() {
                            eprintln!("Killing process group {:?}...", *pid);
                            let _ = killpg(*pid, Signal::SIGKILL);
                        }

                        std::process::exit(1)
                    }
                    None => {
                        continue;
                    }
                }
            }

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
                            if config.nocapture {
                                display_lines(
                                    &mut std::io::stdout(),
                                    &observed_task.stdout_buf,
                                    &mut observed_task.stdout_offset,
                                );
                            }
                        }
                    }
                    if event.is_read_closed() {
                        if config.nocapture {
                            flush_output(
                                &mut std::io::stdout(),
                                &observed_task.stderr_buf,
                                &mut observed_task.stdout_offset,
                            );
                        }
                        observed_task.stdout_pipe = None;
                    }
                }
                InputSource::Stderr => {
                    if event.is_readable() {
                        if let Some(ref mut pipe) = observed_task.stderr_pipe {
                            let n = pipe.read(&mut buf).expect("failed to read STDERR");
                            observed_task.stderr_buf.extend_from_slice(&buf[0..n]);
                            if config.nocapture {
                                display_lines(
                                    &mut std::io::stderr(),
                                    &observed_task.stderr_buf,
                                    &mut observed_task.stderr_offset,
                                );
                            }
                        }
                    }
                    if event.is_read_closed() {
                        if config.nocapture {
                            flush_output(
                                &mut std::io::stderr(),
                                &observed_task.stderr_buf,
                                &mut observed_task.stderr_offset,
                            );
                        }
                        observed_task.stderr_pipe = None;
                    }
                }
                InputSource::Report => {
                    if event.is_readable() {
                        if let Some(ref mut pipe) = observed_task.report_pipe {
                            let n = pipe.read(&mut buf).expect("failed to read REPORT");
                            observed_task.report_decoder.append(&buf[0..n]);
                            if let Some(stage_rep) = observed_task.report_decoder.try_decode() {
                                report.stage(&observed_task.full_name, stage_rep);
                            }
                        }
                    }
                    if event.is_read_closed() {
                        observed_task.report_pipe = None;
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
                    killpg(observed_task.pid, Signal::SIGKILL).unwrap();
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

            let completed_task = CompletedTask {
                full_name: observed_task.full_name,
                duration,
                stdout: observed_task.stdout_buf,
                stderr: observed_task.stderr_buf,
                status,
            };

            report.report(&completed_task);
            task_results.push(completed_task);
        }

        completed_pids.clear();
    }

    report.done();
    task_results
}

mod test {
    #[allow(unused_imports)]
    use super::*;

    #[test]
    fn make_token_is_correct() {
        for src in vec![
            InputSource::Stdout,
            InputSource::Stderr,
            InputSource::Report,
        ] {
            assert_eq!(
                split_token(make_token(Pid::this(), src.clone())),
                (Pid::this(), src)
            )
        }
    }

    #[test]
    fn stream_decoder_is_correct() {
        let s1 = StageReport {
            stage_name: "s1".to_string(),
            status: StageStatus::Success,
            duration: Duration::from_millis(111),
        };
        let s2 = StageReport {
            stage_name: "s2".to_string(),
            status: StageStatus::Success,
            duration: Duration::from_millis(222),
        };
        let s3 = StageReport {
            stage_name: "s3".to_string(),
            status: StageStatus::Failure(42),
            duration: Duration::from_millis(333),
        };

        let mut dec = StreamDecoder::new();
        let mut buf = Vec::new();
        for s in vec![&s1, &s2, &s3] {
            serialize_and_write(&mut buf, s).unwrap();
        }

        dec.append(&buf);
        assert_eq!(dec.try_decode(), Some(s1));
        assert_eq!(dec.try_decode(), Some(s2));
        assert_eq!(dec.try_decode(), Some(s3));
        assert_eq!(dec.try_decode(), None);
    }
}
