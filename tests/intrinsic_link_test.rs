//! Every runtime intrinsic must survive into a linked executable.
//!
//! The failure this exists for: an intrinsic that nothing in Rust calls can be dropped
//! from `libquilon_rt.a`, and the program that needs it then fails to link — or, when the
//! JIT is the one resolving names, calls a null address and segfaults with no diagnostic.
//! It has happened more than once, and it happens *nondeterministically*: the same commit
//! links on one runner and not on another, so a coin-flip decides whether it ships.
//!
//! The program below calls into every intrinsic the runtime exports, so linking it is a
//! decision about all of them at once. A dropped symbol is an undefined reference here —
//! a red test on every run, rather than a red job on some runs.
//!
//! It also runs the result under both linkers, because the retention story differs
//! between them: what `clang` pulls out of an archive and what `gcc` pulls are separately
//! observed behaviours, and the project supports both.

#[cfg(not(target_os = "macos"))]
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Is a tool available on PATH? (Mirrors the other AOT tests, which skip gracefully when
/// no C toolchain is installed.)
fn tool_available(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// A program that reaches every symbol in `quilon_rt::INTRINSICS`.
///
/// Some are reached by using the feature they back (`.split` → `__text_split`); others
/// come with the shape of the program — the entry point's `args`/`env` conversions, the
/// GC initializer `main` emits, the bounds-check failure path every index carries, and
/// the exit primitive behind a failed assertion. Interpolation is what pulls in the
/// number and boolean renderers.
const EVERY_INTRINSIC: &str = r#"
<< core.io
<< core.test
<< core.time
<< core.net

^ = (args :: []Text, env :: [|Text => Text|]) -> $ => <
  ~ __sleep (the @sleep leaf primitive) and __run_fiber_main (the entry runs on a
  ~ scheduler fiber because an @ primitive is used), and __now (the plain clock read).
  @sleep(0)
  assert(now() >= 0)

  ~ __read_launch (the @readStdin leaf primitive) and __force_text (the `.length` reads the
  ~ deferred Text's bytes, forcing it). Run with empty stdin here, so @readStdin yields "".
  line = @readStdin()
  assert(line.length >= 0)

  ~ __tcp_request_launch (the internal @tcpRequest socket primitive) and __force_result (the
  ~ `?` match FORCES the deferred Result). Guarded by a runtime-false condition so codegen
  ~ EMITS both calls (the link/JIT gate sees the symbols) but it never opens a real connection.
  reached = args.size > 1000000
    ? @tcpRequest("127.0.0.1:1", "") ? | Ok(_) => true | NotOk(_) => true
    : true
  assert(reached)

  ~ __argv_to_text_array / __envp_to_map come from these parameters existing.
  assert(args.size >= 1)
  assert(env.size >= 0)

  ~ __alloc, and __index_fail from the checked index.
  xs :: []Num = [10, 20, 30]
  assertEq(xs[1], 20)

  ~ __num_to_text and __bool_to_text, via interpolation.
  rendered = "n `xs[0]` ok `xs.size == 3`"
  assert(rendered.size > 0)

  ~ __print_text_fd and __write_bytes.
  print("linked")
  written = "bytes" |> write(stdout)
  assert(written == 5)

  ~ __text_cmp and __text_length.
  assert("abc" < "abd")
  assertEq("héllo".length, 5)

  ~ The Text methods, one call each.
  assertEq("  pad  ".trimStart(), "pad  ")
  assertEq("  pad  ".trimEnd(), "  pad")
  assertEq("up".toUpper(), "UP")
  assertEq("DOWN".toLower(), "down")
  assert("haystack".contains("stack"))
  assertOk("haystack".indexOf("stack"))
  assertEq("a-a".replaceAll("a", "b"), "b-b")
  assertEq("a-a".replace("a", "b", 1), "b-a")
  assertEq("slice".slice(1, 3), "li")
  assertEq("x,y".split(",").size, 2)

  ~ Map intrinsics: new/set/get/has/len, and keys/values/each iteration.
  counts :: [|Text => Num|] = [|"a" => 1, "b" => 2|]
  assert(counts.has("b"))
  assertEq(counts.size, 2)
  grown :: [|Text => Num|] = counts.set("c", 3)
  assertEq(grown.keys().size, 3)
  assertEq(grown.values().size, 3)
  assertEq(grown.remove("c").size, 2)
  assertOk(counts.get("a"))
  counts.each((k, v) => v)

  ~ Set intrinsics: new/add/has/len, items/each iteration, and the algebra operators.
  odds :: [|Num|] = [|1, 3, 5|]
  evens :: [|Num|] = [|3, 4, 5|]
  odds.each(x => x)
  assert(odds.has(1))
  assertEq(odds.add(7).size, 4)
  assertEq(odds.remove(1).size, 2)
  assertEq(odds.items().size, 3)
  assertEq((odds + evens).size, 4)
  assertEq((odds - evens).size, 1)
  assertEq((odds +- evens).size, 2)
>
"#;

/// Build `EVERY_INTRINSIC` with `linker` and run it, returning the exit code. A link
/// failure is reported with the linker's own diagnostic, since "undefined reference to
/// __something" is the whole point of this test.
fn build_and_run(quilon: &Path, linker: &str, dir: &Path) -> i32 {
    let source = dir.join(format!("every_intrinsic_{linker}.qn"));
    std::fs::write(&source, EVERY_INTRINSIC).expect("writing the test program");
    let out = dir.join(format!("every_intrinsic_{linker}"));

    let build = Command::new(quilon)
        .arg("build")
        .arg(&source)
        .args(["-o".as_ref(), out.as_os_str()])
        .args(["--linker", linker])
        .output()
        .expect("running quilon build");
    assert!(
        build.status.success(),
        "linking with {linker} failed — an intrinsic is missing from libquilon_rt.a:\n{}\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(&out)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("running the built program");
    assert!(
        run.status.success(),
        "the linked program failed at run time (linker={linker}):\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    run.status.code().unwrap_or(-1)
}

#[test]
fn every_intrinsic_survives_the_aot_link() {
    let quilon = PathBuf::from(env!("CARGO_BIN_EXE_quilon"));
    let dir = std::env::temp_dir().join(format!("quilon-intrinsic-link-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("creating the work directory");

    let mut linked_with = Vec::new();
    for linker in ["clang", "gcc"] {
        if !tool_available(linker) {
            eprintln!("skipping the {linker} link: not on PATH");
            continue;
        }
        assert_eq!(
            build_and_run(&quilon, linker, &dir),
            0,
            "the every-intrinsic program must exit 0 (linker={linker})"
        );
        linked_with.push(linker);
    }

    assert!(
        !linked_with.is_empty(),
        "no C toolchain on PATH, so nothing was linked — this gate needs clang or gcc"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The same program under the JIT, where a missing intrinsic is a call to a null address
/// rather than a link error. Cheap, and it covers the other half of the resolution story.
#[test]
fn every_intrinsic_resolves_under_the_jit() {
    let quilon = PathBuf::from(env!("CARGO_BIN_EXE_quilon"));
    let dir = std::env::temp_dir().join(format!("quilon-intrinsic-jit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("creating the work directory");
    let source = dir.join("every_intrinsic.qn");
    std::fs::write(&source, EVERY_INTRINSIC).expect("writing the test program");

    let run = Command::new(&quilon)
        .arg("run")
        .arg(&source)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("running quilon run");
    assert!(
        run.status.success(),
        "the JIT could not resolve every intrinsic:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The program above is only a gate on every intrinsic for as long as it actually
/// reaches them all. Emit its IR and check every exported symbol appears, so a program
/// that quietly stops covering one fails here instead of leaving a hole in the gate.
#[test]
fn the_smoke_program_reaches_every_intrinsic() {
    let quilon = PathBuf::from(env!("CARGO_BIN_EXE_quilon"));
    let dir = std::env::temp_dir().join(format!("quilon-intrinsic-cover-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("creating the work directory");
    let source = dir.join("every_intrinsic.qn");
    std::fs::write(&source, EVERY_INTRINSIC).expect("writing the test program");

    let compile = Command::new(&quilon)
        .arg("compile")
        .arg(&source)
        .output()
        .expect("running quilon compile");
    assert!(
        compile.status.success(),
        "compiling the every-intrinsic program failed:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let ir = std::fs::read_to_string(source.with_extension("ll")).expect("reading the emitted IR");
    let unreached: Vec<&str> = quilon_rt::INTRINSICS
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| !ir.contains(&format!("@{name}")))
        .collect();
    assert!(
        unreached.is_empty(),
        "the smoke program no longer reaches {unreached:?}, so the link gate would not \
         notice those being dropped — extend the program to use them again"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A program that reaches almost nothing: whatever it links beyond these few intrinsics was
/// retained on purpose, not because the program asked for it.
#[cfg(not(target_os = "macos"))]
const BARELY_ANY_INTRINSIC: &str = r#"
<< core.io

^ = () -> $ => <
  print("linked")
>
"#;

/// The symbols a binary or archive DEFINES, as `nm` reports them. `None` when `nm` cannot
/// read the file at all, so a caller can skip rather than fail.
#[cfg(not(target_os = "macos"))]
fn defined_symbols(path: &Path) -> Option<HashSet<String>> {
    let listing = Command::new("nm")
        .arg("--defined-only")
        .arg(path)
        .output()
        .ok()?;
    if !listing.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&listing.stdout)
            .lines()
            .filter_map(|line| line.split_whitespace().last())
            .map(str::to_string)
            .collect(),
    )
}

/// The `libquilon_rt.a` the compiler under test links, resolved the way `quilon build`
/// resolves it in a test run: the `QUILON_RT_LIB` override, else the copy the cargo build
/// script leaves beside the binary.
#[cfg(not(target_os = "macos"))]
fn runtime_archive(quilon: &Path) -> Option<PathBuf> {
    let candidate = match std::env::var("QUILON_RT_LIB") {
        Ok(over) if !over.is_empty() => PathBuf::from(over),
        _ => quilon.parent()?.join("libquilon_rt.a"),
    };
    candidate.exists().then_some(candidate)
}

/// Which intrinsics an archive scan drops when only `__gc_init` is asked for — the set the
/// AOT link has to force back in. Built by linking a C stub against the real archive with
/// no `-u` flags at all, so it measures this linker on this archive rather than assuming.
#[cfg(not(target_os = "macos"))]
fn intrinsics_a_plain_scan_drops(linker: &str, archive: &Path, dir: &Path) -> HashSet<String> {
    let stub = dir.join(format!("plain_scan_{linker}.c"));
    std::fs::write(
        &stub,
        "extern void __gc_init(void);\nint main(void) { __gc_init(); return 0; }\n",
    )
    .expect("writing the control stub");
    let control = dir.join(format!("plain_scan_{linker}"));

    let link = Command::new(linker)
        .arg(&stub)
        .arg(archive)
        .args(["-lpthread", "-ldl", "-lm"])
        .arg("-o")
        .arg(&control)
        .output()
        .expect("linking the control stub");
    assert!(
        link.status.success(),
        "the control stub must link against libquilon_rt.a (linker={linker}):\n{}",
        String::from_utf8_lossy(&link.stderr)
    );

    let retained = defined_symbols(&control).unwrap_or_default();
    quilon_rt::INTRINSICS
        .iter()
        .map(|(name, _)| (*name).to_string())
        .filter(|name| !retained.contains(name))
        .collect()
}

/// The `-u` mechanism itself, not the archive's contents.
///
/// The every-intrinsic program above references every symbol from its own object, so the
/// `-u <intrinsic>` flags `quilon build` passes on the GNU-ld path are redundant there: it
/// would link just as well if the compiler emitted none of them. This builds a program that
/// references almost nothing and asserts the intrinsics it never mentions are in the binary
/// anyway — which only the forced undefined references can explain.
///
/// A C stub linked against the same archive with no `-u` establishes what an on-demand scan
/// leaves behind, so the assertion is about symbols this linker demonstrably drops without
/// help. ld64 is excluded: it takes `-force_load` on the whole archive instead, and spells
/// symbols with a leading underscore.
#[cfg(not(target_os = "macos"))]
#[test]
fn unreferenced_intrinsics_are_forced_into_the_binary() {
    let quilon = PathBuf::from(env!("CARGO_BIN_EXE_quilon"));
    if !tool_available("nm") {
        eprintln!("skipping: nm is not on PATH, so the binary's symbols cannot be read");
        return;
    }
    let Some(archive) = runtime_archive(&quilon) else {
        eprintln!("skipping: no libquilon_rt.a to link the control against");
        return;
    };

    let dir = std::env::temp_dir().join(format!("quilon-intrinsic-forced-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("creating the work directory");
    let source = dir.join("barely_any_intrinsic.qn");
    std::fs::write(&source, BARELY_ANY_INTRINSIC).expect("writing the test program");

    // What the program actually reaches, read off its IR — the rest is what has to be forced.
    let compile = Command::new(&quilon)
        .arg("compile")
        .arg(&source)
        .output()
        .expect("running quilon compile");
    assert!(
        compile.status.success(),
        "compiling the minimal program failed:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let ir = std::fs::read_to_string(source.with_extension("ll")).expect("reading the emitted IR");

    let mut checked = Vec::new();
    for linker in ["clang", "gcc"] {
        if !tool_available(linker) {
            eprintln!("skipping the {linker} link: not on PATH");
            continue;
        }

        let droppable = intrinsics_a_plain_scan_drops(linker, &archive, &dir);
        let mut must_be_forced: Vec<&String> = droppable
            .iter()
            .filter(|name| !ir.contains(&format!("@{name}")))
            .collect();
        must_be_forced.sort();
        assert!(
            !must_be_forced.is_empty(),
            "{linker} retained every intrinsic from a plain archive scan, so this test cannot \
             observe the -u mechanism — the control stub or the archive layout needs revisiting"
        );

        let out = dir.join(format!("barely_any_intrinsic_{linker}"));
        let build = Command::new(&quilon)
            .arg("build")
            .arg(&source)
            .args(["-o".as_ref(), out.as_os_str()])
            .args(["--linker", linker])
            .output()
            .expect("running quilon build");
        assert!(
            build.status.success(),
            "linking the minimal program with {linker} failed:\n{}\n{}",
            String::from_utf8_lossy(&build.stdout),
            String::from_utf8_lossy(&build.stderr)
        );

        let linked = defined_symbols(&out).expect("reading the built binary's symbols with nm");
        let dropped: Vec<&&String> = must_be_forced
            .iter()
            .filter(|name| !linked.contains(**name))
            .collect();
        assert!(
            dropped.is_empty(),
            "{dropped:?} are missing from a binary built with {linker}, though the program never \
             references them — the per-intrinsic `-u` flags are no longer forcing them in"
        );
        checked.push(linker);
    }

    assert!(
        !checked.is_empty(),
        "no C toolchain on PATH, so nothing was linked — this gate needs clang or gcc"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
