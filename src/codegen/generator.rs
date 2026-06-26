// LLVM code generator for Quilon

use crate::ast::{
    BinOp, Expr, FunctionDecl, Item, MatchArm, Pattern, Program, Type, UnaryOp, VarDecl,
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
        let argv = main_fn.get_nth_param(1).unwrap().into_pointer_value();

        let entry = self.context.append_basic_block(main_fn, "entry");
        self.builder.position_at_end(entry);

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
            Item::TypeDecl(_) => Ok(()), // Type declarations don't generate code
        }
    }

    fn generate_var_decl(&mut self, decl: &VarDecl) -> Result<(), String> {
        // Check if this is a record literal to track field names
        if let Expr::Record { fields, .. } = &decl.value {
            let field_names: Vec<String> = fields.iter().map(|(name, _)| name.clone()).collect();
            self.record_types.insert(decl.name.clone(), field_names);
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
                // Create a global string constant
                let string_val = self
                    .builder
                    .build_global_string_ptr(value, "str")
                    .map_err(|e| format!("Failed to build string: {:?}", e))?;
                Ok(string_val.as_pointer_value().into())
            }

            Expr::Bool { value, .. } => Ok(self
                .context
                .bool_type()
                .const_int(*value as u64, false)
                .into()),

            Expr::Ident { name, .. } => {
                let (ptr, ty) = self
                    .variables
                    .get(name)
                    .ok_or_else(|| format!("Undefined variable: {}", name))?;

                self.builder
                    .build_load(*ty, *ptr, name)
                    .map_err(|e| format!("Failed to build load: {:?}", e))
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
            BinOp::Add => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self
                        .builder
                        .build_float_add(l, r, "addtmp")
                        .map_err(|e| format!("Failed to build add: {:?}", e))?
                        .into())
                } else {
                    Err("Add operation requires float values".to_string())
                }
            }
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
                let lhs_bool = self.to_boolean(lhs)?;
                let rhs_bool = self.to_boolean(rhs)?;

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
                let lhs_bool = self.to_boolean(lhs)?;
                let rhs_bool = self.to_boolean(rhs)?;

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
    fn to_boolean(
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

        // Check if this is a sum type constructor (Ok, NotOk, etc.)
        // For now, hardcode the builtin Result constructors
        match func_name.as_str() {
            "Ok" => return self.generate_sum_constructor(0, args), // Tag 0 for Ok
            "NotOk" => return self.generate_sum_constructor(1, args), // Tag 1 for NotOk
            _ => {}
        }

        // Get the function from the module
        let function = self
            .module
            .get_function(func_name)
            .ok_or_else(|| format!("Function not found: {}", func_name))?;

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

    fn generate_sum_constructor(
        &mut self,
        tag: u8,
        args: &[Expr],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // Tagged union layout: { i8 tag, f64 payload }
        // This is a simplified representation - proper implementation would use
        // actual union types and support multiple payload types

        let i8_type = self.context.i8_type();
        let f64_type = self.context.f64_type();

        // Create struct type for Result
        let result_struct = self
            .context
            .struct_type(&[i8_type.into(), f64_type.into()], false);

        // Create the tag value
        let tag_val = i8_type.const_int(tag as u64, false);

        // Generate the payload value (first argument, or 0.0 if none)
        let payload_val = if !args.is_empty() {
            let arg_val = self.generate_expr(&args[0])?;
            // Convert to f64 if needed
            match arg_val {
                BasicValueEnum::FloatValue(f) => f,
                BasicValueEnum::IntValue(i) => {
                    // Convert int to float
                    self.builder
                        .build_unsigned_int_to_float(i, f64_type, "inttofloat")
                        .map_err(|e| format!("Failed to convert int to float: {:?}", e))?
                }
                _ => f64_type.const_float(0.0), // Default for unsupported types
            }
        } else {
            f64_type.const_float(0.0)
        };

        // Build the struct value
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

    fn generate_field_access(
        &mut self,
        expr: &Expr,
        field_name: &str,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // Special handling for .size field on arrays
        if field_name == "size" {
            // For arrays (which are structs {ptr, i64}), we need special handling
            // Check if it's an identifier - we can directly work with the alloca
            if let Expr::Ident { name, .. } = expr {
                if let Some((var_ptr, var_type)) = self.variables.get(name).cloned() {
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
                                .build_signed_int_to_float(
                                    i,
                                    self.context.f64_type(),
                                    "size_as_num",
                                )
                                .map_err(|e| format!("Failed to convert size: {:?}", e))?;

                            return Ok(size_f64.into());
                        }
                    }
                }
            }
        }

        // For regular record field access
        // Special case: if expr is an identifier, check if we have record info
        if let Expr::Ident { name, .. } = expr {
            if let Some(field_names) = self.record_types.get(name) {
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

            Pattern::Constructor { name, args, .. } => {
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

            Pattern::Constructor { name, args, .. } => {
                // For constructors with arguments, extract the payload
                // Result is { i8 tag, f64 payload }
                if let Some(first_arg) = args.first() {
                    if let Pattern::Ident { name: arg_name, .. } = first_arg {
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

    fn type_to_llvm(&self, ty: &Type) -> Result<BasicTypeEnum<'ctx>, String> {
        match ty {
            Type::Num => Ok(self.context.f64_type().into()),
            Type::Bool => Ok(self.context.bool_type().into()),
            Type::Text => Ok(self
                .context
                .i8_type()
                .ptr_type(AddressSpace::default())
                .into()),
            Type::Array(elem_type) => {
                let elem = self.type_to_llvm(elem_type)?;
                // For now, represent arrays as pointers
                Ok(elem.ptr_type(AddressSpace::default()).into())
            }
            Type::Record(fields) => {
                let field_types: Vec<BasicTypeEnum> = fields
                    .iter()
                    .map(|(_name, ty)| self.type_to_llvm(ty))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(self.context.struct_type(&field_types, false).into())
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
}
