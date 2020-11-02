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

  * Defining Raclette tests requires a bit more configuration.
