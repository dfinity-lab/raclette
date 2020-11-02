mod execution;

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
    origin: Box<dyn std::any::Any + Send + 'static>,
}

impl fmt::Debug for PanicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(s) = self.origin.downcast_ref::<&str>() {
            return write!(f, "{}", s);
        }
        if let Some(s) = self.origin.downcast_ref::<String>() {
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
                    ).unwrap();
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
                    ).unwrap();
                    for line in format!("{}", err).lines() {
                        tm.fg(RED).unwrap();
                        write!(
                            tm,
                            "{:width$}{}\n",
                            "",
                            line,
                            width = width + DEPTH_MULTIPLIER
                        ).unwrap();
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
    for result in execution::execute(execution::make_plan(tree)) {
        println!("TEST {} {:?} in {:?}", result.full_name.join("/"),  result.status, result.duration);
        if !result.stdout.is_empty() {
            println!("--- stdout ---");
            println!("{}", String::from_utf8_lossy(&result.stdout[..]));
            println!();
        }
        if !result.stderr.is_empty() {
            println!("--- stderr ---");
            println!("{}", String::from_utf8_lossy(&result.stderr[..]));
            println!();
        }
    }
}
