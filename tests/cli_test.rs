//! End-to-end tests for the `core.cli` module (getEnv / hasFlag / getOpt). Drives the
//! full pipeline (lex -> parse -> link `<< core.cli` -> typecheck -> codegen -> JIT) and
//! asserts the entry point's real exit code. Programs return small (<=255) Nums that
//! bit- or digit-encode the checked values, so a failure pinpoints what diverged.
//!
//! (These use plain `Num` returns rather than `core.test` asserts on purpose: a failing
//! assert calls `__exit`, which would abort the in-process test runner. The self-asserting
//! end-to-end example lives at `examples/cli.ql`, exercised by the examples gate.)

use quilon::jit;
use quilon::lexer::Lexer;
use quilon::modules;
use quilon::parser;
use quilon::typechecker::TypeChecker;
use std::path::Path;
use std::sync::Mutex;

// LLVM JIT / native-target init isn't thread-safe; serialize execution.
static JIT_LOCK: Mutex<()> = Mutex::new(());

/// Lex -> parse -> link imports -> typecheck -> JIT-run `src`, asserting its exit code.
fn assert_exit(src: &str, expected: i32) {
    let _guard = JIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let linked = modules::link(program, Path::new(".")).expect("import linking failed");
    TypeChecker::new()
        .check_program(&linked)
        .expect("type checking failed");
    let code = jit::run_program(&linked, &["program".to_string()]).expect("execution failed");
    assert_eq!(code, expected, "unexpected exit code for source:\n{}", src);
}

// ---- getEnv --------------------------------------------------------------------------

#[test]
fn get_env_found_and_missing() {
    // `A` -> Ok("xyz") (bound value has size 3); `Z` -> NotOk.
    let src = r#"
        << core.cli
        ^ = () -> Num => <
          env :: [][]Text = [["A", "xyz"], ["B", "12"]]
          hit :: Num = getEnv(env, "A") ? | Ok(v) => v.size | NotOk(_) => 99
          miss :: Num = getEnv(env, "Z") ? | Ok(_) => 99 | NotOk(_) => 1
          hit * 10 + miss
        >
    "#;
    assert_exit(src, 31);
}

#[test]
fn get_env_matches_key_not_value() {
    // The lookup keys on pair[0]; a query equal to a VALUE (not a key) must miss.
    let src = r#"
        << core.cli
        ^ = () -> Num => <
          env :: [][]Text = [["KEY", "VAL"]]
          byKey :: Num = getEnv(env, "KEY") ? | Ok(v) => v.size | NotOk(_) => 99
          byVal :: Num = getEnv(env, "VAL") ? | Ok(_) => 99 | NotOk(_) => 1
          byKey * 10 + byVal
        >
    "#;
    // "VAL".size = 3 (byKey), byVal miss = 1 -> 31.
    assert_exit(src, 31);
}

#[test]
fn get_env_empty_value_is_present() {
    // A present key with an empty value is Ok(""), not NotOk.
    let src = r#"
        << core.cli
        ^ = () -> Num => <
          env :: [][]Text = [["E", ""]]
          present :: Num = getEnv(env, "E") ? | Ok(_) => 1 | NotOk(_) => 0
          v :: Text = getEnv(env, "E") ? | Ok(x) => x | NotOk(_) => "?"
          present * 10 + v.size
        >
    "#;
    // present 1, "".size 0 -> 10.
    assert_exit(src, 10);
}

// ---- hasFlag -------------------------------------------------------------------------

#[test]
fn has_flag_with_and_without_dashes() {
    // "--verbose" is matched by "verbose", "--verbose"; "-x" matched literally; a plain
    // "v" is NOT "-v"; "missing" is absent.
    let src = r#"
        << core.cli
        ^ = () -> Num => <
          args :: []Text = ["prog", "--verbose", "-x"]
          a :: Num = hasFlag(args, "verbose") ? 1 : 0
          b :: Num = hasFlag(args, "--verbose") ? 1 : 0
          c :: Num = hasFlag(args, "-x") ? 1 : 0
          d :: Num = hasFlag(args, "missing") ? 1 : 0
          e :: Num = hasFlag(args, "v") ? 1 : 0
          a * 16 + b * 8 + c * 4 + d * 2 + e
        >
    "#;
    // a,b,c = 1; d,e = 0 -> 16 + 8 + 4 = 28.
    assert_exit(src, 28);
}

// ---- getOpt --------------------------------------------------------------------------

