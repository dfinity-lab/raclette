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

fn tests() -> TestTree {
    test_suite(
        "All",
        vec![
            test_suite(
                "Arithmetics",
                vec![
                    test_case("addition", || assert_eq!(4, 2 + 2)),
                    test_case("bad math", || assert_eq!(47, 7 * 7)),
                    test_suite("Multiplication", mult_table_tests()),
                ],
            ),
            test_case("infinite loop", || loop {
                println!("watching a second passing by...");
                std::thread::sleep(Duration::from_secs(1));
            }),
        ],
    )
}

fn main() {
    default_main(tests())
}
