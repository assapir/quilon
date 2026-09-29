//! End-to-end proof that the N-worker scheduler actually places work across more than one
//! worker, that a program with several independent launches overlaps them exactly as it
//! always has, and that `QUILON_WORKERS=1` reproduces a single-worker program's behavior
//! unchanged (see `docs/concurrency/runtime.md`).

mod common;

use common::{connect_with_timeout, read_announced_port};
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A `net.@tcpServe` server whose handler burns CPU for a controllable number of iterations
/// with NO park point anywhere in it (`burn` is a plain tail-recursive loop — no `@`
/// primitive at all), then echoes a fixed reply. A handler with no park point can never
/// share progress with a sibling on the SAME worker: cooperative scheduling only ever
/// switches fibers at a park, so two such handlers overlap only if they land on genuinely
/// DIFFERENT workers. Prints the port it bound as the first line of its own stdout.
fn cpu_bound_server_program(iterations: u64) -> String {
    format!(
        r#"
<< core.net
<< core.io

@server := net.Server {{ handle = 0 }}

burn = (n :: Num, acc :: Num) -> Num => < n <= 0 ? acc : burn(n - 1, acc + (n % 7)) >

reply = (connection :: net.Connection) -> $ => <
  connection.@write(burn({iterations}, 0) >= 0 ? "done" : "done")
>

respond = (connection :: net.Connection) -> $ => <
  line = connection.@read()
  line == "quit" ? server.kill(1) : reply(connection)
  $
>

^ = () -> Num => <
  server := net.@tcpServe("127.0.0.1:0", connection => respond(connection))
  io.print(server.address().port)
  0
>
"#
    )
}

/// A `net.@tcpServe` server whose handler pauses (`time.@sleep`, a genuine park point) for
/// `seconds` before echoing — the regression this test guards: several independent launches
/// (here, accepted connections) overlap their pause rather than serializing it, exactly as
/// a single-threaded fiber scheduler already did before workers existed.
fn sleeping_server_program(seconds: f64) -> String {
    format!(
        r#"
<< core.net
<< core.io
<< core.time

@server := net.Server {{ handle = 0 }}

reply = (connection :: net.Connection) -> $ => <
  time.@sleep({seconds})
  connection.@write("done")
>

respond = (connection :: net.Connection) -> $ => <
  line = connection.@read()
  line == "quit" ? server.kill(1) : reply(connection)
  $
>

^ = () -> Num => <
  server := net.@tcpServe("127.0.0.1:0", connection => respond(connection))
  io.print(server.address().port)
  0
>
"#
    )
}

fn temp_ql(tag: &str, source: &str) -> tempfile::NamedTempFile {
    let file = tempfile::Builder::new()
        .prefix(&format!("quilon_multi_worker_{tag}_"))
        .suffix(".qn")
        .tempfile()
        .expect("create temp .qn");
    std::fs::write(file.path(), source).expect("write temp .qn");
    file
}