#[test]
fn get_opt_collects_space_and_equals_forms() {
    // `--out a` (space) then `--out=bb` (=) collect in argv order: ["a", "bb"].
    let src = r#"
        << core.cli
        ^ = () -> Num => <
          args :: []Text = ["prog", "--out", "a", "--out=bb", "-x"]
          none :: []Text = args.filter(x => false)
          vs :: []Text = getOpt(args, "out") ? | Ok(v) => v | NotOk(_) => none
          vs.size * 100 + vs[0].size * 10 + vs[1].size
        >
    "#;
    // size 2, "a".size 1, "bb".size 2 -> 212.
    assert_exit(src, 212);
}

#[test]
fn get_opt_name_with_or_without_dashes() {
    // Requesting "out" and "--out" behave identically.
    let src = r#"
        << core.cli
        ^ = () -> Num => <
          args :: []Text = ["p", "--out", "zz"]
          none :: []Text = args.filter(x => false)
          va :: []Text = getOpt(args, "out") ? | Ok(v) => v | NotOk(_) => none
          vb :: []Text = getOpt(args, "--out") ? | Ok(v) => v | NotOk(_) => none
          sameSize :: Num = va.size == vb.size ? 1 : 0
          sameVal :: Num = va.size == 1 ? (va[0] == vb[0] ? 1 : 0) : 0
          sameSize * 100 + sameVal * 10 + va.size
        >
    "#;
    // both find "zz": sameSize 1, sameVal 1, va.size 1 -> 100 + 10 + 1 = 111.
    assert_exit(src, 111);
}

#[test]
fn get_opt_equals_form_empty_value() {
    // `--k=` yields a single empty value.
    let src = r#"
        << core.cli
        ^ = () -> Num => <
          args :: []Text = ["p", "--k="]
          none :: []Text = args.filter(x => false)
          vs :: []Text = getOpt(args, "k") ? | Ok(v) => v | NotOk(_) => none
          vs.size * 10 + vs[0].size
        >
    "#;
    // size 1, "".size 0 -> 10.
    assert_exit(src, 10);
}

#[test]
fn get_opt_absent_is_not_ok_with_name() {
    // A name that never appears is NotOk carrying the requested name.
    let src = r#"
        << core.cli
        ^ = () -> Num => <
          args :: []Text = ["p", "--a", "1"]
          name :: Text = getOpt(args, "zzz") ? | Ok(_) => "" | NotOk(n) => n
          name.size
        >
    "#;
    // "zzz".size = 3.
    assert_exit(src, 3);
}

#[test]
fn get_opt_skips_argv0() {
    // The leading "--out" (argv[0]) and its value are ignored; only "real" is collected.
    let src = r#"
        << core.cli
        ^ = () -> Num => <
          args :: []Text = ["--out", "fromArgv0", "--out", "real"]
          none :: []Text = args.filter(x => false)
          vs :: []Text = getOpt(args, "out") ? | Ok(v) => v | NotOk(_) => none
          vs.size * 10 + vs[0].size
        >
    "#;
    // size 1, "real".size 4 -> 14.
    assert_exit(src, 14);
}

#[test]
fn get_opt_trailing_flag_with_no_value_is_not_ok() {
    // A trailing "--out" with no following value collects nothing, so it is NotOk(name).
    let src = r#"
        << core.cli
        ^ = () -> Num => <
          args :: []Text = ["p", "--out"]
          name :: Text = getOpt(args, "out") ? | Ok(_) => "" | NotOk(n) => n
          name.size
        >
    "#;
    // NotOk("out") -> "out".size = 3.
    assert_exit(src, 3);
}

// ---- import wiring -------------------------------------------------------------------

#[test]
fn core_cli_resolves_and_type_checks() {
    // `<< core.cli` is a registered built-in; all three functions must resolve/type-check.
    let src = r#"
        << core.cli
        ^ = (args :: []Text, env :: [][]Text) -> Num => <
          hit :: Num = env |> getEnv("HOME") ? | Ok(_) => 1 | NotOk(_) => 0
          flag :: Num = args |> hasFlag("-v") ? 1 : 0
          opt :: Num = args |> getOpt("--out") ? | Ok(_) => 1 | NotOk(_) => 0
          hit + flag + opt
        >
    "#;
    let tokens = Lexer::tokenize(src).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let linked = modules::link(program, Path::new(".")).expect("import linking failed");
    TypeChecker::new()
        .check_program(&linked)
        .expect("expected core.cli usage to type-check");
}
