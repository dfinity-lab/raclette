pub mod config;
mod execution;
mod report;

use std::any::Any;
use std::fmt;
use std::string::ToString;
use term::{
    self,
    color::{GREEN, RED},
};

type GenericError = Box<dyn std::error::Error + 'static>;
type GenericAssertion = Box<dyn FnOnce() + std::panic::UnwindSafe + 'static>;

pub struct TestTree(TreeNode);

enum TreeNode {
    Leaf {
        name: String,
        assertion: GenericAssertion,
    },
    Fork {
        name: String,
        tests: Vec<TestTree>,
    },
}

fn run_assertion(a: GenericAssertion) -> Result<(), GenericError> {
    match std::panic::catch_unwind(a) {
        Ok(_) => Ok(()),
        Err(origin) => Err(Box::new(PanicError { origin })),
    }
}

struct PanicError {
    origin: Box<dyn Any + Send + 'static>,
}

fn try_get_panic_msg<'a>(obj: &'a Box<dyn Any + Send + 'static>) -> Option<&'a str> {
    obj.downcast_ref::<&str>()
        .map(|s| *s)
        .or_else(|| obj.downcast_ref::<String>().map(|s| s.as_str()))
}

impl fmt::Debug for PanicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(s) = try_get_panic_msg(&self.origin) {
            return write!(f, "{}", s);
        }
        write!(f, "PANICKED")
    }
}

impl fmt::Display for PanicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for PanicError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

pub fn test_case<N: ToString, A: FnOnce() + std::panic::UnwindSafe + 'static>(
    name: N,
    assertion: A,
) -> TestTree {
    TestTree(TreeNode::Leaf {
        name: name.to_string(),
        assertion: Box::new(assertion),
    })
}

pub fn test_suite<N: ToString>(name: N, tests: Vec<TestTree>) -> TestTree {
    TestTree(TreeNode::Fork {
        name: name.to_string(),
        tests: tests,
    })
}

pub fn should_panic(
    expected_msg: &str,
    f: impl FnOnce() + std::panic::UnwindSafe + 'static,
) -> impl FnOnce() + std::panic::UnwindSafe + 'static {
    let msg = expected_msg.to_string();
    move || match std::panic::catch_unwind(f) {
        Ok(_) => {
            print!("note: test did not panic as expected");
            panic!();
        }
        Err(origin) => match try_get_panic_msg(&origin) {
            Some(actual_msg) if actual_msg.contains(&msg) => (),
            Some(actual_msg) => {
                println!(
                    "note: panic did not contain expected string\
              \n      panic message: `{:?}`\
              \n expected substring: `{:?}`\n",
                    actual_msg, msg
                );
                panic!();
            }
            None => panic!("failed to extract a message from panic payload"),
        },
    }
}

#[derive(Default)]
struct TestStats {
    run: usize,
    failed: usize,
}

impl TestStats {
    fn combine(&self, lhs: &TestStats) -> TestStats {
        TestStats {
            run: self.run + lhs.run,
            failed: self.failed + lhs.failed,
        }
    }
}

trait Formatter {
    fn initialize(&mut self, t: &TestTree);
    fn suite(&mut self, name: &str);
    fn test(&mut self, name: &str, result: Result<(), GenericError>);
    fn stats(&mut self, stats: &TestStats);
}

pub fn tasty_main(tree: TestTree) {
    const DEPTH_MULTIPLIER: usize = 2;

    fn max_name_width(d: usize, tree: &TestTree) -> usize {
        let n = match tree {
            TestTree(TreeNode::Leaf { name, .. }) => name.len(),
            TestTree(TreeNode::Fork { tests, .. }) => tests
                .iter()
                .map(|t| max_name_width(d + 1, t))
                .max()
                .unwrap_or(0),
        };
        n + d * DEPTH_MULTIPLIER
    }

    fn execute(d: usize, name_width: usize, t: TestTree) -> TestStats {
        let width = d * DEPTH_MULTIPLIER;

        let mut stats = TestStats::default();
        let mut tm = term::stdout().unwrap();

        match t {
            TestTree(TreeNode::Leaf { name, assertion }) => match run_assertion(assertion) {
                Ok(_) => {
                    write!(
                        tm,
                        "{:width$}{:name_width$} [OK]\n",
                        "",
                        name,
                        width = width,
                        name_width = name_width - width
                    )
                    .unwrap();
                    stats.run += 1;
                }
                Err(err) => {
                    write!(
                        tm,
                        "{:width$}{:name_width$} [FAILED]\n",
                        "",
                        name,
                        width = width,
                        name_width = name_width - width
                    )
                    .unwrap();
                    for line in format!("{}", err).lines() {
                        tm.fg(RED).unwrap();
                        write!(
                            tm,
                            "{:width$}{}\n",
                            "",
                            line,
                            width = width + DEPTH_MULTIPLIER
                        )
                        .unwrap();
                        tm.reset().unwrap();
                    }
                    stats.run += 1;
                    stats.failed += 1;
                }
            },
            TestTree(TreeNode::Fork { name, tests }) => {
                write!(tm, "{:width$}{}\n", "", name, width = width).unwrap();
                for test in tests.into_iter() {
                    stats = stats.combine(&execute(d + 1, name_width, test))
                }
            }
        }
        stats
    }
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let name_width = max_name_width(0, &tree);
    let stats = execute(0, name_width, tree);

    std::panic::set_hook(hook);

    if stats.failed > 0 {
        let mut t = term::stderr().unwrap();
        t.fg(RED).unwrap();
        write!(t, "{} out of {} tests failed\n", stats.failed, stats.run).unwrap();
        t.reset().unwrap();
        panic!("{} out of {} tests failed", stats.failed, stats.run)
    } else {
        let mut t = term::stdout().unwrap();
        t.fg(GREEN).unwrap();
        write!(t, "ran {} tests\n", stats.run).unwrap();
        t.reset().unwrap();
    }
}

pub fn default_main(tree: TestTree) {
    use config::ConfigParseError as E;

    let config = config::Config::from_args().unwrap_or_else(|err| match err {
        E::HelpRequested => {
            print!("{}", config::produce_help());
            std::process::exit(0)
        }
        E::OptionError(err) => {
            println!("{}", err);
            print!("{}", config::produce_help());
            std::process::exit(1)
        }
        E::UnknownArgs(args) => {
            println!("Unsupported arguments: {}", args.join(" "));
            print!("{}", config::produce_help());
            std::process::exit(1)
        }
        E::Unknown(err) => {
            println!("Failed to parse command line flags: {}", err);
            std::process::exit(1)
        }
    });

    let mut report = report::TapReport::new(config.color);
    let plan = execution::make_plan(&config, tree);

    execution::execute(&config, plan, &mut report);
}
