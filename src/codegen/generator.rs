// LLVM code generator for Quilon

use crate::ast::{
    BinOp, Expr, FunctionDecl, Item, MatchArm, MethodDecl, Pattern, Program, Type, TypeDecl,
    TypeDef, UnaryOp, VarDecl,
};
use inkwell::AddressSpace;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::types::{BasicType, BasicTypeEnum};
use inkwell::values::{BasicValue, BasicValueEnum, FunctionValue, PointerValue};
use std::collections::HashMap;

pub struct CodeGenerator<'ctx> {
    context: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    variables: HashMap<String, (PointerValue<'ctx>, BasicTypeEnum<'ctx>)>,
    // Track record field mappings: variable name -> (field names, field types)
    record_types: HashMap<String, Vec<String>>,
    // Named record types: type name -> field names (declared order)
    named_type_fields: HashMap<String, Vec<String>>,
    // Track which named type a variable was constructed from: var name -> type name
    var_named_types: HashMap<String, String>,
    current_function: Option<FunctionValue<'ctx>>,
}

impl<'ctx> CodeGenerator<'ctx> {
    pub fn new(context: &'ctx Context, module_name: &str) -> Self {
        let module = context.create_module(module_name);
        let builder = context.create_builder();

        CodeGenerator {
            context,
            module,
            builder,
            variables: HashMap::new(),
            record_types: HashMap::new(),
            named_type_fields: HashMap::new(),
            var_named_types: HashMap::new(),
            current_function: None,
        }
    }

