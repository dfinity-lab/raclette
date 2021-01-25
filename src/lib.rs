pub mod config;
mod execution;
mod report;

pub use config::Config;

use std::any::Any;
use std::string::ToString;

type GenericAssertion = Box<dyn FnOnce() + 'static>;

pub struct TestTree(TreeNode);

impl TestTree {
    pub fn name(&self) -> &str {
        match self.0 {
            TreeNode::Leaf { ref name, .. } => name.as_str(),
            TreeNode::Fork { ref name, .. } => name.as_str(),
        }
    }
}

#[derive(Clone, Default)]
struct Options {
    pub(crate) skip_reason: Option<String>,
}

impl Options {
    fn inherit(self, parent: Options) -> Options {
        Options {
            skip_reason: self.skip_reason.or(parent.skip_reason),
        }
    }
}

enum TreeNode {
    Leaf {
        name: String,
        assertion: GenericAssertion,
        options: Options,
    },
    Fork {
        name: String,
        tests: Vec<TestTree>,
        options: Options,
    },
}

fn try_get_panic_msg<'a>(obj: &'a Box<dyn Any + Send + 'static>) -> Option<&'a str> {
    obj.downcast_ref::<&str>()
        .copied()
        .or_else(|| obj.downcast_ref::<String>().map(|s| s.as_str()))
}

pub fn test_case<N: ToString, A: FnOnce() + 'static>(name: N, assertion: A) -> TestTree {
    TestTree(TreeNode::Leaf {
        name: name.to_string(),
        assertion: Box::new(assertion),
        options: Options::default(),
    })
}

pub fn test_suite(name: impl ToString, tests: Vec<TestTree>) -> TestTree {
    TestTree(TreeNode::Fork {
        name: name.to_string(),
        tests,
        options: Options::default(),
    })
}

fn with_options(mut test: TestTree, f: impl FnOnce(&mut Options)) -> TestTree {
    match test {
        TestTree(TreeNode::Leaf {
            ref mut options, ..
        }) => {
            f(options);
            test
        }
        TestTree(TreeNode::Fork {
            ref mut options, ..
        }) => {
            f(options);
            test
        }
    }
}

pub fn skip(reason: impl ToString, test: TestTree) -> TestTree {
    with_options(test, |opts| opts.skip_reason = Some(reason.to_string()))
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

/// Runs raclette with a default config but reads the command line arguments
/// and overrides settings from the default config. If this behavior is undesired
/// refer to [default_main_no_config_override] instead.
pub fn default_main(default_config: Config, tree: TestTree) {
    use config::ConfigParseError as E;

    let override_config = Config::from_args().unwrap_or_else(|err| match err {
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

    let config = override_config.merge(default_config);
    default_main_no_config_override(config, tree);
}

/// Runs raclette with a fixed configuration. Does not inspect command line options.
pub fn default_main_no_config_override(config: Config, tree: TestTree) {
    use config::Format;

    let writer = report::ColorWriter::new(config.color);
    let mut report: Box<dyn execution::Report> = match config.format {
        Format::Auto | Format::LibTest => Box::new(report::LibTestReport::new(writer)),
        Format::Json => Box::new(report::JsonReport::new(writer)),
        Format::Tap => Box::new(report::TapReport::new(writer)),
    };
    let plan = execution::make_plan(&config, tree);

    execution::execute(&config, plan, &mut *report);
}