/// Spawn `quilon run file` with `QUILON_WORKERS` set to `workers` (omitted when `None`,
/// leaving the default CPU-count behavior), returning the child once it has announced its
/// bound port on stdout.
fn spawn_server(file: &tempfile::NamedTempFile, workers: Option<&str>) -> (Child, u16) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_quilon"));
    command
        .args(["run", file.path().to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(workers) = workers {
        command.env("QUILON_WORKERS", workers);
    }
    let mut child = command.spawn().expect("spawn quilon run");
    let port = read_announced_port(&mut child);
    (child, port)
}

fn wait_bounded(mut child: Child, timeout: Duration) -> i32 {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("poll the child process") {
            return status.code().expect("process exited with a code");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the test server never exited within {timeout:?} — killed it");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One request/reply round trip against `port`: connect, write a marker line, read the
/// fixed `"done"` reply, and return how long the whole exchange took.
fn timed_round_trip(port: u16) -> Duration {
    let start = Instant::now();
    let mut stream = connect_with_timeout("127.0.0.1", port).expect("connect to the test server");
    stream.write_all(b"go").expect("write the request");
    let mut reply = [0u8; 4];
    stream.read_exact(&mut reply).expect("read the reply");
    assert_eq!(&reply, b"done", "the handler replied with its fixed marker");
    start.elapsed()
}

fn quit(port: u16) {
    let mut stream = connect_with_timeout("127.0.0.1", port).expect("connect to send quit");
    let _ = stream.write_all(b"quit");
}

#[test]
fn cpu_bound_handlers_overlap_across_more_than_one_worker() {
    // Self-calibrated against THIS machine's own speed rather than a hardcoded duration:
    // one request alone gives the baseline (T1); four concurrent requests give T4. On one
    // worker a compute-bound handler (no park point anywhere in `burn`) can never share
    // progress with a sibling — cooperative scheduling only switches at a park — so T4
    // would be ~4x T1 there. Landing on more than one worker (`QUILON_WORKERS=4`, forced
    // so this does not depend on how many CPUs the machine running it happens to expose)
    // lets them make real, overlapping progress, so T4 comes in well under that.
    const ITERATIONS: u64 = 10_000_000;
    let file = temp_ql("cpu_overlap", &cpu_bound_server_program(ITERATIONS));
    let (child, port) = spawn_server(&file, Some("4"));

    let baseline = timed_round_trip(port);

    let start = Instant::now();
    let clients: Vec<_> = (0..4)
        .map(|_| std::thread::spawn(move || timed_round_trip(port)))
        .collect();
    for client in clients {
        client.join().expect("client thread panicked");
    }
    let four_concurrent = start.elapsed();

    quit(port);
    assert_eq!(
        wait_bounded(child, Duration::from_secs(15)),
        0,
        "the server's own process exits 0 once kill has settled the accept loop"
    );

    eprintln!(
        "cpu-bound overlap: baseline {baseline:?}, four concurrent {four_concurrent:?} \
         (serial would be ~{:?})",
        baseline * 4
    );
    assert!(
        four_concurrent < baseline * 5 / 2,
        "four concurrent CPU-bound handlers took {four_concurrent:?}, not meaningfully \
         under four times the {baseline:?} baseline — they do not appear to have landed on \
         more than one worker"
    );
}

#[test]
fn four_concurrent_sleeping_handlers_overlap_under_the_naive_sum() {
    const SECONDS: f64 = 0.15;
    let file = temp_ql("sleep_overlap", &sleeping_server_program(SECONDS));
    let (child, port) = spawn_server(&file, None);

    let start = Instant::now();
    let clients: Vec<_> = (0..4)
        .map(|_| std::thread::spawn(move || timed_round_trip(port)))
        .collect();
    for client in clients {
        client.join().expect("client thread panicked");
    }
    let elapsed = start.elapsed();

    quit(port);
    assert_eq!(
        wait_bounded(child, Duration::from_secs(15)),
        0,
        "the server's own process exits 0 once kill has settled the accept loop"
    );

    eprintln!("four concurrent {SECONDS}s sleeps: {elapsed:?} (naive sum would be 0.6s)");
    assert!(
        elapsed < Duration::from_millis((SECONDS * 1000.0) as u64 * 3),
        "four concurrent sleeping handlers took {elapsed:?}, not well under their naive \
         4x{SECONDS}s sum — independent launches no longer overlap"
    );
}

#[test]
fn quilon_workers_one_reproduces_the_default_programs_behavior() {
    // A program with no launches at all (`examples/sleep.qn`'s own self-assertions do use
    // `time.@sleep`, exercised on the seed fiber only — no placement decision is ever made)
    // must behave identically whether it runs on the default worker count or is forced down
    // to exactly one — `QUILON_WORKERS=1` is a debugging aid for exactly this comparison.
    let source = std::fs::read_to_string("examples/sleep.qn").expect("read examples/sleep.qn");
    let file = temp_ql("workers_one", &source);

    let default_run = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["run", file.path().to_str().unwrap()])
        .output()
        .expect("run with the default worker count");
    let single_worker_run = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["run", file.path().to_str().unwrap()])
        .env("QUILON_WORKERS", "1")
        .output()
        .expect("run with QUILON_WORKERS=1");

    assert_eq!(
        default_run.status.code(),
        Some(0),
        "the default run must pass its own self-assertions"
    );
    assert_eq!(
        single_worker_run.status.code(),
        default_run.status.code(),
        "QUILON_WORKERS=1 must reproduce the default run's exit code"
    );
    assert_eq!(
        single_worker_run.stdout, default_run.stdout,
        "QUILON_WORKERS=1 must reproduce the default run's stdout"
    );
}
