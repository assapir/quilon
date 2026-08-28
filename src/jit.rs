// In-process LLVM JIT execution for Quilon programs.
//
// Compiles a type-checked `Program` to LLVM IR via the code generator, then
// executes the generated C-compatible `main` wrapper in-process using inkwell's
// `ExecutionEngine`, returning the program's exit code. This is what backs
// `quilon run` and the execution-based test harness in `tests/run_test.rs`.

use crate::ast::Program;
use crate::codegen::CodeGenerator;
use crate::deferral::DeferInfo;
use crate::source_map::SourceMap;
use crate::typechecker::TypeTable;
use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::execution_engine::JitFunction;
use inkwell::targets::{InitializationConfig, Target};
use std::ffi::CString;
use std::os::raw::c_char;
use std::rc::Rc;

/// Signature of the generated C `main`: `int main(int argc, char** argv, char** envp)`.
type MainFn = unsafe extern "C" fn(i32, *const *const c_char, *const *const c_char) -> i32;

/// `values` as the NUL-terminated C strings a `main` receives, REFUSING one that carries a
/// NUL of its own.
///
/// A NUL is where a C string ends, so a value containing one cannot be passed to a program
/// at all: `execve` rejects it, and a native build therefore never sees such an argument or
/// environment entry. The JIT rejects it for the same reason, rather than substituting a
/// value (an empty string, or the bytes before the NUL) the program would read as real —
/// which is a JIT/AOT parity break, and a silent one.
fn c_strings(
    values: impl Iterator<Item = impl Into<Vec<u8>>>,
    what: &str,
) -> Result<Vec<CString>, String> {
    values
        .enumerate()
        .map(|(index, value)| {
            CString::new(value).map_err(|e| {
                format!(
                    "{what} {index} contains a NUL byte at position {}, so it cannot be \
                     passed to a program",
                    e.nul_position()
                )
            })
        })
        .collect()
}

/// JIT-compile and execute a type-checked program in-process.
///
/// `args` is the exact argument vector the program's `^` entry point should see
/// as `args :: []Text` — `argv[0]` first, then any trailing arguments. Callers
/// are responsible for building it the same way the OS builds a native binary's
/// argv: for `quilon run <file> [user args...]`, `main.rs` passes
/// `[<file>, <user args...>]` so the JIT mirrors `./<file> <user args...>` and
/// never leaks the `quilon run` CLI prefix.
///
/// Returns the value the program's `^` entry point yields, as an `i32` exit
/// code. Libc symbols the generated code may reference (e.g. `printf`,
/// `malloc`, `memcpy`) resolve automatically from the host process. Custom
/// runtime intrinsics added by later workstreams (e.g. `__text_length`,
/// Boehm GC) are registered at the extension point noted below.
pub fn run_program(
    program: &Program,
    types: TypeTable,
    defer: DeferInfo,
    sources: Rc<SourceMap>,
    args: &[String],
) -> Result<i32, String> {
    // The JIT'd program allocates through the collector on whichever thread called us,
    // and the collector aborts the process if it has to stop a thread it was never told
    // about. A compiled binary never meets this — it has one thread — but a host that
    // runs programs from several threads does, which is what the test suite is. Held for
    // the duration of the run; dropping it unregisters the thread.
    let _gc_thread = quilon_rt::register_thread();

    // LLVM requires the native target to be initialized before a JIT engine
    // can be created.
    Target::initialize_native(&InitializationConfig::default())
        .map_err(|e| format!("Failed to initialize native target: {}", e))?;

    let context = Context::create();
    // Build the generator with the type oracle installed (so read sites recover precise
    // element/field/match-result types instead of assuming f64).
    let mut generator = CodeGenerator::new(&context, "main");
    generator.set_type_table(types);
    generator.set_defer_info(defer);
    generator.set_source_map(sources);

    // Populate, verify, and emit the module (also builds the `main` wrapper).
    generator.generate(program)?;

    let module = generator.module();

    let engine = module
        .create_jit_execution_engine(OptimizationLevel::None)
        .map_err(|e| format!("Failed to create JIT execution engine: {}", e))?;

    // Register the Rust-provided runtime intrinsics with the JIT. libc/libgc symbols
    // (memcpy, GC_*) resolve from the host process automatically, but our
    // `#[no_mangle]` Rust wrappers are not in the dynamic symbol table, so the JIT
    // cannot find them via dlsym — map any the module declares to its in-process
    // address. Without this, the generated `main` calls `__gc_init` at a null address
    // and segfaults. The addresses come from the runtime's own registry, so an
    // intrinsic cannot be added there and forgotten here.
    for (name, address) in crate::runtime::intrinsics::INTRINSICS {
        if let Some(func) = module.get_function(name) {
            engine.add_global_mapping(&func, *address as usize);
        }
    }

    // Build the real `argv`/`envp` the JIT'd `main` will thread into an `^` that
    // declares `args :: []Text` / `env :: [|Text => Text|]`. These mirror the C arrays a
    // native `main` receives: NULL-terminated arrays of NUL-terminated C strings. The
    // owning `CString`/pointer `Vec`s must outlive the `main.call` below (the runtime
    // copies their bytes into GC memory during the call), so they are bound here.
    //
    // `argv` comes from the caller-supplied `args` (not `std::env::args()`), so a
    // JIT'd program sees exactly the argument vector a native build would — with no
    // `quilon run` CLI prefix leaked in. `envp` still comes from the
    // process environment, matching what the OS hands a native binary.
    let arg_cstrings = c_strings(args.iter().map(String::as_str), "argument")?;
    let mut argv: Vec<*const c_char> = arg_cstrings.iter().map(|c| c.as_ptr()).collect();
    argv.push(std::ptr::null()); // argv is conventionally NULL-terminated
    let argc = arg_cstrings.len() as i32;

    let env_cstrings = c_strings(
        std::env::vars().map(|(k, v)| format!("{k}={v}")),
        "environment entry",
    )?;
    let mut envp: Vec<*const c_char> = env_cstrings.iter().map(|c| c.as_ptr()).collect();
    envp.push(std::ptr::null()); // envp is NULL-terminated

    unsafe {
        let main: JitFunction<MainFn> = engine
            .get_function("main")
            .map_err(|_| "Program has no entry point to execute (expected `^`)".to_string())?;

        Ok(main.call(argc, argv.as_ptr(), envp.as_ptr()))
    }
}
