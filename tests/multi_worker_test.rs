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

    // Warm-up: the first placement this process ever makes starts the helper threads
    // (lazy start — see `scheduler::ensure_helpers_started`), a one-time cost that belongs
    // outside the steady-state measurement below, not folded into the baseline.
    timed_round_trip(port);

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

    let serial_estimate = baseline * 4;
    let cores = std::thread::available_parallelism().map_or(0, |n| n.get());
    eprintln!(
        "cpu-bound overlap: baseline {baseline:?}, four concurrent {four_concurrent:?} \
         (serial would be ~{serial_estimate:?}, {cores} cores available)"
    );
    // Bound relative to the serial estimate, not a hardcoded core count: a CI container
    // does not promise four cores, so this only proves the handlers overlap AT ALL — a
    // scheduler with zero parallelism (ratio 4x) still fails it.
    assert!(
        four_concurrent < serial_estimate.mul_f64(0.85),
        "four concurrent CPU-bound handlers took {four_concurrent:?}, not meaningfully \
         under the ~{serial_estimate:?} serial estimate — they do not appear to overlap at \
         all ({cores} cores available)"
    );
}

#[test]
fn four_concurrent_sleeping_handlers_overlap_under_the_naive_sum() {
    const SECONDS: f64 = 0.15;
    let file = temp_ql("sleep_overlap", &sleeping_server_program(SECONDS));
    let (child, port) = spawn_server(&file, None);

    // Warm-up: see the matching comment in `cpu_bound_handlers_overlap_across_more_than_one_worker` —
    // the one-time helper-spawn cost of this process's first placement stays out of `elapsed`.
    timed_round_trip(port);

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

/// A `net.@tcpServe` server whose handler bumps a shared atomic `@hits` by
/// `increments_per_connection`, in a tight native `.each` loop with NO park point inside it
/// at all — pure arithmetic — after a short synchronized pause so every concurrent
/// connection's burst actually overlaps on real, parallel worker threads rather than merely
/// running back to back. `"report"` reads the running total back as text; `"quit"` settles
/// the server.
fn atomic_counter_server_program(increments_per_connection: u64) -> String {
    format!(
        r#"
<< core.net
<< core.io
<< core.time

@server := net.Server {{ handle = 0 }}
@hits := 0

bump = (connection :: net.Connection) -> $ => <
  time.@sleep(0.05)
  (1 <- {increments_per_connection}).each(i => <
    hits := hits + 1
  >)
  connection.@write("ok")
>

report = (connection :: net.Connection) -> $ => <
  connection.@write("`hits`")
>

respond = (connection :: net.Connection) -> $ => <
  line = connection.@read()
  line == "quit" ? server.kill(1) : (line == "report" ? report(connection) : bump(connection))
>

^ = () -> Num => <
  server := net.@tcpServe("127.0.0.1:0", connection => respond(connection))
  io.print(server.address().port)
  0
>
"#
    )
}

#[test]
fn atomic_binding_increments_never_lose_updates_across_workers() {
    // The regression this test guards (PR 492's own body documents it): two handler
    // fibers on two different WORKERS — genuine OS threads, genuine parallelism — both
    // running `hits := hits + 1` at once can race on the non-atomic load/add/store a plain
    // global reassignment used to lower to, losing an update. Several connections, each
    // doing many increments with no park point between them (so a whole burst runs as one
    // uninterrupted stretch on its worker), forced onto `QUILON_WORKERS=4` and started in
    // near lockstep (`time.@sleep(0.05)`), make the race land reliably if the reassignment
    // is not actually atomic across workers.
    const CONNECTIONS: u64 = 8;
    // Large enough that the race (a lost update under real cross-worker contention) is not
    // a matter of luck — main loses more than half of 16,000,000 at this same shape — small
    // enough to leave headroom under the fixed 5s socket read timeout when this file's other
    // tests are also competing for CPU in the same `cargo test` run.
    const PER_CONNECTION: u64 = 500_000;
    let file = temp_ql(
        "atomic_counter",
        &atomic_counter_server_program(PER_CONNECTION),
    );
    let (child, port) = spawn_server(&file, Some("4"));

    let clients: Vec<_> = (0..CONNECTIONS)
        .map(|_| {
            std::thread::spawn(move || {
                let mut stream = connect_with_timeout("127.0.0.1", port).expect("connect to bump");
                stream.write_all(b"bump").expect("write bump");
                let mut reply = [0u8; 2];
                stream.read_exact(&mut reply).expect("read the ok reply");
                assert_eq!(&reply, b"ok");
            })
        })
        .collect();
    for client in clients {
        client.join().expect("client thread panicked");
    }

    let mut report_stream = connect_with_timeout("127.0.0.1", port).expect("connect to report");
    report_stream.write_all(b"report").expect("write report");
    let mut reply = Vec::new();
    report_stream
        .read_to_end(&mut reply)
        .expect("read the report reply");
    let total: f64 = std::str::from_utf8(&reply)
        .expect("report reply is UTF-8")
        .trim()
        .parse()
        .expect("report reply is a number");

    quit(port);
    assert_eq!(
        wait_bounded(child, Duration::from_secs(15)),
        0,
        "the server's own process exits 0 once kill has settled the accept loop"
    );

    assert_eq!(
        total,
        (CONNECTIONS * PER_CONNECTION) as f64,
        "lost at least one update to the shared atomic binding across workers — expected \
         every one of {} increments from {CONNECTIONS} connections to survive",
        CONNECTIONS * PER_CONNECTION
    );
}

/// A `net.@tcpServe` server exercising a REASSIGNMENT whose right side reads a DIFFERENT
/// atomic binding — directly (`pumpA`/`pumpB`: `a := b + 1` / `b := a + 1`) and through an
/// ordinary function call one frame further out (`pumpA2`/`pumpB2`: `a := readB() + 1` /
/// `b := readA() + 1`, `readB`/`readA` each just reading the other binding). This is the
/// shape a PER-BINDING lock scheme deadlocks on: one worker holds `a`'s lock wanting `b`'s
/// while another holds `b`'s wanting `a`'s, at the same instant.
fn cross_binding_server_program(iterations: u64) -> String {
    format!(
        r#"
<< core.net
<< core.io
<< core.time

@server := net.Server {{ handle = 0 }}
@a := 0
@b := 0

readB = () -> Num => < b >
readA = () -> Num => < a >

pumpA = (connection :: net.Connection) -> $ => <
  time.@sleep(0.05)
  (1 <- {iterations}).each(i => <
    a := b + 1
  >)
  connection.@write("ok")
>

pumpB = (connection :: net.Connection) -> $ => <
  time.@sleep(0.05)
  (1 <- {iterations}).each(i => <
    b := a + 1
  >)
  connection.@write("ok")
>

pumpA2 = (connection :: net.Connection) -> $ => <
  time.@sleep(0.05)
  (1 <- {iterations}).each(i => <
    a := readB() + 1
  >)
  connection.@write("ok")
>

pumpB2 = (connection :: net.Connection) -> $ => <
  time.@sleep(0.05)
  (1 <- {iterations}).each(i => <
    b := readA() + 1
  >)
  connection.@write("ok")
>

report = (connection :: net.Connection) -> $ => <
  connection.@write("`a`,`b`")
>

respond = (connection :: net.Connection) -> $ => <
  line = connection.@read()
  line == "quit"
    ? server.kill(1)
    : (line == "pumpA"
      ? pumpA(connection)
      : (line == "pumpB"
        ? pumpB(connection)
        : (line == "pumpA2"
          ? pumpA2(connection)
          : (line == "pumpB2" ? pumpB2(connection) : report(connection)))))
>

^ = () -> Num => <
  server := net.@tcpServe("127.0.0.1:0", connection => respond(connection))
  io.print(server.address().port)
  0
>
"#
    )
}

/// Connect, write `command`, and wait for the fixed `"ok"` reply — one leg of each
/// deadlock test below.
fn pump(port: u16, command: &str) {
    let mut stream = connect_with_timeout("127.0.0.1", port).expect("connect to the test server");
    stream
        .write_all(command.as_bytes())
        .unwrap_or_else(|e| panic!("write {command}: {e}"));
    let mut reply = [0u8; 2];
    stream
        .read_exact(&mut reply)
        .unwrap_or_else(|e| panic!("read the ok reply to {command}: {e}"));
    assert_eq!(&reply, b"ok", "{command} did not reply ok");
}

/// `report`'s `"a,b"` reply, parsed back into the two numbers.
fn report_a_b(port: u16) -> (f64, f64) {
    let mut stream = connect_with_timeout("127.0.0.1", port).expect("connect to report");
    stream.write_all(b"report").expect("write report");
    let mut reply = Vec::new();
    stream
        .read_to_end(&mut reply)
        .expect("read the report reply");
    let text = std::str::from_utf8(&reply).expect("report reply is UTF-8");
    let mut parts = text.split(',');
    let a: f64 = parts
        .next()
        .expect("a present")
        .trim()
        .parse()
        .expect("a is a number");
    let b: f64 = parts
        .next()
        .expect("b present")
        .trim()
        .parse()
        .expect("b is a number");
    (a, b)
}

/// Every single reassignment of `a` or `b` takes the ONE process-wide atomic-binding lock
/// for its whole duration, so the whole run is some serialization of individual steps, each
/// setting one binding to the other's CURRENT value plus one. By induction neither binding
/// ever exceeds the number of steps completed so far, so this bound — generous by 2 — holds
/// regardless of how the two pumps interleave; a torn read/write (the bug a correct lock
/// rules out) is what would blow past it (or produce a non-finite value).
fn assert_well_formed_ping_pong(a: f64, b: f64, iterations: u64) {
    let bound = (2 * iterations + 2) as f64;
    assert!(
        a.is_finite() && a >= 0.0 && a <= bound,
        "`a` ended at an implausible value: {a} (bound {bound})"
    );
    assert!(
        b.is_finite() && b >= 0.0 && b <= bound,
        "`b` ended at an implausible value: {b} (bound {bound})"
    );
}

#[test]
fn cross_reassignment_of_two_atomic_bindings_does_not_deadlock() {
    // The deadlock a PER-BINDING lock scheme risks: worker 1 running `a := b + 1` holds
    // `a`'s lock and wants `b`'s to read it, while worker 2 running `b := a + 1` holds `b`'s
    // and wants `a`'s, at the same instant — classic AB-BA, both spin forever. A hard
    // timeout makes a hang the test failure rather than an indefinite wait: `wait_bounded`
    // kills the child and panics if this takes anywhere near as long as a real deadlock
    // would park every worker spinning.
    const ITERATIONS: u64 = 200_000;
    let file = temp_ql(
        "cross_reassignment",
        &cross_binding_server_program(ITERATIONS),
    );
    let (child, port) = spawn_server(&file, Some("4"));

    let pump_a = std::thread::spawn(move || pump(port, "pumpA"));
    let pump_b = std::thread::spawn(move || pump(port, "pumpB"));
    pump_a.join().expect("pumpA thread panicked");
    pump_b.join().expect("pumpB thread panicked");

    let (a, b) = report_a_b(port);

    quit(port);
    assert_eq!(
        wait_bounded(child, Duration::from_secs(15)),
        0,
        "the server's own process exits 0 — a hang here IS the deadlock this test guards \
         against"
    );
    assert_well_formed_ping_pong(a, b, ITERATIONS);
}

#[test]
fn a_reassignment_reading_a_different_binding_through_a_function_does_not_deadlock() {
    // The same risk, one call frame further out: the right side reads the OTHER binding
    // through an ordinary function call (`readB()`/`readA()`) rather than inline, proving
    // the lock's re-entrancy does not depend on the nested read being syntactically direct
    // in the reassignment's own right-side expression.
    const ITERATIONS: u64 = 200_000;
    let file = temp_ql("nested_read", &cross_binding_server_program(ITERATIONS));
    let (child, port) = spawn_server(&file, Some("4"));

    let pump_a = std::thread::spawn(move || pump(port, "pumpA2"));
    let pump_b = std::thread::spawn(move || pump(port, "pumpB2"));
    pump_a.join().expect("pumpA2 thread panicked");
    pump_b.join().expect("pumpB2 thread panicked");

    let (a, b) = report_a_b(port);

    quit(port);
    assert_eq!(
        wait_bounded(child, Duration::from_secs(15)),
        0,
        "the server's own process exits 0 — a hang here IS the deadlock this test guards \
         against"
    );
    assert_well_formed_ping_pong(a, b, ITERATIONS);
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
