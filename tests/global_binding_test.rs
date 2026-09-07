//! Top-level bindings: module-level state.
//!
//! A top-level binding's initializer is computed once, in file order, before `^` — imports
//! first, then top to bottom, an initializer seeing only what is defined above it (no
//! hoisting). A `:=` global is one mutable cell for the whole program: a write from any
//! function persists across calls, and `>>` on it is a compile error (mutation does not
//! cross a module boundary). Every accepted form here is proven under BOTH the JIT
//! (`assert_exit`) and, for the form that used to lose its write silently, a native build
//! too — `check`, `run`, and `build` must all agree.

mod common;
use common::{
    assert_exit, assert_type_error, assert_type_error_code, build_and_run_native, tool_available,
};
use quilon::diagnostic::codes::Code;

#[test]
fn a_mutable_global_survives_across_calls() {
    // Regression: a `:=` global's write used to be lost on return.
    let src = concat!(
        "sheepCounted := 0\n",
        "countSheep = () -> Num => <\n",
        "  sheepCounted := sheepCounted + 1\n",
        "  sheepCounted\n",
        ">\n",
        "^ = () -> Num => <\n",
        "  a = countSheep()\n",
        "  b = countSheep()\n",
        "  a * 10 + b\n",
        ">",
    );
    assert_exit(src, 12);

    if !tool_available("clang") {
        eprintln!("skipping the native half of the counter regression: clang is not on PATH");
        return;
    }
    let (code, _) = build_and_run_native("global_counter_native", src);
    assert_eq!(code, 12, "a native build must agree with the JIT");
}

#[test]
fn a_computed_text_global_is_accepted() {
    assert_exit(
        "battleCry = \"hi \" + \"there\"\n^ = () -> Num => < battleCry.size >",
        8,
    );
}

#[test]
fn a_computed_record_global_is_accepted() {
    assert_exit(
        "Llama = { spit :: Num }\npet = Llama { spit = 5 }\n^ = () -> Num => < pet.spit >",
        5,
    );
}

#[test]
fn a_mutable_record_global_field_is_written_through_from_a_function() {
    let src = concat!(
        "Llama = { spit :: Num }\n",
        "grumpyCat := Llama { spit = 0 }\n",
        "poke = () -> Num => <\n",
        "  grumpyCat.spit := grumpyCat.spit + 1\n",
        "  grumpyCat.spit\n",
        ">\n",
        "^ = () -> Num => <\n",
        "  poke()\n",
        "  poke()\n",
        ">",
    );
    assert_exit(src, 2);
}

#[test]
fn a_whole_array_global_reassignment_persists() {
    // Same failure shape as the counter regression above, but for a whole-value reassignment.
    let src = concat!(
        "shoppingList := [1]\n",
        "addToList = () -> Num => <\n",
        "  shoppingList := shoppingList + 2\n",
        "  shoppingList.size\n",
        ">\n",
        "^ = () -> Num => <\n",
        "  addToList()\n",
        "  addToList()\n",
        ">",
    );
    assert_exit(src, 3);
}

#[test]
fn a_computed_global_calling_a_function_above_it_is_accepted() {
    assert_exit(
        "addOne = (n :: Num) -> Num => < n + 1 >\nanswer = addOne(41)\n^ = () -> Num => < answer >",
        42,
    );
}

#[test]
fn a_global_computed_from_another_global_is_accepted() {
    assert_exit(
        "crew = 3\ntwiceTheCrew = crew * 2\n^ = () -> Num => < twiceTheCrew >",
        6,
    );
}

#[test]
fn a_global_initializer_sees_only_what_is_above_it() {
    // No hoisting: `today` is undefined at the point `tomorrow`'s initializer reads it.
    assert_type_error("tomorrow = today + 1\ntoday = 5\n^ = () -> Num => < tomorrow >");
}

#[test]
fn an_exported_mutable_global_is_rejected() {
    assert_type_error_code(">> secretSauce := 0", Code::ExportedMutableGlobal);
}

#[test]
fn deep_immutability_still_fires_across_a_global() {
    // Deep immutability applies whether the `:=` side is a local or a global.
    let src = concat!(
        "Llama = { spit :: Num }\n",
        "mood := Llama { spit = 1 }\n",
        "peek = () -> Num => <\n",
        "  snapshot = mood\n",
        "  snapshot.spit\n",
        ">\n",
        "^ = () -> Num => < peek() >",
    );
    assert_type_error(src);
}

#[test]
fn a_global_initializer_using_a_match_expression_is_accepted() {
    let src = concat!(
        "Size = Small / Large\n",
        "chosen = Large\n",
        "factor = chosen ?\n",
        "  | Small => 1\n",
        "  | Large => 2\n",
        "^ = () -> Num => < factor * 7 >",
    );
    assert_exit(src, 14);
}

#[test]
fn a_global_initializer_using_backtick_interpolation_over_an_array_is_accepted() {
    let src = concat!(
        "lotteryNumbers = [7, 13, 21]\n",
        "banner = \"winners `lotteryNumbers`\"\n",
        "^ = () -> Num => < banner.size >",
    );
    assert_exit(src, 19);
}

#[test]
fn a_deferred_value_is_forced_at_a_global_initializer() {
    // A top-level initializer is a strict site (`src/deferral.rs`), so `@readStdin()`
    // bound to a global still forces before `^` runs. Spawns the real binary: a deferred
    // read's background fiber can't safely piggyback on a same-process JIT call here.
    let source = concat!(
        "<< core.io\n",
        "line = @readStdin()\n",
        "^ = () -> Num => <\n",
        "  io.print(line)\n",
        "  0\n",
        ">\n",
    );
    let dir = std::env::temp_dir().join(format!(
        "quilon_global_defer_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let file = dir.join("program.qn");
    std::fs::write(&file, source).expect("write temp program");

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_quilon"))
        .args(["run", file.to_str().unwrap()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn quilon run");
    use std::io::Write;
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(b"kaki the yak\n")
        .expect("write to child stdin");
    let output = child.wait_with_output().expect("wait for quilon run");

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "kaki the yak\n",
        "the global's deferred @readStdin value must have been forced before `^` ran"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
