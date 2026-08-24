//! DWARF line-number debug-info test for `quilon build --debug`.
//!
//! Builds a small `.qn` program with `--debug` and shells out to `llvm-dwarfdump` to
//! assert the emitted binary carries a DWARF compile unit that references the `.qn`
//! source: a `.debug_line` file table naming the `.qn` file, and a `.debug_info`
//! subprogram for the user's function declaration-lined at its source line. Skips gracefully
//! when the C toolchain or `llvm-dwarfdump` is unavailable (mirrors the native-AOT tests).

use inkwell::context::Context;
use quilon::codegen::CodeGenerator;
use quilon::driver::front_end;
use std::path::Path;
use std::process::Command;

mod common;
use common::ensure_runtime_lib;

/// Is a tool available on PATH (responds to `--version`)?
fn tool_available(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// A program that uses an `@` primitive runs its entry on a scheduler fiber: `main` calls
/// `__run_fiber_main` on a separate `__ql_entry` thunk. Under `--debug` that call must carry
/// a `!dbg` scope in `main`'s own subprogram, not `__ql_entry`'s — LLVM's verifier rejects an
/// instruction whose debug scope is a different function than the one it lives in. This drives
/// the codegen path (which verifies the module) with debug info on directly, so it catches the
/// verifier failure without needing a linker or `llvm-dwarfdump` — the gap the linker-gated
/// tests below left open.
#[test]
fn debug_codegen_verifies_module_for_a_deferral_program() {
    let src = "\
<< core.time

^ = () -> Num => <
  @sleep(0)
  0
>
";
    let dir = std::env::temp_dir().join(format!("quilon_dbgdefer_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let ql = dir.join("defer.qn");
    std::fs::write(&ql, src).expect("write temp source");

    let checked = front_end(&ql).unwrap_or_else(|e| panic!("front end failed: {e}"));
    assert!(
        checked.defer.uses_deferral,
        "the `@sleep` program should use deferral (be the wrapped-entry codegen path)"
    );

    let context = Context::create();
    let mut generator = CodeGenerator::new(&context, "main");
    generator.set_type_table(checked.types);
    generator.set_defer_info(checked.defer);
    generator.enable_debug(&ql, checked.sources.root_text(), checked.imported_items);
    generator.set_source_map(checked.sources);

    // `generate` runs `module.verify()` internally, so an Err here IS the verifier failure.
    generator
        .generate(&checked.program)
        .unwrap_or_else(|e| panic!("debug codegen of a deferral program failed to verify: {e}"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn debug_build_emits_dwarf_line_info_for_the_ql_source() {
    let quilon = env!("CARGO_BIN_EXE_quilon");

    // Need a linker to produce the binary and `llvm-dwarfdump` to inspect it.
    let Some(linker) = ["clang", "gcc"].into_iter().find(|t| tool_available(t)) else {
        eprintln!("skipping debug-info test: need a linker (`clang` or `gcc`) on PATH");
        return;
    };
    if !tool_available("llvm-dwarfdump") {
        eprintln!("skipping debug-info test: `llvm-dwarfdump` not on PATH");
        return;
    }
    ensure_runtime_lib(Path::new(quilon).parent().expect("binary has a parent dir"));

    // A single-file program (no imports) so every emitted function maps to THIS file.
    // `factorial` is on line 2; the entry point `^` is on line 3.
    let src = "\nfactorial = (n :: Num) -> Num => n <= 1 ? 1 : n * factorial(n - 1)\n^ = () -> Num => factorial(5)\n";
    let dir = std::env::temp_dir().join(format!("quilon_dbg_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let ql = dir.join("prog.qn");
    std::fs::write(&ql, src).expect("write temp source");
    let bin = dir.join("prog");

    let build = Command::new(quilon)
        .args(["build", ql.to_str().unwrap()])
        .args(["--linker", linker])
        .args(["--debug", "-o", bin.to_str().unwrap()])
        .output()
        .expect("run quilon build --debug");
    assert!(
        build.status.success(),
        "`quilon build --debug --linker {linker}` failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    // The program's exit code is `factorial(5)` == 120 — debug info must not change behavior.
    let run = Command::new(&bin).status().expect("run built binary");
    assert_eq!(
        run.code(),
        Some(120),
        "debug build changed program behavior"
    );

    // `.debug_line`: the line-number program must name the `.qn` source file.
    let line = Command::new("llvm-dwarfdump")
        .arg("--debug-line")
        .arg(&bin)
        .output()
        .expect("run llvm-dwarfdump --debug-line");
    assert!(line.status.success(), "llvm-dwarfdump --debug-line failed");
    let line_out = String::from_utf8_lossy(&line.stdout);
    assert!(
        line_out.contains("prog.qn"),
        "expected the `.qn` file in the DWARF line table, got:\n{line_out}"
    );

    // `.debug_info`: a subprogram for `factorial`, attributed to the `.qn` file at line 2.
    let info = Command::new("llvm-dwarfdump")
        .arg("--debug-info")
        .arg(&bin)
        .output()
        .expect("run llvm-dwarfdump --debug-info");
    assert!(info.status.success(), "llvm-dwarfdump --debug-info failed");
    let info_out = String::from_utf8_lossy(&info.stdout);
    assert!(
        info_out.contains("DW_TAG_subprogram"),
        "expected at least one subprogram in the DWARF info, got:\n{info_out}"
    );
    assert!(
        info_out.contains("prog.qn"),
        "expected the `.qn` file referenced by a subprogram's DW_AT_decl_file"
    );
    assert!(
        info_out.contains("\"factorial\""),
        "expected a `factorial` subprogram in the DWARF info"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The quoted DWARF type name `llvm-dwarfdump` prints on the `DW_AT_type` line of the
/// variable/parameter named `var` (e.g. `Num`, `Text`, `[]Num`, `Point *`). `None` if the
/// variable or its type is absent. Used to assert each Quilon type gets a DISTINCT entry.
fn di_var_type(dump: &str, var: &str) -> Option<String> {
    let name_line = format!("DW_AT_name\t(\"{var}\")");
    let lines: Vec<&str> = dump.lines().collect();
    let at = lines.iter().position(|l| l.contains(&name_line))?;
    // The `DW_AT_type` for this DIE is one of the next few attribute lines.
    for line in lines.iter().skip(at + 1).take(6) {
        if line.contains("DW_TAG_") {
            break; // ran into the next DIE without finding a type
        }
        if let Some(rest) = line.split("DW_AT_type").nth(1)
            && let Some(open) = rest.find('"')
            && let Some(close) = rest[open + 1..].find('"')
        {
            return Some(rest[open + 1..open + 1 + close].to_string());
        }
    }
    None
}

#[test]
fn debug_build_emits_distinct_typed_local_variables() {
    let quilon = env!("CARGO_BIN_EXE_quilon");

    let Some(linker) = ["clang", "gcc"].into_iter().find(|t| tool_available(t)) else {
        eprintln!("skipping debug-variables test: need a linker (`clang` or `gcc`) on PATH");
        return;
    };
    if !tool_available("llvm-dwarfdump") {
        eprintln!("skipping debug-variables test: `llvm-dwarfdump` not on PATH");
        return;
    }
    ensure_runtime_lib(Path::new(quilon).parent().expect("binary has a parent dir"));

    // A record type plus locals covering every representation that shares the `{ptr, i64}`-ish
    // LLVM shape: `Num` (base), `Bool` (base), `Text`, `[]Num` (array), and a `Point` record
    // parameter. Each must get a DISTINCT DWARF type so a debugger can tell them apart.
    let src = "\
Point = { x :: Num, y :: Num }

describe = (p :: Point) -> Num => <
  count :: Num = 3
  flag :: Bool = count > 1
  label :: Text = \"hi\"
  nums :: []Num = [1, 2, 3]
  p.x + p.y + count + nums.size
>

^ = () -> Num => describe(Point { x = 4, y = 5 })
";
    let dir = std::env::temp_dir().join(format!("quilon_dbgvars_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let ql = dir.join("vars.qn");
    std::fs::write(&ql, src).expect("write temp source");
    let bin = dir.join("vars");

    let build = Command::new(quilon)
        .args(["build", ql.to_str().unwrap()])
        .args(["--linker", linker])
        .args(["--debug", "-o", bin.to_str().unwrap()])
        .output()
        .expect("run quilon build --debug");
    assert!(
        build.status.success(),
        "`quilon build --debug --linker {linker}` failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    // Behavior is unchanged by debug info: 4 + 5 + 3 + 3 == 15.
    let run = Command::new(&bin).status().expect("run built binary");
    assert_eq!(run.code(), Some(15), "debug build changed program behavior");

    let info = Command::new("llvm-dwarfdump")
        .arg("--debug-info")
        .arg(&bin)
        .output()
        .expect("run llvm-dwarfdump --debug-info");
    assert!(info.status.success(), "llvm-dwarfdump --debug-info failed");
    let out = String::from_utf8_lossy(&info.stdout);

    // Every local/parameter is present with its own `DW_AT_type`, and the four distinct Quilon
    // types map to four distinct DWARF type names — which is the whole point of typed locals:
    // `Text`, `[]Num`, and the `Point` record all share the `{ptr, i64}`-ish LLVM shape yet
    // must NOT collapse to one DWARF type.
    let count_ty = di_var_type(&out, "count").expect("`count` variable with a type");
    let flag_ty = di_var_type(&out, "flag").expect("`flag` variable with a type");
    let label_ty = di_var_type(&out, "label").expect("`label` variable with a type");
    let nums_ty = di_var_type(&out, "nums").expect("`nums` variable with a type");
    let p_ty = di_var_type(&out, "p").expect("`p` parameter with a type");

    assert_eq!(
        count_ty, "Num",
        "`count :: Num` should be the `Num` base type"
    );
    assert_eq!(
        flag_ty, "Bool",
        "`flag :: Bool` should be the `Bool` base type"
    );
    assert_eq!(
        label_ty, "Text",
        "`label :: Text` should be the `Text` type"
    );
    assert_eq!(
        nums_ty, "[]Num",
        "`nums :: []Num` should be the `[]Num` type"
    );
    assert!(
        p_ty.contains("Point"),
        "`p :: Point` should reference the `Point` record type, got {p_ty:?}"
    );

    // The three composite types that share the `{ptr, i64}`-ish shape are pairwise distinct.
    assert_ne!(
        label_ty, nums_ty,
        "Text and []Num must be distinct DWARF types"
    );
    assert_ne!(
        label_ty, p_ty,
        "Text and the record must be distinct DWARF types"
    );
    assert_ne!(
        nums_ty, p_ty,
        "[]Num and the record must be distinct DWARF types"
    );

    // Spot-check the emitted type identities: `Text` and `[]Num` are named struct types, and
    // `Num` is a float base type. These names are what a pretty-printer dispatches on.
    assert!(
        out.contains("DW_AT_name\t(\"Text\")"),
        "expected a named `Text` DWARF type"
    );
    assert!(
        out.contains("DW_AT_name\t(\"[]Num\")"),
        "expected a named `[]Num` DWARF type"
    );
    assert!(
        out.contains("DW_ATE_float"),
        "expected `Num` emitted as a float base type"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn debug_build_emits_distinct_sum_type_debug_info() {
    let quilon = env!("CARGO_BIN_EXE_quilon");

    let Some(linker) = ["clang", "gcc"].into_iter().find(|t| tool_available(t)) else {
        eprintln!("skipping sum-debug test: need a linker on PATH");
        return;
    };
    if !tool_available("llvm-dwarfdump") {
        eprintln!("skipping sum-debug test: `llvm-dwarfdump` not on PATH");
        return;
    }
    ensure_runtime_lib(Path::new(quilon).parent().expect("binary has a parent dir"));

    // A user sum type and the built-in `Result`: both must get distinct tagged-struct DWARF
    // types (a leading `tag` member + payload slots), distinct from each other and from records.
    let src = "\
Color = Red / Green / Blue

^ = () -> Num => <
  c :: Color = Green
  outcome :: Result = Ok(42)
  outcome ?
    | Ok(x)    => x
    | NotOk(e) => 0
>
";
    let dir = std::env::temp_dir().join(format!("quilon_dbgsum_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let ql = dir.join("sum.qn");
    std::fs::write(&ql, src).expect("write temp source");
    let bin = dir.join("sum");

    let build = Command::new(quilon)
        .args(["build", ql.to_str().unwrap()])
        .args(["--linker", linker])
        .args(["--debug", "-o", bin.to_str().unwrap()])
        .output()
        .expect("run quilon build --debug");
    assert!(
        build.status.success(),
        "`quilon build --debug` failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).status().expect("run built binary");
    assert_eq!(run.code(), Some(42), "debug build changed program behavior");

    let info = Command::new("llvm-dwarfdump")
        .arg("--debug-info")
        .arg(&bin)
        .output()
        .expect("run llvm-dwarfdump --debug-info");
    let out = String::from_utf8_lossy(&info.stdout);

    let c_ty = di_var_type(&out, "c").expect("`c` variable with a type");
    let outcome_ty = di_var_type(&out, "outcome").expect("`outcome` variable with a type");
    assert_eq!(c_ty, "Color", "`c :: Color` should be the `Color` sum type");
    assert_eq!(
        outcome_ty, "Result",
        "`outcome :: Result` should be the `Result` sum type"
    );
    assert_ne!(
        c_ty, outcome_ty,
        "distinct sum types must be distinct DWARF types"
    );
    // The sum types carry the tagged-union tag member.
    assert!(
        out.contains("DW_AT_name\t(\"Color\")") && out.contains("DW_AT_name\t(\"tag\")"),
        "expected a `Color` tagged-struct with a `tag` member"
    );
    // The payload slot must land at its NATURAL offset (byte 8, after the i8 tag + padding),
    // matching the LLVM `{ i8, double }` value — not byte 1. A regression here (using the
    // always-zero basic-type alignment) would mis-place every sum/record payload.
    let color = out
        .split("DW_AT_name\t(\"Color\")")
        .nth(1)
        .expect("Color struct in the dump");
    let color = &color[..color.find("DW_TAG_structure_type").unwrap_or(color.len())];
    assert!(
        color.contains("DW_AT_byte_size\t(0x10)"),
        "Color should be 16 bytes ({{ i8 tag, double payload }}), got:\n{color}"
    );
    assert!(
        color.contains("DW_AT_data_member_location\t(0x08)"),
        "Color's payload must sit at byte offset 8, got:\n{color}"
    );

    // `Result` has ONE canonical `{ptr,i64}` payload slot (16 bytes) into which any payload
    // is packed, so its tagged-struct is `{ i8 tag, {ptr,i64} payload }` = 24 bytes (0x18)
    // with the payload at byte offset 8 — matching the uniform LLVM Result value.
    let result = out
        .split("DW_AT_name\t(\"Result\")")
        .nth(1)
        .expect("Result struct in the dump");
    let result = &result[..result.find("DW_TAG_structure_type").unwrap_or(result.len())];
    assert!(
        result.contains("DW_AT_byte_size\t(0x18)"),
        "Result should be 24 bytes ({{ i8 tag, {{ptr,i64}} payload }}), got:\n{result}"
    );
    assert!(
        result.contains("DW_AT_data_member_location\t(0x08)"),
        "Result's payload slot must sit at byte offset 8, got:\n{result}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn non_debug_build_has_no_ql_debug_info() {
    let quilon = env!("CARGO_BIN_EXE_quilon");

    let Some(linker) = ["clang", "gcc"].into_iter().find(|t| tool_available(t)) else {
        eprintln!("skipping non-debug-info test: need a linker on PATH");
        return;
    };
    if !tool_available("llvm-dwarfdump") {
        eprintln!("skipping non-debug-info test: `llvm-dwarfdump` not on PATH");
        return;
    }
    ensure_runtime_lib(Path::new(quilon).parent().expect("binary has a parent dir"));

    let src = "^ = () -> Num => 7\n";
    let dir = std::env::temp_dir().join(format!("quilon_nodbg_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let ql = dir.join("plain.qn");
    std::fs::write(&ql, src).expect("write temp source");
    let bin = dir.join("plain");

    let build = Command::new(quilon)
        .args(["build", ql.to_str().unwrap()])
        .args(["--linker", linker])
        .args(["-o", bin.to_str().unwrap()])
        .output()
        .expect("run quilon build");
    assert!(
        build.status.success(),
        "`quilon build` failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    // Without `--debug`, no compile unit should reference the `.qn` source. (The Rust
    // runtime's own debug info may be present, but it never names a `.qn` file.)
    let info = Command::new("llvm-dwarfdump")
        .arg("--debug-info")
        .arg(&bin)
        .output()
        .expect("run llvm-dwarfdump --debug-info");
    let info_out = String::from_utf8_lossy(&info.stdout);
    assert!(
        !info_out.contains("plain.qn"),
        "a non-debug build must not carry `.qn` debug info, got:\n{info_out}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
