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

/// The linkers to exercise: `clang` and `gcc` when present, since what each pulls out of an
/// archive is a separately observed behaviour and the project supports both.
fn linkers_on_path() -> Vec<&'static str> {
    let mut found = Vec::new();
    for linker in ["clang", "gcc"] {
        if tool_available(linker) {
            found.push(linker);
        } else {
            eprintln!("skipping the {linker} link: not on PATH");
        }
    }
    assert!(
        !found.is_empty(),
        "no C toolchain on PATH, so nothing was linked — this gate needs clang or gcc"
    );
    found
}

/// `quilon build source -o out --linker linker`, asserting it linked. A failure is reported
/// with the linker's own diagnostic, since "undefined reference to __something" is what these
/// tests are watching for.
fn build_with(quilon: &Path, source: &Path, out: &Path, linker: &str) {
    let build = Command::new(quilon)
        .arg("build")
        .arg(source)
        .args(["-o".as_ref(), out.as_os_str()])
        .args(["--linker", linker])
        .output()
        .expect("running quilon build");
    assert!(
        build.status.success(),
        "linking {} with {linker} failed — an intrinsic is missing from libquilon_rt.a:\n{}\n{}",
        source.display(),
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
}

/// The intrinsics an emitted module reaches: every `INTRINSICS` name the IR declares or calls.
/// The trailing `(` keeps one name from masking another it is a prefix of.
fn intrinsics_reached_by(ir: &str) -> Vec<&'static str> {
    quilon_rt::INTRINSICS
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| ir.contains(&format!("@{name}(")))
        .collect()
}

