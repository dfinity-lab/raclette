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

fn tests() -> TestTree {
    test_suite(
        "all",
        vec![
            test_suite(
                "arithmetics",
                vec![
                    test_case("addition", || assert_eq!(4, 2 + 2)),
                    test_case("bad math", || assert_eq!(47, 7 * 7)),
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
        ],
    )
}

fn main() {
    default_main(Config::default(), tests());
}