    /// Access the underlying LLVM module after `generate` has populated it.
    /// Used by the JIT runner to create an execution engine in-process.
    pub fn module(&self) -> &Module<'ctx> {
        &self.module
    }

    pub fn generate(&mut self, program: &Program) -> Result<String, String> {
        // Generate code for all top-level items
        for item in &program.items {
            self.generate_item(item)?;
        }

        // Check if entry point function (^) exists and generate C main wrapper
        if self.module.get_function("^").is_some() {
            self.generate_main_wrapper()?;
        }

        // Verify the module
        if let Err(e) = self.module.verify() {
            return Err(format!("Module verification failed: {}", e));
        }

        // Return the LLVM IR as a string
        Ok(self.module.print_to_string().to_string())
    }

    fn generate_main_wrapper(&mut self) -> Result<(), String> {
        // Create C-compatible main: int main(int argc, char** argv)
        let i32_type = self.context.i32_type();
        let ptr_type = self.context.ptr_type(AddressSpace::default());

        let main_type = i32_type.fn_type(&[i32_type.into(), ptr_type.into()], false);

        let main_fn = self.module.add_function("main", main_type, None);
        let argc = main_fn.get_nth_param(0).unwrap().into_int_value();
        let _argv = main_fn.get_nth_param(1).unwrap().into_pointer_value();

        let entry = self.context.append_basic_block(main_fn, "entry");
        self.builder.position_at_end(entry);

        // Initialize the Boehm GC before any user code runs. Compiled programs
        // allocate heap memory (Text, sum payloads) via GC_malloc.
        let gc_init = self.get_intrinsic("__gc_init")?;
        self.builder
            .build_call(gc_init, &[], "")
            .map_err(|e| format!("Failed to call GC init: {:?}", e))?;

        // Get the ^ (entry point) function
        let user_entry = self
            .module
            .get_function("^")
            .ok_or_else(|| "Entry point function ^ not found".to_string())?;

        // Check the signature of ^ to determine how to call it
        let user_entry_type = user_entry.get_type();
        let param_count = user_entry_type.count_param_types();

        let result = if param_count == 0 {
            // >> = () -> Num - Call with no arguments
            self.builder
                .build_call(user_entry, &[], "entry_result")
                .map_err(|e| format!("Failed to call entry point: {:?}", e))?
        } else if param_count == 2 {
            // >> = (argc, argv) -> Num - Pass argc/argv
            // Convert argc to Num (f64)
            let argc_as_f64 = self
                .builder
                .build_signed_int_to_float(argc, self.context.f64_type(), "argc_f64")
                .map_err(|e| format!("Failed to convert argc: {:?}", e))?;

            // For argv, since we don't have proper pointer types yet, pass 0
            // TODO: Convert argv to []String when we have proper array support
            let argv_placeholder = self.context.f64_type().const_zero();

            let args: &[inkwell::values::BasicMetadataValueEnum] =
                &[argc_as_f64.into(), argv_placeholder.into()];

            self.builder
                .build_call(user_entry, args, "entry_result")
                .map_err(|e| format!("Failed to call entry point: {:?}", e))?
        } else {
            return Err(format!(
                "Entry point >> must have 0 or 2 parameters, found {}. \
                 Valid signatures: '() -> Num' or '(argc :: Num, argv :: Num) -> Num'",
                param_count
            ));
        };

        // Convert result to i32
        use inkwell::values::AnyValue;
        let return_val = match result.as_any_value_enum() {
            inkwell::values::AnyValueEnum::FloatValue(f) => {
                // Convert double to i32
                self.builder
                    .build_float_to_signed_int(f, i32_type, "result_int")
                    .map_err(|e| format!("Failed to convert result: {:?}", e))?
            }
            _ => {
                // Return 0 if not a numeric result
                i32_type.const_zero()
            }
        };

        self.builder
            .build_return(Some(&return_val))
            .map_err(|e| format!("Failed to build return: {:?}", e))?;

        Ok(())
    }

    fn generate_item(&mut self, item: &Item) -> Result<(), String> {
        match item {
            Item::VarDecl(decl) => self.generate_var_decl(decl),
            Item::FunctionDecl(decl) => self.generate_function_decl(decl),
            Item::TypeDecl(decl) => self.generate_type_decl(decl),
        }
    }

    /// Generate code for a named type declaration.
    ///
    /// Sum types carry no code. Record types register their field layout and emit each
    /// method as a top-level function `"{TypeName}_{method}"` whose first parameter is the
    /// implicit receiver `it` (passed as a pointer to the record struct). Dispatch is static
    /// (monomorphic) — `recv.method(args)` resolves to that mangled function at the call site.
    fn generate_type_decl(&mut self, decl: &TypeDecl) -> Result<(), String> {
        if let TypeDef::Record { fields, methods } = &decl.type_def {
            let field_names: Vec<String> = fields.iter().map(|(n, _)| n.clone()).collect();
            self.named_type_fields
                .insert(decl.name.clone(), field_names.clone());

            let ptr_type = self.context.ptr_type(AddressSpace::default());

            // Pass 1: declare every method signature first, so a method body may reference
            // sibling methods (or recurse) regardless of declaration order.
            for method in methods {
                let mangled = format!("{}_{}", decl.name, method.name);
                if self.module.get_function(&mangled).is_some() {
                    continue;
                }
                let mut param_types: Vec<inkwell::types::BasicMetadataTypeEnum> =
                    vec![ptr_type.into()];
                for p in &method.params {
                    let pt = self.type_to_llvm(&p.type_annotation.clone().unwrap_or(Type::Num))?;
                    param_types.push(pt.into());
                }
                let return_type =
                    self.type_to_llvm(&method.return_type.clone().unwrap_or(Type::Num))?;
                let fn_type = return_type.fn_type(&param_types, false);
                self.module.add_function(&mangled, fn_type, None);
            }

            // Pass 2: generate each method body.
            for method in methods {
                self.generate_method(&decl.name, &field_names, method)?;
            }
        }

        // Type declarations are not inside a function; clear any stray function context so a
        // following global declaration is not mistaken for a local.
        self.current_function = None;
        Ok(())
    }

    /// Emit the body of a single method as the pre-declared `"{TypeName}_{method}"` function,
    /// with `it` bound to the receiver pointer so `it.field` / sibling-method calls resolve.
    fn generate_method(
        &mut self,
        type_name: &str,
        field_names: &[String],
        method: &MethodDecl,
    ) -> Result<(), String> {
        let mangled = format!("{}_{}", type_name, method.name);
        let function = self
            .module
            .get_function(&mangled)
            .ok_or_else(|| format!("Method function not declared: {}", mangled))?;
        self.current_function = Some(function);

        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);

        self.variables.clear();

        // Param 0 is the implicit receiver `it` (a pointer to the record struct).
        let it_param = function.get_nth_param(0).unwrap();
        it_param.set_name("it");
        let it_type = it_param.as_basic_value_enum().get_type();
        let it_alloca = self.create_entry_block_alloca("it", it_type)?;
        self.builder
            .build_store(it_alloca, it_param)
            .map_err(|e| format!("Failed to store it: {:?}", e))?;
        self.variables
            .insert("it".to_string(), (it_alloca, it_type));
        // So `it.field` and `it.method()` resolve against this type.
        self.record_types
            .insert("it".to_string(), field_names.to_vec());
        self.var_named_types
            .insert("it".to_string(), type_name.to_string());

        // Remaining params follow the receiver.
        for (i, param) in method.params.iter().enumerate() {
            let llvm_param = function.get_nth_param((i + 1) as u32).unwrap();
            llvm_param.set_name(&param.name);
            let param_type = llvm_param.as_basic_value_enum().get_type();
            let alloca = self.create_entry_block_alloca(&param.name, param_type)?;
            self.builder
                .build_store(alloca, llvm_param)
                .map_err(|e| format!("Failed to build store: {:?}", e))?;
            self.variables
                .insert(param.name.clone(), (alloca, param_type));
        }

        let body_value = self.generate_expr(&method.body)?;
        self.builder
            .build_return(Some(&body_value))
            .map_err(|e| format!("Failed to build return: {:?}", e))?;

        Ok(())
    }

    fn generate_var_decl(&mut self, decl: &VarDecl) -> Result<(), String> {
        // Check if this is a record literal to track field names
        if let Expr::Record { fields, .. } = &decl.value {
            let field_names: Vec<String> = fields.iter().map(|(name, _)| name.clone()).collect();
            self.record_types.insert(decl.name.clone(), field_names);
        }
        // A named-type instance (e.g. `u = User { ... }`) — remember its type so method calls
        // on `u` can resolve to the mangled `User_method` functions.
        if let Expr::Constructor {
            type_name, fields, ..
        } = &decl.value
        {
            let field_names: Vec<String> = self
                .named_type_fields
                .get(type_name)
                .cloned()
                .unwrap_or_else(|| fields.iter().map(|(name, _)| name.clone()).collect());
            self.record_types.insert(decl.name.clone(), field_names);
            self.var_named_types
                .insert(decl.name.clone(), type_name.clone());
        }

        let value = self.generate_expr(&decl.value)?;

        if self.current_function.is_some() {
            // Local variable - use alloca
            let var_type = value.get_type();
            let alloca = self.create_entry_block_alloca(&decl.name, var_type)?;
            self.builder
                .build_store(alloca, value)
                .map_err(|e| format!("Failed to build store: {:?}", e))?;
            self.variables.insert(decl.name.clone(), (alloca, var_type));
        } else {
            // Global variable
            let global =
                self.module
                    .add_global(value.get_type(), Some(AddressSpace::default()), &decl.name);
            global.set_initializer(&value);
        }

        Ok(())
    }

    fn generate_function_decl(&mut self, decl: &FunctionDecl) -> Result<(), String> {
        // Convert parameter types to LLVM types
        let param_types: Vec<BasicTypeEnum> = decl
            .params
            .iter()
            .map(|p| self.type_to_llvm(&p.type_annotation.clone().unwrap_or(Type::Num)))
            .collect::<Result<Vec<_>, _>>()?;

        // Convert return type
        let return_type = self.type_to_llvm(&decl.return_type.clone().unwrap_or(Type::Num))?;

        // Create function type - use a helper to convert BasicTypeEnum to BasicMetadataTypeEnum
        let fn_type = return_type.fn_type(
            &param_types
                .iter()
                .map(|t| (*t).into())
                .collect::<Vec<inkwell::types::BasicMetadataTypeEnum>>(),
            false,
        );

        // Create the function
        let function = self.module.add_function(&decl.name, fn_type, None);
        self.current_function = Some(function);

        // Create entry block
        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);

        // Store parameters in variables map
        self.variables.clear();
        for (i, param) in decl.params.iter().enumerate() {
            let llvm_param = function.get_nth_param(i as u32).unwrap();
            llvm_param.set_name(&param.name);

            // Allocate space for the parameter
            let param_type = llvm_param.as_basic_value_enum().get_type();
            let alloca = self.create_entry_block_alloca(&param.name, param_type)?;
            self.builder
                .build_store(alloca, llvm_param)
                .map_err(|e| format!("Failed to build store: {:?}", e))?;

            self.variables
                .insert(param.name.clone(), (alloca, param_type));
        }

        // Generate function body
        let body_value = self.generate_expr(&decl.body)?;
        self.builder
            .build_return(Some(&body_value))
            .map_err(|e| format!("Failed to build return: {:?}", e))?;

        Ok(())
    }

    fn create_entry_block_alloca(
        &self,
        name: &str,
        ty: BasicTypeEnum<'ctx>,
    ) -> Result<PointerValue<'ctx>, String> {
        let builder = self.context.create_builder();

        let entry = self
            .current_function
            .unwrap()
            .get_first_basic_block()
            .unwrap();
        match entry.get_first_instruction() {
            Some(first_instr) => builder.position_before(&first_instr),
            None => builder.position_at_end(entry),
        }

        builder
            .build_alloca(ty, name)
            .map_err(|e| format!("Failed to build alloca: {:?}", e))
    }

    fn generate_expr(&mut self, expr: &Expr) -> Result<BasicValueEnum<'ctx>, String> {
        match expr {
            Expr::Number { value, .. } => {
                // For now, use f64 for all numbers
                Ok(self.context.f64_type().const_float(*value).into())
            }

            Expr::String { value, .. } => {
                // Text is { ptr data, i64 byte_len }. `data` points at a global,
                // NUL-terminated C string (so `print` can treat it as a C string);
                // `byte_len` is the UTF-8 byte length, excluding the terminator.
                let global = self
                    .builder
                    .build_global_string_ptr(value, "str")
                    .map_err(|e| format!("Failed to build string: {:?}", e))?;
                let data_ptr = global.as_pointer_value();
                let len = self.context.i64_type().const_int(value.len() as u64, false);
                let text_ty = self.ptr_len_struct_type();
                let with_ptr = self
                    .builder
                    .build_insert_value(text_ty.get_undef(), data_ptr, 0, "text_ptr")
                    .map_err(|e| format!("Failed to insert text ptr: {:?}", e))?
                    .into_struct_value();
                let text = self
                    .builder
                    .build_insert_value(with_ptr, len, 1, "text_len")
                    .map_err(|e| format!("Failed to insert text len: {:?}", e))?
                    .into_struct_value();
                Ok(text.into())
            }

            Expr::Bool { value, .. } => Ok(self
                .context
                .bool_type()
                .const_int(*value as u64, false)
                .into()),

            Expr::Ident { name, .. } => {
                // Local binding (function-scoped alloca) first.
                if let Some((ptr, ty)) = self.variables.get(name) {
                    return self
                        .builder
                        .build_load(*ty, *ptr, name)
                        .map_err(|e| format!("Failed to build load: {:?}", e));
                }
                // Otherwise a top-level/module global constant (e.g. core.io's
                // `stdout`/`stderr`, or any top-level `name = <const>`).
                if let Some(global) = self.module.get_global(name) {
                    let ty = global
                        .get_initializer()
                        .map(|v| v.get_type())
                        .unwrap_or_else(|| self.context.f64_type().into());
                    return self
                        .builder
                        .build_load(ty, global.as_pointer_value(), name)
                        .map_err(|e| format!("Failed to build load global: {:?}", e));
                }
                Err(format!("Undefined variable: {}", name))
            }

            Expr::BinOp {
                left, op, right, ..
            } => self.generate_binop(left, *op, right),

            Expr::UnaryOp { op, expr, .. } => self.generate_unary_op(*op, expr),

            Expr::Call { func, args, .. } => self.generate_call(func, args),

            Expr::If {
                cond, then, else_, ..
            } => self.generate_if(cond, then, else_),

            Expr::Block { stmts, .. } => self.generate_block(stmts),

            Expr::Array { elements, .. } => self.generate_array(elements),

            Expr::Record { fields, .. } => self.generate_record(fields),

            Expr::Constructor {
                type_name: _,
                fields,
                ..
            } => {
                // Constructors have the same representation as records
                self.generate_record(fields)
            }

            Expr::FieldAccess { expr, field, .. } => self.generate_field_access(expr, field),

            Expr::Index { expr, index, .. } => self.generate_index(expr, index),

            Expr::Match { expr, arms, .. } => self.generate_match(expr, arms),

            Expr::ForLoop {
                collection,
                pattern,
                body,
                ..
            } => self.generate_for_loop(collection, pattern, body),

            // `left |> right` desugars to a call with `left` as the first arg
            // (must match the type checker's desugaring exactly).
            Expr::Pipeline { left, right, span } => {
                let call = Expr::desugar_pipeline(left, right, span);
                self.generate_expr(&call)
            }

            _ => Err(format!("Unsupported expression type: {:?}", expr)),
        }
    }

    fn generate_binop(
        &mut self,
        left: &Expr,
        op: BinOp,
        right: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let lhs = self.generate_expr(left)?;
        let rhs = self.generate_expr(right)?;

        match op {
            BinOp::Add => match (lhs, rhs) {
                (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) => Ok(self
                    .builder
                    .build_float_add(l, r, "addtmp")
                    .map_err(|e| format!("Failed to build add: {:?}", e))?
                    .into()),
                // Text + Text = concatenation (both are { ptr, i64 } structs).
                (BasicValueEnum::StructValue(l), BasicValueEnum::StructValue(r)) => {
                    self.generate_text_concat(l, r)
                }
                _ => Err("Add requires two Nums or two Texts".to_string()),
            },
            BinOp::Sub => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self
                        .builder
                        .build_float_sub(l, r, "subtmp")
                        .map_err(|e| format!("Failed to build sub: {:?}", e))?
                        .into())
                } else {
                    Err("Sub operation requires float values".to_string())
                }
            }
            BinOp::Mul => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self
                        .builder
                        .build_float_mul(l, r, "multmp")
                        .map_err(|e| format!("Failed to build mul: {:?}", e))?
                        .into())
                } else {
                    Err("Mul operation requires float values".to_string())
                }
            }
            BinOp::Div => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self
                        .builder
                        .build_float_div(l, r, "divtmp")
                        .map_err(|e| format!("Failed to build div: {:?}", e))?
                        .into())
                } else {
                    Err("Div operation requires float values".to_string())
                }
            }
            BinOp::Eq => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self
                        .builder
                        .build_float_compare(inkwell::FloatPredicate::OEQ, l, r, "eqtmp")
                        .map_err(|e| format!("Failed to build compare: {:?}", e))?
                        .into())
                } else {
                    Err("Eq operation requires float values".to_string())
                }
            }
            BinOp::Ne => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self
                        .builder
                        .build_float_compare(inkwell::FloatPredicate::ONE, l, r, "netmp")
                        .map_err(|e| format!("Failed to build compare: {:?}", e))?
                        .into())
                } else {
                    Err("Ne operation requires float values".to_string())
                }
            }
            BinOp::Lt => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self
                        .builder
                        .build_float_compare(inkwell::FloatPredicate::OLT, l, r, "lttmp")
                        .map_err(|e| format!("Failed to build compare: {:?}", e))?
                        .into())
                } else {
                    Err("Lt operation requires float values".to_string())
                }
            }
            BinOp::Le => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self
                        .builder
                        .build_float_compare(inkwell::FloatPredicate::OLE, l, r, "letmp")
                        .map_err(|e| format!("Failed to build compare: {:?}", e))?
                        .into())
                } else {
                    Err("Le operation requires float values".to_string())
                }
            }
            BinOp::Gt => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self
                        .builder
                        .build_float_compare(inkwell::FloatPredicate::OGT, l, r, "gttmp")
                        .map_err(|e| format!("Failed to build compare: {:?}", e))?
                        .into())
                } else {
                    Err("Gt operation requires float values".to_string())
                }
            }
            BinOp::Ge => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self
                        .builder
                        .build_float_compare(inkwell::FloatPredicate::OGE, l, r, "getmp")
                        .map_err(|e| format!("Failed to build compare: {:?}", e))?
                        .into())
                } else {
                    Err("Ge operation requires float values".to_string())
                }
            }
            BinOp::And => {
                // Logical AND with short-circuit evaluation
                // Convert operands to boolean (i1)
                let lhs_bool = self.value_to_boolean(lhs)?;
                let rhs_bool = self.value_to_boolean(rhs)?;

                // Use LLVM's 'and' instruction
                Ok(self
                    .builder
                    .build_and(lhs_bool, rhs_bool, "andtmp")
                    .map_err(|e| format!("Failed to build and: {:?}", e))?
                    .into())
            }
            BinOp::Or => {
                // Logical OR with short-circuit evaluation
                // Convert operands to boolean (i1)
                let lhs_bool = self.value_to_boolean(lhs)?;
                let rhs_bool = self.value_to_boolean(rhs)?;

                // Use LLVM's 'or' instruction
                Ok(self
                    .builder
                    .build_or(lhs_bool, rhs_bool, "ortmp")
                    .map_err(|e| format!("Failed to build or: {:?}", e))?
                    .into())
            }
            _ => Err(format!("Unsupported binary operation: {:?}", op)),
        }
    }

    // Helper to convert a value to boolean (i1)
    fn value_to_boolean(
        &mut self,
        value: BasicValueEnum<'ctx>,
    ) -> Result<inkwell::values::IntValue<'ctx>, String> {
        match value {
            BasicValueEnum::IntValue(i) => {
                // Already an int - check if it's i1
                if i.get_type().get_bit_width() == 1 {
                    Ok(i)
                } else {
                    // Convert to i1 by comparing with 0
                    self.builder
                        .build_int_compare(
                            inkwell::IntPredicate::NE,
                            i,
                            i.get_type().const_zero(),
                            "tobool",
                        )
                        .map_err(|e| format!("Failed to convert to bool: {:?}", e))
                }
            }
            BasicValueEnum::FloatValue(f) => {
                // Convert float to bool by comparing with 0.0
                self.builder
                    .build_float_compare(
                        inkwell::FloatPredicate::ONE, // Ordered Not Equal
                        f,
                        f.get_type().const_zero(),
                        "tobool",
                    )
                    .map_err(|e| format!("Failed to convert float to bool: {:?}", e))
            }
            _ => Err("Cannot convert value to boolean".to_string()),
        }
    }

    fn generate_unary_op(
        &mut self,
        op: UnaryOp,
        expr: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let val = self.generate_expr(expr)?;

        match op {
            UnaryOp::Neg => {
                if let BasicValueEnum::FloatValue(f) = val {
                    Ok(self
                        .builder
                        .build_float_neg(f, "negtmp")
                        .map_err(|e| format!("Failed to build neg: {:?}", e))?
                        .into())
                } else {
                    Err("Neg operation requires float value".to_string())
                }
            }
            UnaryOp::Not => {
                if let BasicValueEnum::IntValue(i) = val {
                    Ok(self
                        .builder
                        .build_not(i, "nottmp")
                        .map_err(|e| format!("Failed to build not: {:?}", e))?
                        .into())
                } else {
                    Err("Not operation requires int value".to_string())
                }
            }
        }
    }

    /// Declare (once) and return an external runtime intrinsic by its
    /// Quilon-internal name. These resolve to `#[no_mangle]` symbols in
    /// `src/runtime/intrinsics.rs` (or libc, e.g. `memcpy`) — available both to
    /// the in-process JIT and to AOT-linked executables.
    fn get_intrinsic(&self, name: &str) -> Result<FunctionValue<'ctx>, String> {
        if let Some(f) = self.module.get_function(name) {
            return Ok(f);
        }
        let ctx = self.context;
        let ptr = ctx.ptr_type(AddressSpace::default());
        let i64t = ctx.i64_type();
        let f64t = ctx.f64_type();
        let void = ctx.void_type();
        let fn_type = match name {
            // i8* __alloc(i64) — GC-managed allocation.
            "__alloc" => ptr.fn_type(&[i64t.into()], false),
            // void __gc_init() — initialize the Boehm GC.
            "__gc_init" => void.fn_type(&[], false),
            // i8* memcpy(i8*, i8*, i64) — libc.
            "memcpy" => ptr.fn_type(&[ptr.into(), ptr.into(), i64t.into()], false),
            // i64 __text_length(i8*, i64) — grapheme-cluster count.
            "__text_length" => i64t.fn_type(&[ptr.into(), i64t.into()], false),
            // i64 __write_bytes(i64 fd, i8* ptr, i64 len) — raw write, backs `write`.
            "__write_bytes" => i64t.fn_type(&[i64t.into(), ptr.into(), i64t.into()], false),
            // void __print_num_fd(i64 fd, double) — number + newline to fd.
            "__print_num_fd" => void.fn_type(&[i64t.into(), f64t.into()], false),
            // void __print_bool_fd(i64 fd, i64 b) — "true"/"false" + newline to fd.
            "__print_bool_fd" => void.fn_type(&[i64t.into(), i64t.into()], false),
            // void __print_text_fd(i64 fd, i8*) — C string + newline to fd.
            "__print_text_fd" => void.fn_type(&[i64t.into(), ptr.into()], false),
            other => return Err(format!("Unknown runtime intrinsic: {}", other)),
        };
        Ok(self.module.add_function(name, fn_type, None))
    }

    /// Lower a `print`/`eprint` builtin call: render the single argument's text
    /// and write it, followed by a newline, to stdout (`print`, fd 1) or stderr
    /// (`eprint`, fd 2). Dispatches on the LLVM type of the argument: floats print
    /// as numbers, Text structs / pointers as C strings, integers (incl. bools)
    /// widen to numbers. Yields `Num` 0, so it is usable in expression position.
    fn generate_print(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if args.len() != 1 {
            return Err(format!(
                "{} expects exactly 1 argument, got {}",
                name,
                args.len()
            ));
        }
        let fd = if name == "eprint" { 2 } else { 1 };
        let fd_val = self.context.i64_type().const_int(fd, false);
        let val = self.generate_expr(&args[0])?;
        let (intrinsic, arg): (&str, inkwell::values::BasicMetadataValueEnum) = match val {
            BasicValueEnum::FloatValue(f) => ("__print_num_fd", f.into()),
            // Text is { ptr data, i64 len }; print its NUL-terminated `data`.
            BasicValueEnum::StructValue(s) => {
                let data = self
                    .builder
                    .build_extract_value(s, 0, "text_data")
                    .map_err(|e| format!("Failed to extract text data: {:?}", e))?
                    .into_pointer_value();
                ("__print_text_fd", data.into())
            }
            // A bare pointer (C string) prints as text.
            BasicValueEnum::PointerValue(p) => ("__print_text_fd", p.into()),
            // A Bool (i1) prints as "true"/"false"; any wider int widens to a number.
            BasicValueEnum::IntValue(i) if i.get_type().get_bit_width() == 1 => {
                let b = self
                    .builder
                    .build_int_z_extend(i, self.context.i64_type(), "bool_ext")
                    .map_err(|e| format!("Failed to extend bool for print: {:?}", e))?;
                ("__print_bool_fd", b.into())
            }
            BasicValueEnum::IntValue(i) => {
                let f = self
                    .builder
                    .build_unsigned_int_to_float(i, self.context.f64_type(), "print_num")
                    .map_err(|e| format!("Failed to convert int for print: {:?}", e))?;
                ("__print_num_fd", f.into())
            }
            other => {
                return Err(format!(
                    "print does not support a value of type {:?}",
                    other.get_type()
                ));
            }
        };
        let print_fn = self.get_intrinsic(intrinsic)?;
        self.builder
            .build_call(print_fn, &[fd_val.into(), arg], "")
            .map_err(|e| format!("Failed to build print call: {:?}", e))?;
        Ok(self.context.f64_type().const_float(0.0).into())
    }

    /// Lower the `write(content, fd)` builtin: write the raw bytes of a `Text`
    /// `content` to file descriptor `fd` (a `Num`), with no trailing newline.
    /// Yields `Num` (bytes written).
    fn generate_write(&mut self, args: &[Expr]) -> Result<BasicValueEnum<'ctx>, String> {
        if args.len() != 2 {
            return Err(format!(
                "write expects exactly 2 arguments (content, fd), got {}",
                args.len()
            ));
        }
        let content = self.generate_expr(&args[0])?;
        let fd_num = self.generate_expr(&args[1])?;
        // content must be a Text { ptr data, i64 byte_len }.
        let s = match content {
            BasicValueEnum::StructValue(s) => s,
            other => {
                return Err(format!(
                    "write expects a Text content, got {:?}",
                    other.get_type()
                ));
            }
        };
        let data = self
            .builder
            .build_extract_value(s, 0, "write_data")
            .map_err(|e| format!("Failed to extract text data: {:?}", e))?
            .into_pointer_value();
        let len = self
            .builder
            .build_extract_value(s, 1, "write_len")
            .map_err(|e| format!("Failed to extract text len: {:?}", e))?
            .into_int_value();
        let fd_float = match fd_num {
            BasicValueEnum::FloatValue(f) => f,
            other => {
                return Err(format!(
                    "write expects a Num fd, got {:?}",
                    other.get_type()
                ));
            }
        };
        let fd_i64 = self
            .builder
            .build_float_to_signed_int(fd_float, self.context.i64_type(), "write_fd")
            .map_err(|e| format!("Failed to convert fd: {:?}", e))?;
        let write_fn = self.get_intrinsic("__write_bytes")?;
        use inkwell::values::AnyValue;
        let written = self
            .builder
            .build_call(
                write_fn,
                &[fd_i64.into(), data.into(), len.into()],
                "write_n",
            )
            .map_err(|e| format!("Failed to call __write_bytes: {:?}", e))?
            .as_any_value_enum()
            .into_int_value();
        Ok(self
            .builder
            .build_signed_int_to_float(written, self.context.f64_type(), "write_ret")
            .map_err(|e| format!("Failed to convert write result: {:?}", e))?
            .into())
    }

    fn generate_call(
        &mut self,
        func: &Expr,
        args: &[Expr],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // Get function name - only support direct calls for now
        let func_name = if let Expr::Ident { name, .. } = func {
            name
        } else {
            return Err("Only direct function calls supported".to_string());
        };

        // Core IO builtins, lowered to runtime intrinsics (see runtime::intrinsics).
        match func_name.as_str() {
            "print" | "eprint" => return self.generate_print(func_name, args),
            "write" => return self.generate_write(args),
            _ => {}
        }

        // Check if this is a sum type constructor (Ok, NotOk, etc.)
        // For now, hardcode the builtin Result constructors
        match func_name.as_str() {
            "Ok" => return self.generate_sum_constructor(0, args), // Tag 0 for Ok
            "NotOk" => return self.generate_sum_constructor(1, args), // Tag 1 for NotOk
            _ => {}
        }

        // Get the function from the module. If there is no plain top-level function with this
        // name, it may be a method call: the parser desugars `recv.method(a, b)` to
        // `method(recv, a, b)`, so resolve `recv`'s named type and dispatch to `Type_method`.
        let function = match self.module.get_function(func_name) {
            Some(f) => f,
            None => {
                let mangled = args
                    .first()
                    .and_then(|recv| self.receiver_type_name(recv))
                    .map(|type_name| format!("{}_{}", type_name, func_name));
                match mangled.and_then(|m| self.module.get_function(&m)) {
                    Some(f) => f,
                    None => return Err(format!("Function not found: {}", func_name)),
                }
            }
        };

        // Generate argument values
        let arg_values: Vec<BasicValueEnum> = args
            .iter()
            .map(|arg| self.generate_expr(arg))
            .collect::<Result<Vec<_>, _>>()?;

        // Convert to BasicMetadataValueEnum for the call
        let arg_metadata: Vec<inkwell::values::BasicMetadataValueEnum> =
            arg_values.iter().map(|v| (*v).into()).collect();

        // Build the call
        let call_site = self
            .builder
            .build_call(function, &arg_metadata, "calltmp")
            .map_err(|e| format!("Failed to build call: {:?}", e))?;

        // In Inkwell 0.8, try_as_basic_value returns a special ValueKind enum
        // We need to pattern match on it, but since it's an opaque type,
        // let's just use into_basic_value which works for functions that return values
        // For now, we'll unsafely assume all functions return values
        use inkwell::values::AnyValue;
        let any_val = call_site.as_any_value_enum();

        // Convert AnyValueEnum to BasicValueEnum
        match any_val {
            inkwell::values::AnyValueEnum::IntValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::FloatValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::PointerValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::ArrayValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::StructValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::VectorValue(v) => Ok(v.into()),
            _ => Err("Function does not return a basic value".to_string()),
        }
    }

    /// Resolve the named record type of a method-call receiver, if known. Handles both a
    /// variable holding a constructed instance and a constructor expression used directly.
    fn receiver_type_name(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Ident { name, .. } => self.var_named_types.get(name).cloned(),
            Expr::Constructor { type_name, .. } => Some(type_name.clone()),
            _ => None,
        }
    }

    fn generate_sum_constructor(
        &mut self,
        tag: u8,
        args: &[Expr],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // Tagged-union layout: { i8 tag, <payload> }.
        // The payload field is typed as the actual payload value's LLVM type:
        //   Ok(42)        -> { i8 0, double 42.0 }
        //   NotOk("err")  -> { i8 1, ptr <str> }
        // Numeric payloads are normalized to f64 so the canonical Result type
        // (see `type_to_llvm` for `Type::Sum`) is { i8, double }. Non-numeric
        // payloads (pointers, structs) are stored at their real type without
        // coercion. This fixes the previous bug where every payload was forced
        // to f64, so `NotOk("error")` silently became `{ i8 1, double 0.0 }`.
        //
        // NOTE: a single Result *value* therefore carries one concrete payload
        // type. Unifying different payload types behind one value (e.g. a
        // branch yielding `Ok(num)` or `NotOk(text)`, or a function returning a
        // non-numeric Result) needs a sized union and is deferred — the
        // canonical return-position representation here is numeric.
        let i8_type = self.context.i8_type();
        let f64_type = self.context.f64_type();

        // Create the tag value
        let tag_val = i8_type.const_int(tag as u64, false);

        // Generate the payload value at its real type (first argument, or 0.0
        // if the variant has no payload), normalizing integers to f64.
        let payload_val: BasicValueEnum = if let Some(arg) = args.first() {
            let arg_val = self.generate_expr(arg)?;
            match arg_val {
                BasicValueEnum::IntValue(i) => self
                    .builder
                    .build_unsigned_int_to_float(i, f64_type, "inttofloat")
                    .map_err(|e| format!("Failed to convert int to float: {:?}", e))?
                    .into(),
                other => other,
            }
        } else {
            f64_type.const_float(0.0).into()
        };

        // Build the Result struct sized to the payload's real LLVM type.
        let result_struct = self
            .context
            .struct_type(&[i8_type.into(), payload_val.get_type()], false);
        let undef = result_struct.get_undef();
        let with_tag = self
            .builder
            .build_insert_value(undef, tag_val, 0, "with_tag")
            .map_err(|e| format!("Failed to insert tag: {:?}", e))?
            .into_struct_value();
        let with_payload = self
            .builder
            .build_insert_value(with_tag, payload_val, 1, "with_payload")
            .map_err(|e| format!("Failed to insert payload: {:?}", e))?
            .into_struct_value();

        Ok(with_payload.into())
    }

    fn generate_if(
        &mut self,
        cond: &Expr,
        then_expr: &Expr,
        else_expr: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let cond_val = self.generate_expr(cond)?;

        let cond_bool = if let BasicValueEnum::IntValue(i) = cond_val {
            i
        } else {
            return Err("Condition must be a boolean".to_string());
        };

        let function = self
            .current_function
            .ok_or_else(|| "If expression outside of function".to_string())?;

        // Create blocks
        let then_bb = self.context.append_basic_block(function, "then");
        let else_bb = self.context.append_basic_block(function, "else");
        let merge_bb = self.context.append_basic_block(function, "ifcont");

        // Build conditional branch
        self.builder
            .build_conditional_branch(cond_bool, then_bb, else_bb)
            .map_err(|e| format!("Failed to build conditional branch: {:?}", e))?;

        // Generate then block
        self.builder.position_at_end(then_bb);
        let then_val = self.generate_expr(then_expr)?;
        self.builder
            .build_unconditional_branch(merge_bb)
            .map_err(|e| format!("Failed to build branch: {:?}", e))?;
        let then_bb = self.builder.get_insert_block().unwrap();

        // Generate else block
        self.builder.position_at_end(else_bb);
        let else_val = self.generate_expr(else_expr)?;
        self.builder
            .build_unconditional_branch(merge_bb)
            .map_err(|e| format!("Failed to build branch: {:?}", e))?;
        let else_bb = self.builder.get_insert_block().unwrap();

        // Generate merge block
        self.builder.position_at_end(merge_bb);
        let phi = self
            .builder
            .build_phi(then_val.get_type(), "iftmp")
            .map_err(|e| format!("Failed to build phi: {:?}", e))?;
        phi.add_incoming(&[(&then_val, then_bb), (&else_val, else_bb)]);

        Ok(phi.as_basic_value())
    }

    fn generate_block(
        &mut self,
        stmts: &[crate::ast::Statement],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let mut result = self.context.f64_type().const_float(0.0).into();

        for stmt in stmts {
            match stmt {
                crate::ast::Statement::Item(item) => {
                    self.generate_item(item)?;
                }
                crate::ast::Statement::Expr(expr) => {
                    result = self.generate_expr(expr)?;
                }
            }
        }

        Ok(result)
    }

    fn generate_array(&mut self, elements: &[Expr]) -> Result<BasicValueEnum<'ctx>, String> {
        // Arrays are represented as structs: { ptr data, i64 size }
        // This allows .size field access

        if self.current_function.is_none() {
            return Err("Global arrays not yet implemented".to_string());
        }

        let size = elements.len();

        if size == 0 {
            // Empty array - create struct with null ptr and size 0
            let ptr_type = self.context.ptr_type(AddressSpace::default());
            let i64_type = self.context.i64_type();
            let array_struct_type = self
                .context
                .struct_type(&[ptr_type.into(), i64_type.into()], false);

            let null_ptr = ptr_type.const_zero();
            let zero_size = i64_type.const_zero();

            return Ok(array_struct_type
                .const_named_struct(&[null_ptr.into(), zero_size.into()])
                .into());
        }

        // Generate all element values
        let values: Vec<BasicValueEnum> = elements
            .iter()
            .map(|e| self.generate_expr(e))
            .collect::<Result<Vec<_>, _>>()?;

        // Get element type from first element
        let elem_type = values[0].get_type();

        // Allocate array storage
        let array_type = elem_type.array_type(size as u32);
        let array_alloca = self
            .builder
            .build_alloca(array_type, "array_data")
            .map_err(|e| format!("Failed to allocate array: {:?}", e))?;

        // Store each element
        for (i, value) in values.iter().enumerate() {
            let index = self.context.i32_type().const_int(i as u64, false);
            let gep = unsafe {
                self.builder
                    .build_gep(
                        array_type,
                        array_alloca,
                        &[self.context.i32_type().const_zero(), index],
                        &format!("elem_{}", i),
                    )
                    .map_err(|e| format!("Failed to build GEP: {:?}", e))?
            };
            self.builder
                .build_store(gep, *value)
                .map_err(|e| format!("Failed to store element: {:?}", e))?;
        }

        // Create the array struct { ptr, size }
        let ptr_type = self.context.ptr_type(AddressSpace::default());
        let i64_type = self.context.i64_type();
        let array_struct_type = self
            .context
            .struct_type(&[ptr_type.into(), i64_type.into()], false);

        let array_struct = self
            .builder
            .build_alloca(array_struct_type, "array")
            .map_err(|e| format!("Failed to allocate array struct: {:?}", e))?;

        // Store pointer to data in field 0
        let ptr_field = self
            .builder
            .build_struct_gep(array_struct_type, array_struct, 0, "array_ptr_field")
            .map_err(|e| format!("Failed to get ptr field: {:?}", e))?;

        self.builder
            .build_store(ptr_field, array_alloca)
            .map_err(|e| format!("Failed to store ptr: {:?}", e))?;

        // Store size in field 1
        let size_field = self
            .builder
            .build_struct_gep(array_struct_type, array_struct, 1, "array_size_field")
            .map_err(|e| format!("Failed to get size field: {:?}", e))?;

        let size_value = i64_type.const_int(size as u64, false);
        self.builder
            .build_store(size_field, size_value)
            .map_err(|e| format!("Failed to store size: {:?}", e))?;

        // Load and return the struct
        self.builder
            .build_load(array_struct_type, array_struct, "array")
            .map_err(|e| format!("Failed to load array struct: {:?}", e))
    }

    fn generate_record(
        &mut self,
        fields: &[(String, Expr)],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if fields.is_empty() {
            // Empty record - create empty struct
            let struct_type = self.context.struct_type(&[], false);
            return Ok(struct_type.const_zero().into());
        }

        // Generate all field values
        let mut field_values: Vec<BasicValueEnum> = Vec::new();
        for (_name, expr) in fields {
            field_values.push(self.generate_expr(expr)?);
        }

        // Get field types
        let field_types: Vec<BasicTypeEnum> = field_values.iter().map(|v| v.get_type()).collect();

        // Create struct type
        let struct_type = self.context.struct_type(&field_types, false);

        // Create the struct value
        if self.current_function.is_some() {
            // Allocate space for the struct
            let alloca = self
                .builder
                .build_alloca(struct_type, "record")
                .map_err(|e| format!("Failed to build alloca: {:?}", e))?;

            // Store each field
            for (i, value) in field_values.iter().enumerate() {
                let gep = self
                    .builder
                    .build_struct_gep(struct_type, alloca, i as u32, &format!("field_{}", i))
                    .map_err(|e| format!("Failed to build GEP: {:?}", e))?;
                self.builder
                    .build_store(gep, *value)
                    .map_err(|e| format!("Failed to build store: {:?}", e))?;
            }

            Ok(alloca.into())
        } else {
            // For globals, we need constant values
            Err("Global records not yet implemented".to_string())
        }
    }

    /// Concatenate two `Text` values into a fresh, GC-allocated, NUL-terminated
    /// buffer and return a new `{ ptr, byte_len }` struct.
    fn generate_text_concat(
        &mut self,
        left: inkwell::values::StructValue<'ctx>,
        right: inkwell::values::StructValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let i8t = self.context.i8_type();
        let i64t = self.context.i64_type();

        let field = |s: inkwell::values::StructValue<'ctx>,
                     idx: u32,
                     name: &str|
         -> Result<BasicValueEnum<'ctx>, String> {
            self.builder
                .build_extract_value(s, idx, name)
                .map_err(|e| format!("Failed to extract text field: {:?}", e))
        };
        let l_ptr = field(left, 0, "l_ptr")?.into_pointer_value();
        let l_len = field(left, 1, "l_len")?.into_int_value();
        let r_ptr = field(right, 0, "r_ptr")?.into_pointer_value();
        let r_len = field(right, 1, "r_len")?.into_int_value();

        let total = self
            .builder
            .build_int_add(l_len, r_len, "concat_len")
            .map_err(|e| format!("Failed to add lengths: {:?}", e))?;
        // +1 byte for the NUL terminator so the result is also a valid C string.
        let alloc_size = self
            .builder
            .build_int_add(total, i64t.const_int(1, false), "concat_alloc")
            .map_err(|e| format!("Failed to size alloc: {:?}", e))?;

        use inkwell::values::AnyValue;
        let alloc_fn = self.get_intrinsic("__alloc")?;
        let dest = self
            .builder
            .build_call(alloc_fn, &[alloc_size.into()], "concat_buf")
            .map_err(|e| format!("Failed to call __alloc: {:?}", e))?
            .as_any_value_enum()
            .into_pointer_value();

        let memcpy_fn = self.get_intrinsic("memcpy")?;
        self.builder
            .build_call(memcpy_fn, &[dest.into(), l_ptr.into(), l_len.into()], "")
            .map_err(|e| format!("Failed to copy left text: {:?}", e))?;
        let tail = unsafe {
            self.builder
                .build_gep(i8t, dest, &[l_len], "concat_tail")
                .map_err(|e| format!("Failed to offset into buffer: {:?}", e))?
        };
        self.builder
            .build_call(memcpy_fn, &[tail.into(), r_ptr.into(), r_len.into()], "")
            .map_err(|e| format!("Failed to copy right text: {:?}", e))?;
        let nul = unsafe {
            self.builder
                .build_gep(i8t, dest, &[total], "concat_nul")
                .map_err(|e| format!("Failed to offset NUL: {:?}", e))?
        };
        self.builder
            .build_store(nul, i8t.const_zero())
            .map_err(|e| format!("Failed to write NUL: {:?}", e))?;

        let text_ty = self.ptr_len_struct_type();
        let with_ptr = self
            .builder
            .build_insert_value(text_ty.get_undef(), dest, 0, "cat_ptr")
            .map_err(|e| format!("Failed to insert concat ptr: {:?}", e))?
            .into_struct_value();
        let text = self
            .builder
            .build_insert_value(with_ptr, total, 1, "cat_len")
            .map_err(|e| format!("Failed to insert concat len: {:?}", e))?
            .into_struct_value();
        Ok(text.into())
    }

    fn generate_field_access(
        &mut self,
        expr: &Expr,
        field_name: &str,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // A record may legitimately have a field literally named `size`/`length`.
        // Resolve known record fields by NAME first (matching the type checker,
        // which dispatches on static type) so they don't collide with the Text/array
        // `.size`/`.length` struct-shape handling below. Text/array values are never
        // tracked in `record_types`, so this only diverts genuine record fields.
        let is_named_record_field = matches!(expr, Expr::Ident { name, .. }
            if self
                .record_types
                .get(name)
                .is_some_and(|fields| fields.iter().any(|f| f == field_name)));

        // Special handling for .size field on arrays
        if !is_named_record_field && field_name == "size" {
            // For arrays (which are structs {ptr, i64}), we need special handling
            // Check if it's an identifier - we can directly work with the alloca
            if let Expr::Ident { name, .. } = expr
                && let Some((var_ptr, var_type)) = self.variables.get(name).cloned()
            {
                // Check if this is a struct type (could be an array)
                if let BasicTypeEnum::StructType(struct_type) = var_type {
                    // Get field 1 (size field of array struct) directly from the alloca
                    let size_field = self
                        .builder
                        .build_struct_gep(struct_type, var_ptr, 1, "size_field")
                        .map_err(|e| format!("Failed to get size field: {:?}", e))?;

                    let size_val = self
                        .builder
                        .build_load(self.context.i64_type(), size_field, "size")
                        .map_err(|e| format!("Failed to load size: {:?}", e))?;

                    // Convert i64 to f64 (Num)
                    if let BasicValueEnum::IntValue(i) = size_val {
                        let size_f64 = self
                            .builder
                            .build_signed_int_to_float(i, self.context.f64_type(), "size_as_num")
                            .map_err(|e| format!("Failed to convert size: {:?}", e))?;

                        return Ok(size_f64.into());
                    }
                }
            }
        }

        // Text/array as a value: `.size` is the i64 length field (byte length for
        // Text); `.length` is the grapheme count (Text only — the checker rejects
        // `.length` on arrays). Handles non-identifier receivers like `("a"+"b").size`.
        if !is_named_record_field && (field_name == "size" || field_name == "length") {
            let val = self.generate_expr(expr)?;
            if let BasicValueEnum::StructValue(s) = val {
                let len = self
                    .builder
                    .build_extract_value(s, 1, "len_field")
                    .map_err(|e| format!("Failed to extract length field: {:?}", e))?
                    .into_int_value();
                if field_name == "size" {
                    return Ok(self
                        .builder
                        .build_signed_int_to_float(len, self.context.f64_type(), "size_as_num")
                        .map_err(|e| format!("Failed to convert size: {:?}", e))?
                        .into());
                }
                // `.length`: grapheme-cluster count via __text_length(data, byte_len).
                let data = self
                    .builder
                    .build_extract_value(s, 0, "data_field")
                    .map_err(|e| format!("Failed to extract data field: {:?}", e))?
                    .into_pointer_value();
                let len_fn = self.get_intrinsic("__text_length")?;
                use inkwell::values::AnyValue;
                let count = self
                    .builder
                    .build_call(len_fn, &[data.into(), len.into()], "graphemes")
                    .map_err(|e| format!("Failed to call __text_length: {:?}", e))?
                    .as_any_value_enum()
                    .into_int_value();
                return Ok(self
                    .builder
                    .build_signed_int_to_float(count, self.context.f64_type(), "length_as_num")
                    .map_err(|e| format!("Failed to convert length: {:?}", e))?
                    .into());
            }
        }

        // For regular record field access
        // Special case: if expr is an identifier, check if we have record info
        if let Expr::Ident { name, .. } = expr
            && let Some(field_names) = self.record_types.get(name)
        {
            // Find the field index
            if let Some(field_idx) = field_names.iter().position(|f| f == field_name) {
                // Get the variable pointer
                let (var_ptr, _var_type) = self
                    .variables
                    .get(name)
                    .ok_or_else(|| format!("Variable not found: {}", name))?;

                // The variable holds a pointer to the struct
                // Load it to get the actual struct pointer
                let struct_ptr = self
                    .builder
                    .build_load(
                        self.context.ptr_type(AddressSpace::default()),
                        *var_ptr,
                        "load_struct_ptr",
                    )
                    .map_err(|e| format!("Failed to load struct pointer: {:?}", e))?
                    .into_pointer_value();

                // Now we need the struct type for GEP
                // We need to reconstruct the struct type from field count
                // For now, assume all fields are f64 (this is a limitation)
                let field_types: Vec<BasicTypeEnum> =
                    vec![self.context.f64_type().into(); field_names.len()];
                let struct_type = self.context.struct_type(&field_types, false);

                // Use GEP to get field pointer
                let field_ptr = self
                    .builder
                    .build_struct_gep(
                        struct_type,
                        struct_ptr,
                        field_idx as u32,
                        &format!("field_{}", field_name),
                    )
                    .map_err(|e| format!("Failed to build GEP: {:?}", e))?;

                // Load the field value
                let field_val = self
                    .builder
                    .build_load(self.context.f64_type(), field_ptr, field_name)
                    .map_err(|e| format!("Failed to load field: {:?}", e))?;

                return Ok(field_val);
            }
        }

        Err(format!(
            "Field access not fully implemented. Need type information for field '{}'",
            field_name
        ))
    }

    fn generate_index(
        &mut self,
        expr: &Expr,
        index_expr: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // Generate the array expression
        let array_val = self.generate_expr(expr)?;

        // Generate the index expression
        let index_val = self.generate_expr(index_expr)?;

        // Array is a struct { ptr data, i64 size }
        // We need to:
        // 1. Extract the data pointer (field 0)
        // 2. Convert index from f64 to i64
        // 3. Use GEP to get element pointer
        // 4. Load the element

        if let BasicValueEnum::StructValue(struct_val) = array_val {
            // Store struct temporarily to access fields
            let struct_type = struct_val.get_type();
            let alloca = self
                .builder
                .build_alloca(struct_type, "temp_array")
                .map_err(|e| format!("Failed to allocate temp: {:?}", e))?;

            self.builder
                .build_store(alloca, struct_val)
                .map_err(|e| format!("Failed to store array: {:?}", e))?;

            // Get data pointer (field 0)
            let data_field = self
                .builder
                .build_struct_gep(struct_type, alloca, 0, "data_ptr_field")
                .map_err(|e| format!("Failed to get data field: {:?}", e))?;

            let data_ptr = self
                .builder
                .build_load(
                    self.context.ptr_type(AddressSpace::default()),
                    data_field,
                    "data_ptr",
                )
                .map_err(|e| format!("Failed to load data ptr: {:?}", e))?
                .into_pointer_value();

            // Convert index from f64 to i64
            let index_i64 = if let BasicValueEnum::FloatValue(f) = index_val {
                self.builder
                    .build_float_to_signed_int(f, self.context.i64_type(), "index_i64")
                    .map_err(|e| format!("Failed to convert index: {:?}", e))?
            } else {
                return Err("Index must be a number".to_string());
            };

            // Use GEP to get element pointer
            // For now, assume elements are f64
            let elem_ptr = unsafe {
                self.builder
                    .build_gep(self.context.f64_type(), data_ptr, &[index_i64], "elem_ptr")
                    .map_err(|e| format!("Failed to build GEP: {:?}", e))?
            };

            // Load the element
            self.builder
                .build_load(self.context.f64_type(), elem_ptr, "elem")
                .map_err(|e| format!("Failed to load element: {:?}", e))
        } else {
            Err("Can only index into arrays".to_string())
        }
    }

    fn generate_match(
        &mut self,
        expr: &Expr,
        arms: &[MatchArm],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // For now, implement a simplified version that only handles constructor patterns
        // and wildcards for Option-like types

        // Evaluate the expression being matched
        let match_val = self.generate_expr(expr)?;

        // Get the current function
        let function = self
            .current_function
            .ok_or_else(|| "Match expression must be in a function".to_string())?;

        // Create basic blocks for each arm and a continuation block
        let mut arm_blocks = vec![];
        let mut check_blocks = vec![];
        for i in 0..arms.len() {
            check_blocks.push(
                self.context
                    .append_basic_block(function, &format!("check_{}", i)),
            );
            arm_blocks.push(
                self.context
                    .append_basic_block(function, &format!("arm_{}", i)),
            );
        }
        let cont_block = self.context.append_basic_block(function, "match_cont");

        // Create a phi node to collect results from all arms
        // We need to determine the result type - for now assume f64
        let result_alloca =
            self.create_entry_block_alloca("match_result", self.context.f64_type().into())?;

        // Jump to first check
        self.builder
            .build_unconditional_branch(check_blocks[0])
            .map_err(|e| format!("Failed to build branch: {:?}", e))?;

        // Generate code for each arm
        for (i, arm) in arms.iter().enumerate() {
            // Position at check block
            self.builder.position_at_end(check_blocks[i]);

            // Check if pattern matches
            let matches = self.check_pattern(&arm.pattern, match_val)?;

            // Conditional branch to arm or next check
            let next_block = if i + 1 < check_blocks.len() {
                check_blocks[i + 1]
            } else {
                // Last arm - if it doesn't match, it's an error
                // For now, just go to continuation with a default value
                cont_block
            };

            self.builder
                .build_conditional_branch(matches, arm_blocks[i], next_block)
                .map_err(|e| format!("Failed to build conditional branch: {:?}", e))?;

            // Generate arm body
            self.builder.position_at_end(arm_blocks[i]);

            // Bind pattern variables
            self.bind_pattern(&arm.pattern, match_val)?;

            let arm_val = self.generate_expr(&arm.body)?;
            self.builder
                .build_store(result_alloca, arm_val)
                .map_err(|e| format!("Failed to store result: {:?}", e))?;

            self.builder
                .build_unconditional_branch(cont_block)
                .map_err(|e| format!("Failed to build branch: {:?}", e))?;
        }

        // Position at continuation block
        self.builder.position_at_end(cont_block);

        // Load the result
        self.builder
            .build_load(self.context.f64_type(), result_alloca, "match_result")
            .map_err(|e| format!("Failed to load result: {:?}", e))
    }

    fn check_pattern(
        &mut self,
        pattern: &Pattern,
        value: BasicValueEnum<'ctx>,
    ) -> Result<inkwell::values::IntValue<'ctx>, String> {
        match pattern {
            Pattern::Wildcard { .. } => {
                // Wildcard always matches
                Ok(self.context.bool_type().const_all_ones())
            }

            Pattern::Ident { .. } => {
                // Identifier pattern always matches (binds the value)
                Ok(self.context.bool_type().const_all_ones())
            }

            Pattern::Number { value: num_val, .. } => {
                // Compare the value
                if let BasicValueEnum::FloatValue(fval) = value {
                    let const_val = self.context.f64_type().const_float(*num_val);
                    self.builder
                        .build_float_compare(
                            inkwell::FloatPredicate::OEQ,
                            fval,
                            const_val,
                            "num_match",
                        )
                        .map_err(|e| format!("Failed to build comparison: {:?}", e))
                } else {
                    Ok(self.context.bool_type().const_zero())
                }
            }

            Pattern::Constructor { name, args: _, .. } => {
                // For constructors, we need to check the discriminant
                // Result is represented as { i8 tag, f64 payload }
                // Ok = tag 0, NotOk = tag 1

                let expected_tag = match name.as_str() {
                    "Ok" => 0u8,
                    "NotOk" => 1u8,
                    _ => return Err(format!("Unknown constructor: {}", name)),
                };

                // Extract tag from struct (field 0)
                if let BasicValueEnum::StructValue(struct_val) = value {
                    let tag_val = self
                        .builder
                        .build_extract_value(struct_val, 0, "tag")
                        .map_err(|e| format!("Failed to extract tag: {:?}", e))?
                        .into_int_value();

                    let expected_tag_val =
                        self.context.i8_type().const_int(expected_tag as u64, false);

                    self.builder
                        .build_int_compare(
                            inkwell::IntPredicate::EQ,
                            tag_val,
                            expected_tag_val,
                            "tag_match",
                        )
                        .map_err(|e| format!("Failed to compare tags: {:?}", e))
                } else {
                    // Not a struct - pattern doesn't match
                    Ok(self.context.bool_type().const_zero())
                }
            }
        }
    }

    fn bind_pattern(
        &mut self,
        pattern: &Pattern,
        value: BasicValueEnum<'ctx>,
    ) -> Result<(), String> {
        match pattern {
            Pattern::Ident { name, .. } => {
                // Bind the value to the identifier
                let alloca = self.create_entry_block_alloca(name, value.get_type())?;
                self.builder
                    .build_store(alloca, value)
                    .map_err(|e| format!("Failed to store pattern binding: {:?}", e))?;
                self.variables
                    .insert(name.clone(), (alloca, value.get_type()));
                Ok(())
            }

            Pattern::Constructor { name: _, args, .. } => {
                // For constructors with arguments, extract the payload
                // Result is { i8 tag, f64 payload }
                if let Some(first_arg) = args.first()
                    && let Pattern::Ident { name: arg_name, .. } = first_arg
                {
                    // Extract payload (field 1) from the struct
                    if let BasicValueEnum::StructValue(struct_val) = value {
                        let payload = self
                            .builder
                            .build_extract_value(struct_val, 1, "payload")
                            .map_err(|e| format!("Failed to extract payload: {:?}", e))?;

                        // Bind the payload value
                        let alloca =
                            self.create_entry_block_alloca(arg_name, payload.get_type())?;
                        self.builder
                            .build_store(alloca, payload)
                            .map_err(|e| format!("Failed to store constructor arg: {:?}", e))?;
                        self.variables
                            .insert(arg_name.clone(), (alloca, payload.get_type()));
                    }
                }
                Ok(())
            }

            _ => Ok(()), // Other patterns don't bind variables
        }
    }

    fn generate_for_loop(
        &mut self,
        collection: &Expr,
        pattern: &crate::ast::ForPattern,
        body: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        use crate::ast::ForPattern;

        // Generate the collection (should be an array struct: {ptr, size})
        let array_val = self.generate_expr(collection)?;

        // Get current function
        let function = self
            .current_function
            .ok_or_else(|| "For loop must be in a function".to_string())?;

        // Allocate the array struct to memory so we can access its fields
        let array_struct_type = self.context.struct_type(
            &[
                self.context.ptr_type(AddressSpace::default()).into(),
                self.context.i64_type().into(),
            ],
            false,
        );

        let array_alloca =
            self.create_entry_block_alloca("array_temp", array_struct_type.into())?;
        self.builder
            .build_store(array_alloca, array_val)
            .map_err(|e| format!("Failed to store array: {:?}", e))?;

        // Extract size field (field 1)
        let size_field_ptr = self
            .builder
            .build_struct_gep(array_struct_type, array_alloca, 1, "size_field_ptr")
            .map_err(|e| format!("Failed to get size field: {:?}", e))?;

        let size = self
            .builder
            .build_load(self.context.i64_type(), size_field_ptr, "size")
            .map_err(|e| format!("Failed to load size: {:?}", e))?
            .into_int_value();

        // Extract data pointer (field 0)
        let data_field_ptr = self
            .builder
            .build_struct_gep(array_struct_type, array_alloca, 0, "data_field_ptr")
            .map_err(|e| format!("Failed to get data field: {:?}", e))?;

        let data_ptr = self
            .builder
            .build_load(
                self.context.ptr_type(AddressSpace::default()),
                data_field_ptr,
                "data_ptr",
            )
            .map_err(|e| format!("Failed to load data ptr: {:?}", e))?
            .into_pointer_value();

        // Create basic blocks
        let loop_header = self.context.append_basic_block(function, "loop_header");
        let loop_body_block = self.context.append_basic_block(function, "loop_body");
        let loop_exit = self.context.append_basic_block(function, "loop_exit");

        // Create counter variable (i = 0)
        let counter_alloca =
            self.create_entry_block_alloca("loop_counter", self.context.i64_type().into())?;
        self.builder
            .build_store(counter_alloca, self.context.i64_type().const_int(0, false))
            .map_err(|e| format!("Failed to store counter: {:?}", e))?;

        // Jump to loop header
        self.builder
            .build_unconditional_branch(loop_header)
            .map_err(|e| format!("Failed to build branch: {:?}", e))?;

        // Loop header: check condition (i < size)
        self.builder.position_at_end(loop_header);
        let counter_val = self
            .builder
            .build_load(self.context.i64_type(), counter_alloca, "i")
            .map_err(|e| format!("Failed to load counter: {:?}", e))?
            .into_int_value();

        let cond = self
            .builder
            .build_int_compare(inkwell::IntPredicate::SLT, counter_val, size, "loop_cond")
            .map_err(|e| format!("Failed to build compare: {:?}", e))?;

        self.builder
            .build_conditional_branch(cond, loop_body_block, loop_exit)
            .map_err(|e| format!("Failed to build conditional branch: {:?}", e))?;

        // Loop body
        self.builder.position_at_end(loop_body_block);

        // Get current element: data_ptr[i]
        let elem_ptr = unsafe {
            self.builder
                .build_gep(
                    self.context.f64_type(), // TODO: support other element types
                    data_ptr,
                    &[counter_val],
                    "elem_ptr",
                )
                .map_err(|e| format!("Failed to build GEP: {:?}", e))?
        };

        let elem_val = self
            .builder
            .build_load(self.context.f64_type(), elem_ptr, "elem")
            .map_err(|e| format!("Failed to load element: {:?}", e))?;

        // Bind pattern variables
        match pattern {
            ForPattern::Item { name, .. } => {
                // Bind item
                let item_alloca = self.create_entry_block_alloca(name, elem_val.get_type())?;
                self.builder
                    .build_store(item_alloca, elem_val)
                    .map_err(|e| format!("Failed to store item: {:?}", e))?;
                self.variables
                    .insert(name.clone(), (item_alloca, elem_val.get_type()));
            }
            ForPattern::ItemIndex { item, index, .. } => {
                // Bind item
                let item_alloca = self.create_entry_block_alloca(item, elem_val.get_type())?;
                self.builder
                    .build_store(item_alloca, elem_val)
                    .map_err(|e| format!("Failed to store item: {:?}", e))?;
                self.variables
                    .insert(item.clone(), (item_alloca, elem_val.get_type()));

                // Bind index (convert i64 to f64 for Num type)
                let index_f64 = self
                    .builder
                    .build_signed_int_to_float(counter_val, self.context.f64_type(), "index_f64")
                    .map_err(|e| format!("Failed to convert index: {:?}", e))?;

                let index_alloca =
                    self.create_entry_block_alloca(index, index_f64.get_type().into())?;
                self.builder
                    .build_store(index_alloca, index_f64)
                    .map_err(|e| format!("Failed to store index: {:?}", e))?;
                self.variables
                    .insert(index.clone(), (index_alloca, index_f64.get_type().into()));
            }
        }

        // Generate loop body code
        let _ = self.generate_expr(body)?;

        // Increment counter: i = i + 1
        let next_counter = self
            .builder
            .build_int_add(
                counter_val,
                self.context.i64_type().const_int(1, false),
                "next_i",
            )
            .map_err(|e| format!("Failed to build add: {:?}", e))?;

        self.builder
            .build_store(counter_alloca, next_counter)
            .map_err(|e| format!("Failed to store next counter: {:?}", e))?;

        // Jump back to loop header
        self.builder
            .build_unconditional_branch(loop_header)
            .map_err(|e| format!("Failed to build branch: {:?}", e))?;

        // Loop exit: position builder and return 0 (unit/void equivalent)
        self.builder.position_at_end(loop_exit);
        Ok(self.context.f64_type().const_float(0.0).into())
    }

    /// The `{ ptr data, i64 len }` struct shared by arrays and `Text`. For `Text`,
    /// `data` is a NUL-terminated UTF-8 buffer and `len` is its byte length.
    fn ptr_len_struct_type(&self) -> inkwell::types::StructType<'ctx> {
        self.context.struct_type(
            &[
                self.context.ptr_type(AddressSpace::default()).into(),
                self.context.i64_type().into(),
            ],
            false,
        )
    }

    fn type_to_llvm(&self, ty: &Type) -> Result<BasicTypeEnum<'ctx>, String> {
        match ty {
            Type::Num => Ok(self.context.f64_type().into()),
            Type::Bool => Ok(self.context.bool_type().into()),
            // Text is { ptr data, i64 byte_len } (same shape as an array).
            Type::Text => Ok(self.ptr_len_struct_type().into()),
            Type::Array(elem_type) => {
                // Validate the element type, but LLVM uses opaque pointers so the
                // pointee type is not encoded in the pointer itself.
                let _elem = self.type_to_llvm(elem_type)?;
                Ok(self.context.ptr_type(AddressSpace::default()).into())
            }
            Type::Record(fields) => {
                let field_types: Vec<BasicTypeEnum> = fields
                    .iter()
                    .map(|(_name, ty)| self.type_to_llvm(ty))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(self.context.struct_type(&field_types, false).into())
            }
            Type::Sum { .. } => {
                // Canonical Result representation: { i8 tag, double payload }.
                // Matches the numeric payload built by `generate_sum_constructor`,
                // which lets functions declare `-> Result`. Non-numeric payloads
                // in return position are not yet unified (see the note there).
                Ok(self
                    .context
                    .struct_type(
                        &[
                            self.context.i8_type().into(),
                            self.context.f64_type().into(),
                        ],
                        false,
                    )
                    .into())
            }
            _ => Err(format!("Unsupported type: {:?}", ty)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::parse;

    #[test]
    fn test_simple_number() {
        let context = Context::create();
        let mut codegen = CodeGenerator::new(&context, "test");

        let tokens = Lexer::tokenize("x = 42").unwrap();
        let program = parse(&tokens).unwrap();

        let result = codegen.generate(&program);
        assert!(result.is_ok());

        let ir = result.unwrap();
        println!("Generated IR:\n{}", ir);
        // Global variable with float value
        assert!(ir.contains("4.2") || ir.contains("42"));
    }

    #[test]
    fn test_simple_function() {
        let context = Context::create();
        let mut codegen = CodeGenerator::new(&context, "test");

        let tokens = Lexer::tokenize("add = (a :: Num, b :: Num) -> Num => a + b").unwrap();
        let program = parse(&tokens).unwrap();

        let result = codegen.generate(&program);
        assert!(result.is_ok());

        let ir = result.unwrap();
        assert!(ir.contains("define"));
        assert!(ir.contains("add"));
    }

    #[test]
    fn test_local_variable() {
        let context = Context::create();
        let mut codegen = CodeGenerator::new(&context, "test");

        let code = "double = x :: Num => < y = x + x y >";
        let tokens = Lexer::tokenize(code).unwrap();
        let program = parse(&tokens).unwrap();

        let result = codegen.generate(&program);
        assert!(result.is_ok());

        let ir = result.unwrap();
        println!("Generated IR:\n{}", ir);
        assert!(ir.contains("alloca")); // Local variable
        assert!(ir.contains("load")); // Variable load
        assert!(ir.contains("store")); // Variable store
        assert!(ir.contains("fadd")); // Addition
    }

    #[test]
    fn test_array() {
        let context = Context::create();
        let mut codegen = CodeGenerator::new(&context, "test");

        // Test array in a function body - return the first element as a number
        let code = "sum = x :: Num => < arr = [x, x, x] x >";
        let tokens = Lexer::tokenize(code).unwrap();
        let program = parse(&tokens).unwrap();

        let result = codegen.generate(&program);
        if let Err(e) = &result {
            println!("Error: {}", e);
        }
        assert!(result.is_ok());

        let ir = result.unwrap();
        println!("Generated IR:\n{}", ir);
        assert!(ir.contains("alloca")); // Array allocation
        assert!(ir.contains("getelementptr")); // Array element access
    }

    #[test]
    fn test_function_call() {
        let context = Context::create();
        let mut codegen = CodeGenerator::new(&context, "test");

        // Test calling a function
        let code = "
            add = (a :: Num, b :: Num) => a + b
            main = => add(3, 4)
        ";
        let tokens = Lexer::tokenize(code).unwrap();
        let program = parse(&tokens).unwrap();

        let result = codegen.generate(&program);
        if let Err(e) = &result {
            println!("Error: {}", e);
        }
        assert!(result.is_ok());

        let ir = result.unwrap();
        println!("Generated IR:\n{}", ir);
        assert!(ir.contains("call")); // Function call
        assert!(ir.contains("@add")); // Call to add function
        assert!(ir.contains("fadd")); // Addition in add function
    }

    #[test]
    fn test_record() {
        let context = Context::create();
        let mut codegen = CodeGenerator::new(&context, "test");

        // Test record creation
        let code = "make_point = (x :: Num, y :: Num) => < p = {x = x, y = y} x >";
        let tokens = Lexer::tokenize(code).unwrap();
        let program = parse(&tokens).unwrap();

        let result = codegen.generate(&program);
        if let Err(e) = &result {
            println!("Error: {}", e);
        }
        assert!(result.is_ok());

        let ir = result.unwrap();
        println!("Generated IR:\n{}", ir);
        assert!(ir.contains("alloca")); // Struct allocation
        assert!(ir.contains("getelementptr")); // Field access
    }

    #[test]
    fn test_field_access() {
        let context = Context::create();
        let mut codegen = CodeGenerator::new(&context, "test");

        // Test field access
        let code = "get_x = (a :: Num, b :: Num) => < p = {x = a, y = b} p.x >";
        let tokens = Lexer::tokenize(code).unwrap();
        let program = parse(&tokens).unwrap();

        let result = codegen.generate(&program);
        if let Err(e) = &result {
            println!("Error: {}", e);
        }
        assert!(result.is_ok());

        let ir = result.unwrap();
        println!("Generated IR:\n{}", ir);
        assert!(ir.contains("getelementptr")); // Field GEP
        assert!(ir.contains("load")); // Field load
    }

    #[test]
    fn test_method_codegen_and_dispatch() {
        let context = Context::create();
        let mut codegen = CodeGenerator::new(&context, "test");

        // A named record with a method; the entry point constructs an instance and calls it.
        // All fields are Num so the field layout/access is exact.
        let code = "Point = {
  x :: Num,
  y :: Num,
  sum = => it.x + it.y
}

^ = () -> Num => <
  p = Point { x = 3, y = 4 }
  p.sum()
>";
        let tokens = Lexer::tokenize(code).unwrap();
        let program = parse(&tokens).unwrap();

        let result = codegen.generate(&program);
        if let Err(e) = &result {
            println!("Error: {}", e);
        }
        assert!(result.is_ok());

        let ir = result.unwrap();
        println!("Generated IR:\n{}", ir);
        // The method is emitted as a mangled top-level function taking the receiver pointer.
        assert!(ir.contains("@Point_sum"));
        // And the call site dispatches to it.
        assert!(ir.contains("call") && ir.contains("Point_sum"));
    }

    #[test]
    fn test_method_calls_sibling_method() {
        let context = Context::create();
        let mut codegen = CodeGenerator::new(&context, "test");

        // `doubled` calls the sibling method `sum` via `it.sum()` — exercises the signature
        // pre-pass (forward reference) and `it`-based dispatch.
        let code = "Point = {
  x :: Num,
  y :: Num,
  sum = => it.x + it.y,
  doubled = => it.sum() + it.sum()
}

^ = () -> Num => <
  p = Point { x = 10, y = 5 }
  p.doubled()
>";
        let tokens = Lexer::tokenize(code).unwrap();
        let program = parse(&tokens).unwrap();

        let result = codegen.generate(&program);
        if let Err(e) = &result {
            println!("Error: {}", e);
        }
        assert!(result.is_ok());

        let ir = result.unwrap();
        println!("Generated IR:\n{}", ir);
        assert!(ir.contains("@Point_sum"));
        assert!(ir.contains("@Point_doubled"));
    }
}
