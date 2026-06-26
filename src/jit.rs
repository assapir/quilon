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

/// Signature of the generated C `main`: `int main(int argc, char** argv)`.
type MainFn = unsafe extern "C" fn(i32, *const *const u8) -> i32;

/// JIT-compile and execute a type-checked program in-process.
///
/// Returns the value the program's `^` entry point yields, as an `i32` exit
/// code. Libc symbols the generated code may reference (e.g. `printf`,
/// `malloc`, `memcpy`) resolve automatically from the host process. Custom
/// runtime intrinsics added by later workstreams (e.g. `__text_length`,
/// Boehm GC) are registered at the extension point noted below.
pub fn run_program(program: &Program) -> Result<i32, String> {
    // LLVM requires the native target to be initialized before a JIT engine
    // can be created.
    Target::initialize_native(&InitializationConfig::default())
        .map_err(|e| format!("Failed to initialize native target: {}", e))?;

    let context = Context::create();
    let mut generator = CodeGenerator::new(&context, "main");

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
        ];
        for (name, addr) in mappings {
            if let Some(func) = module.get_function(name) {
                engine.add_global_mapping(&func, *addr);
            }
        }
    }

    unsafe {
        let main: JitFunction<MainFn> = engine
            .get_function("main")
            .map_err(|_| "Program has no entry point to execute (expected `^`)".to_string())?;

        // Numeric entry points (`() -> Num`) ignore argc/argv; pass empty args.
        Ok(main.call(0, std::ptr::null()))
    }
}