/// Build `EVERY_INTRINSIC` with `linker` and run it, returning the exit code.
fn build_and_run(quilon: &Path, linker: &str, dir: &Path) -> i32 {
    let source = dir.join(format!("every_intrinsic_{linker}.qn"));
    std::fs::write(&source, EVERY_INTRINSIC).expect("writing the test program");
    let out = dir.join(format!("every_intrinsic_{linker}"));
    build_with(quilon, &source, &out, linker);

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

    for linker in linkers_on_path() {
        assert_eq!(
            build_and_run(&quilon, linker, &dir),
            0,
            "the every-intrinsic program must exit 0 (linker={linker})"
        );
    }
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
    let reached = intrinsics_reached_by(&ir);
    let unreached: Vec<&str> = quilon_rt::INTRINSICS
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| !reached.contains(name))
        .collect();
    assert!(
        unreached.is_empty(),
        "the smoke program no longer reaches {unreached:?}, so the link gate would not \
         notice those being dropped — extend the program to use them again"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The `-u` mechanism itself, rather than the archive's contents.
///
/// The every-intrinsic program above references every symbol from its own object, so the
/// `-u <intrinsic>` flags `quilon build` passes on the GNU-ld path are redundant there: it
/// would link just as well if the compiler emitted none of them. These tests build a program
/// that references almost nothing and assert the intrinsics it never mentions are in the
/// binary anyway — which only the forced undefined references can explain.
///
/// ld64 is excluded: it takes `-force_load` on the whole archive instead, and spells symbols
/// with a leading underscore.
#[cfg(not(target_os = "macos"))]
mod forced_undefined_symbols {
    use super::*;
    use std::collections::HashSet;

    /// A program that reaches almost nothing, so nearly every intrinsic in the binary it
    /// links is there because the link forced it, not because the program asked.
    const BARELY_ANY_INTRINSIC: &str = r#"
<< core.io

^ = () -> $ => <
  print("linked")
>
"#;

    /// The symbols a binary DEFINES, as `nm` reports them. Callers check that `nm` is on
    /// PATH first, so failing here is a broken tool rather than a missing one.
    fn defined_symbols(path: &Path) -> HashSet<String> {
        let listing = Command::new("nm")
            .arg("--defined-only")
            .arg(path)
            .output()
            .unwrap_or_else(|e| panic!("running nm on {}: {e}", path.display()));
        assert!(
            listing.status.success(),
            "nm could not read {}:\n{}",
            path.display(),
            String::from_utf8_lossy(&listing.stderr)
        );
        String::from_utf8_lossy(&listing.stdout)
            .lines()
            .filter_map(|line| line.split_whitespace().last())
            .map(str::to_string)
            .collect()
    }

    /// The `libquilon_rt.a` `quilon build` will link: the runtime `QUILON_RT_LIB` override
    /// when the environment carries one, else the copy the cargo build script places beside
    /// the binary and bakes as that same variable.
    fn runtime_archive() -> Option<PathBuf> {
        let archive = std::env::var("QUILON_RT_LIB")
            .ok()
            .filter(|value| !value.is_empty())
            .or_else(|| option_env!("QUILON_RT_LIB").map(str::to_string))
            .map(PathBuf::from)?;
        archive.exists().then_some(archive)
    }

    /// Which intrinsics an archive scan drops when asked for exactly what the program under
    /// test asks for, and nothing else.
    ///
    /// A C stub declares `rooted` — the intrinsics the program's own IR reaches — and takes
    /// their addresses, so linking it against the real archive with no `-u` at all reproduces
    /// the program's demand on this archive with this linker. Whatever is missing from the
    /// result is exactly what `-u` has to force back in. `None` when the stub does not link,
    /// which is a broken control rather than a product failure.
    fn intrinsics_a_plain_scan_drops(
        linker: &str,
        archive: &Path,
        dir: &Path,
        rooted: &[&'static str],
    ) -> Option<Vec<&'static str>> {
        let declarations: String = rooted
            .iter()
            .map(|name| format!("extern void {name}(void);\n"))
            .collect();
        let addresses: String = rooted
            .iter()
            .map(|name| format!("    (void *){name},\n"))
            .collect();
        let stub = dir.join(format!("plain_scan_{linker}.c"));
        std::fs::write(
            &stub,
            format!("{declarations}void *const roots[] = {{\n{addresses}}};\nint main(void) {{ return 0; }}\n"),
        )
        .expect("writing the control stub");
        let control = dir.join(format!("plain_scan_{linker}"));

        let link = Command::new(linker)
            .arg(&stub)
            .arg(archive)
            .args(quilon::build::SYSTEM_LIBS)
            .arg("-o")
            .arg(&control)
            .output()
            .expect("linking the control stub");
        if !link.status.success() {
            eprintln!(
                "skipping the {linker} check: the control stub did not link against \
                 libquilon_rt.a, so what a plain scan drops cannot be measured:\n{}",
                String::from_utf8_lossy(&link.stderr)
            );
            return None;
        }

        let retained = defined_symbols(&control);
        let mut dropped: Vec<&'static str> = quilon_rt::INTRINSICS
            .iter()
            .map(|(name, _)| *name)
            .filter(|name| !retained.contains(*name))
            .collect();
        dropped.sort_unstable();
        Some(dropped)
    }

    #[test]
    fn unreferenced_intrinsics_are_forced_into_the_binary() {
        let quilon = PathBuf::from(env!("CARGO_BIN_EXE_quilon"));
        if !tool_available("nm") {
            eprintln!("skipping: nm is not on PATH, so the binary's symbols cannot be read");
            return;
        }
        let Some(archive) = runtime_archive() else {
            eprintln!("skipping: no libquilon_rt.a to link the control against");
            return;
        };

        let dir =
            std::env::temp_dir().join(format!("quilon-intrinsic-forced-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("creating the work directory");
        let source = dir.join("barely_any_intrinsic.qn");
        std::fs::write(&source, BARELY_ANY_INTRINSIC).expect("writing the test program");

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
        let ir =
            std::fs::read_to_string(source.with_extension("ll")).expect("reading the emitted IR");
        let reached = intrinsics_reached_by(&ir);

        for linker in linkers_on_path() {
            let Some(must_be_forced) =
                intrinsics_a_plain_scan_drops(linker, &archive, &dir, &reached)
            else {
                continue;
            };
            assert!(
                !must_be_forced.is_empty(),
                "{linker} retained every intrinsic from a plain scan of libquilon_rt.a, so \
                 nothing here exercises the -u flags — the archive's layout changed and this \
                 test needs revisiting"
            );

            let out = dir.join(format!("barely_any_intrinsic_{linker}"));
            build_with(&quilon, &source, &out, linker);
            let linked = defined_symbols(&out);
            let dropped: Vec<&&str> = must_be_forced
                .iter()
                .filter(|name| !linked.contains(**name))
                .collect();
            assert!(
                dropped.is_empty(),
                "{dropped:?} are missing from a binary built with {linker}, though a plain \
                 archive scan drops them and the program never references them — the \
                 per-intrinsic `-u` flags are no longer forcing them in"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
