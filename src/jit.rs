// In-process LLVM JIT execution for Quilon programs.
//
// Compiles a type-checked `Program` to LLVM IR via the code generator, then
// executes the generated C-compatible `main` wrapper in-process using inkwell's
// `ExecutionEngine`, returning the program's exit code. This is what backs
// `quilon run` and the execution-based test harness in `tests/run_test.rs`.

use crate::ast::Program;
use crate::codegen::CodeGenerator;
use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::execution_engine::JitFunction;
use inkwell::targets::{InitializationConfig, Target};
use std::ffi::CString;
use std::os::raw::c_char;

/// Signature of the generated C `main`: `int main(int argc, char** argv, char** envp)`.
type MainFn = unsafe extern "C" fn(i32, *const *const c_char, *const *const c_char) -> i32;

/// JIT-compile and execute a type-checked program in-process.
///
/// `args` is the exact argument vector the program's `^` entry point should see
/// as `args :: []Text` — `argv[0]` first, then any trailing arguments. Callers
/// are responsible for building it the same way the OS builds a native binary's
/// argv: for `quilon run <file> [user args...]`, `main.rs` passes
/// `[<file>, <user args...>]` so the JIT mirrors `./<file> <user args...>` and
/// never leaks the `quilon run` CLI prefix (issue #44).
///
/// Returns the value the program's `^` entry point yields, as an `i32` exit
/// code. Libc symbols the generated code may reference (e.g. `printf`,
/// `malloc`, `memcpy`) resolve automatically from the host process. Custom
/// runtime intrinsics added by later workstreams (e.g. `__text_length`,
/// Boehm GC) are registered at the extension point noted below.
pub fn run_program(program: &Program, args: &[String]) -> Result<i32, String> {
    // LLVM requires the native target to be initialized before a JIT engine
    // can be created.
    Target::initialize_native(&InitializationConfig::default())
        .map_err(|e| format!("Failed to initialize native target: {}", e))?;

    let context = Context::create();
    // Build the generator with the type oracle installed (so read sites recover precise
    // element/field/match-result types instead of assuming f64).
    let mut generator = CodeGenerator::with_oracle(&context, "main", program)?;

    // Populate, verify, and emit the module (also builds the `main` wrapper).
    generator.generate(program)?;

    let module = generator.module();

    let engine = module
        .create_jit_execution_engine(OptimizationLevel::None)
        .map_err(|e| format!("Failed to create JIT execution engine: {}", e))?;

    // Register the Rust-provided runtime intrinsics with the JIT. libc/libgc
    // symbols (memcpy, GC_*) resolve from the host process automatically, but
    // our `#[no_mangle]` Rust wrappers are not in the dynamic symbol table, so
    // the JIT cannot find them via dlsym — map any the module declares to their
    // in-process addresses. Without this, the generated `main` calls
    // `__gc_init` at a null address and segfaults.
    {
        use crate::runtime::intrinsics;
        let mappings: &[(&str, usize)] = &[
            ("__gc_init", intrinsics::__gc_init as *const () as usize),
            ("__alloc", intrinsics::__alloc as *const () as usize),
            (
                "__text_length",
                intrinsics::__text_length as *const () as usize,
            ),
            ("__text_cmp", intrinsics::__text_cmp as *const () as usize),
            (
                "__write_bytes",
                intrinsics::__write_bytes as *const () as usize,
            ),
            (
                "__print_num_fd",
                intrinsics::__print_num_fd as *const () as usize,
            ),
            (
                "__print_bool_fd",
                intrinsics::__print_bool_fd as *const () as usize,
            ),
            (
                "__print_text_fd",
                intrinsics::__print_text_fd as *const () as usize,
            ),
            (
                "__argv_to_text_array",
                intrinsics::__argv_to_text_array as *const () as usize,
            ),
            (
                "__envp_to_pairs",
                intrinsics::__envp_to_pairs as *const () as usize,
            ),
            (
                "__text_trim_start",
                intrinsics::__text_trim_start as *const () as usize,
            ),
            (
                "__text_trim_end",
                intrinsics::__text_trim_end as *const () as usize,
            ),
            (
                "__text_to_upper",
                intrinsics::__text_to_upper as *const () as usize,
            ),
            (
                "__text_to_lower",
                intrinsics::__text_to_lower as *const () as usize,
            ),
            (
                "__text_contains",
                intrinsics::__text_contains as *const () as usize,
            ),
            (
                "__text_index_of",
                intrinsics::__text_index_of as *const () as usize,
            ),
            (
                "__text_replace_all",
                intrinsics::__text_replace_all as *const () as usize,
            ),
            (
                "__text_replace_n",
                intrinsics::__text_replace_n as *const () as usize,
            ),
            (
                "__text_slice",
                intrinsics::__text_slice as *const () as usize,
            ),
            (
                "__text_split",
                intrinsics::__text_split as *const () as usize,
            ),
        ];
        for (name, addr) in mappings {
            if let Some(func) = module.get_function(name) {
                engine.add_global_mapping(&func, *addr);
            }
        }
    }

    // Build the real `argv`/`envp` the JIT'd `main` will thread into an `^` that
    // declares `args :: []Text` / `env :: [][]Text`. These mirror the C arrays a
    // native `main` receives: NULL-terminated arrays of NUL-terminated C strings. The
    // owning `CString`/pointer `Vec`s must outlive the `main.call` below (the runtime
    // copies their bytes into GC memory during the call), so they are bound here.
    //
    // `argv` comes from the caller-supplied `args` (not `std::env::args()`), so a
    // JIT'd program sees exactly the argument vector a native build would — with no
    // `quilon run` CLI prefix leaked in (issue #44). `envp` still comes from the
    // process environment, matching what the OS hands a native binary.
    let arg_cstrings: Vec<CString> = args
        .iter()
        .map(|a| CString::new(a.as_str()).unwrap_or_default())
        .collect();
    let mut argv: Vec<*const c_char> = arg_cstrings.iter().map(|c| c.as_ptr()).collect();
    argv.push(std::ptr::null()); // argv is conventionally NULL-terminated
    let argc = arg_cstrings.len() as i32;

    let env_cstrings: Vec<CString> = std::env::vars()
        .map(|(k, v)| CString::new(format!("{k}={v}")).unwrap_or_default())
        .collect();
    let mut envp: Vec<*const c_char> = env_cstrings.iter().map(|c| c.as_ptr()).collect();
    envp.push(std::ptr::null()); // envp is NULL-terminated

    unsafe {
        let main: JitFunction<MainFn> = engine
            .get_function("main")
            .map_err(|_| "Program has no entry point to execute (expected `^`)".to_string())?;

        Ok(main.call(argc, argv.as_ptr(), envp.as_ptr()))
    }
}
