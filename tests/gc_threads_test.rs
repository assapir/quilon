//! Running Quilon programs from more than one thread must not abort the process.
//!
//! The collector stops the world by signalling the threads it knows about, and it knows
//! the thread that initialized it plus any that registered. Nothing registered before, so
//! a program run from a second thread eventually met a collection that tried to stop a
//! thread the collector could not signal, and libgc aborted — reported as a flaky
//! `SIGABRT` in the suite, and reproducible here.
//!
//! What makes it worth a test of its own: it is not a concurrency bug. The two runs below
//! are strictly serialized, one thread at a time. All that matters is that the threads are
//! *different*, which is why locking the JIT (as the suite does) never fixed it.

mod common;
use common::JIT_LOCK;

use quilon::jit;
use quilon::lexer::Lexer;
use quilon::parser;
use quilon::typechecker::TypeChecker;
use std::sync::{Arc, Barrier, mpsc};
use std::thread;

/// Allocates enough to force several collections.
const ALLOCATING_PROGRAM: &str = r#"
churn = (n :: Num, acc :: Num) -> Num => n == 0 ? acc : churn(n - 1, acc + [n, n + 1, n + 2].size)
^ = () -> Num => churn(200000, 0) > 0 ? 0 : 1
"#;

fn run_allocating_program() {
    let _guard = JIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tokens = Lexer::tokenize(ALLOCATING_PROGRAM).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let types = TypeChecker::new()
        .check_program(&program)
        .expect("type checking failed");
    let code = jit::run_program(&program, types, &["program".to_string()]).expect("execution");
    assert_eq!(code, 0, "the allocating program should exit 0");
}

#[test]
fn programs_run_from_several_threads_in_turn() {
    // The barrier holds both workers until both exist, so neither has initialized the
    // collector yet — the situation a test harness creates, and the one that aborted.
    // The channel then gives them one turn each.
    let both_alive = Arc::new(Barrier::new(2));
    let (first_done, second_may_start) = mpsc::channel();

    let gate = Arc::clone(&both_alive);
    let first = thread::spawn(move || {
        gate.wait();
        run_allocating_program();
        first_done.send(()).unwrap();
    });
    let second = thread::spawn(move || {
        both_alive.wait();
        second_may_start.recv().unwrap();
        run_allocating_program();
    });

    first
        .join()
        .expect("the first worker must not abort the process");
    second
        .join()
        .expect("the second worker must not abort the process");
}
