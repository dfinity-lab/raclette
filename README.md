![presubmit](https://github.com/dfinity-lab/raclette/workflows/presubmit/badge.svg?branch=master)

# Raclette — tasty integration tests

Raclette is an alternative Rust test framework for integration tests.

The two main principles of Raclette's design:

  1. Tests are first-class objects, i.e., it's possible to manipulate them programmatically.

  2. Each test is executed in a separate process.

## Pros

  * Programmable test suites.

    Being able to manipulate tests programmatically allows you to get more value from your test framework without much effort.
    For example, parameterized tests (same test running multiple times with different combinations of parameters/types) can be easily implemented as a function producing a list of tests to run.
    No need to learn new syntax and rely on obscure macros.
    It's simply Rust all the way down.

  * No cascading test failures.

    If a single test fails, it is far less likely to affect the outcome of other tests.
    This means tests are free to modify global feature flags, and a single poisoned lock won't make your full test suite red.

  * Faithful stdout/stderr capture.

    libtest shipped with Rust by default captures stdout and stderr produced by each test.
    However, the capture is only set for the main thread that runs your test.
    If the code being tested spawns more threads (which is not uncommon in integration tests), your test output might be littered with unwanted logs.
    Running each test in a separate process allows Raclette to fully capture stdout/stderr of your tests.

  * Safe test timeouts.

    Running each test in a separate process allows the framework to safely kill code that runs for too long.

## Cons

  * This is not what most Rust programmers are familiar with.

  * No IDE integration.

  * Defining Raclette tests requires a bit more configuration.

## Wait, what's wrong with libtest again?

Libtest is great for simple unit tests but starts to show its limits when one tries to test complex systems with it:

  * The test driver might crash on you with no clues on which test causes the troubles:

        test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
        error: test failed, to rerun pass '-p my-awesome-package --test tests'
        Caused by:
          process didn't exit successfully: `tests-75f270a85cc7dc45` (signal: 11, SIGSEGV: invalid memory reference)

  * A failure of a single test can cause subsequent tests to fail as well, which complicates the search of the real cause:

        thread 'worker:13' panicked at 'The RwLock is poisoned due to a writer panic.: "PoisonError { inner: .. }"', src/lib.rs:301:5

  * The stdout/stderr capture doesn't work properly for test that spawn extra threads.
    This might lead to a garbled output and complicate debugging.

        test test::single_threaded ... ok
        Hello from a background thread!
        Look ma, cargo test can't capture me!

        test test::multi_threaded ... ok
