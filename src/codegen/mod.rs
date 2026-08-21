// Code generation module for Quilon (LLVM IR)

pub mod debug;
pub mod generator;
pub mod num_type;

pub use generator::CodeGenerator;

/// The LLVM layout of the built-in `Site` record — see [`CodeGenerator::site_struct_type`].
pub fn site_struct_type<'ctx>(
    context: &'ctx inkwell::context::Context,
) -> Result<inkwell::types::StructType<'ctx>, String> {
    CodeGenerator::site_struct_type(context)
}
