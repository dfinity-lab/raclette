use raclette::*;
use std::time::Duration;

fn mult_table_tests() -> Vec<TestTree> {
    let mut tests = vec![];
    for i in 1..9 {
        for j in 1..9 {
            tests.push(test_case(
                format!("check {}×{} = {}", i, j, i * j),
                move || assert_eq!(i * j, i * j),
            ))
        }
    }
    tests
}

fn zero() -> u64 {
    0
}

fn loop_infinitely() {
    loop {
        println!("watching a second passing by...");
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn test_my_reporter(rep: &mut StageReportSender) {
    rep.report_stage_start("first");
    println!("Sleeping one second");
    std::thread::sleep(Duration::from_millis(1234));
    rep.report_stage_end("first", StageStatus::Success);
}

fn tests() -> TestTree {
    test_suite(
        "all",
        vec![
            test_suite(
                "arithmetics",
                vec![
                    test_case("addition", || assert_eq!(4, 2 + 2)),
                    test_case(
                        "bad math",
                        should_panic("assertion failed", || assert_eq!(47, 7 * 7)),
                    ),
                    test_case(
                        "div by zero",
                        should_panic("zero", || {
                            let _x = 3 / zero();
                        }),
                    ),
                    test_suite("multiplication", mult_table_tests()),
                ],
            ),
            test_case("infinite loop 1", loop_infinitely),
            test_case("infinite loop 2", loop_infinitely),
            skip(
                "two infinite loops are enough",
                test_case("infinite loop 3", loop_infinitely),
            ),
            test_case_reporter("with a reporter", test_my_reporter),
        ],
    )
}

// The test suite above is engineered to have two timeouts and one
// skipped test.
const NUM_FAILURES: usize = 2;
const NUM_IGNORED: usize = 1;

#[test]
fn raclette_sample_main() {
    let completed_tasks = default_main(Config::default().format(config::Format::Json), tests());
    let failed_tasks: Vec<CompletedTask> = completed_tasks
        .into_iter()
        .filter(|task| !task.status.is_ok())
        .collect();

    println!(
        r#"This executable runs a raclette test suite which has been engineered
to have {} failures and {} ignored test. If raclette detected these
failures and ignored tests correctly this process returns 0"#,
        NUM_FAILURES, NUM_IGNORED,
    );

    // In this particular test-suite, we expect two infinite loops to be stopped by raclette,
    // in your case, you should probably ensure failed_tasks.len() == 0
    if failed_tasks.len() != NUM_FAILURES {
        std::process::exit(1);
    }
}
