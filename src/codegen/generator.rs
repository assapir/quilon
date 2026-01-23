// LLVM code generator for Quilon

use crate::ast::{BinOp, Expr, FunctionDecl, Item, Program, Type, UnaryOp, VarDecl};
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::builder::Builder;
use inkwell::values::{BasicValue, BasicValueEnum, FunctionValue, PointerValue};
use inkwell::types::{BasicTypeEnum, BasicType};
use inkwell::AddressSpace;
use std::collections::HashMap;

pub struct CodeGenerator<'ctx> {
    context: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    variables: HashMap<String, (PointerValue<'ctx>, BasicTypeEnum<'ctx>)>,
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
            current_function: None,
        }
    }

    pub fn generate(&mut self, program: &Program) -> Result<String, String> {
        // Generate code for all top-level items
        for item in &program.items {
            self.generate_item(item)?;
        }

        // Verify the module
        if let Err(e) = self.module.verify() {
            return Err(format!("Module verification failed: {}", e));
        }

        // Return the LLVM IR as a string
        Ok(self.module.print_to_string().to_string())
    }

    fn generate_item(&mut self, item: &Item) -> Result<(), String> {
        match item {
            Item::VarDecl(decl) => self.generate_var_decl(decl),
            Item::FunctionDecl(decl) => self.generate_function_decl(decl),
            Item::TypeDecl(_) => Ok(()), // Type declarations don't generate code
        }
    }

    fn generate_var_decl(&mut self, decl: &VarDecl) -> Result<(), String> {
        let value = self.generate_expr(&decl.value)?;
        
        if self.current_function.is_some() {
            // Local variable - use alloca
            let var_type = value.get_type();
            let alloca = self.create_entry_block_alloca(&decl.name, var_type)?;
            self.builder.build_store(alloca, value)
                .map_err(|e| format!("Failed to build store: {:?}", e))?;
            self.variables.insert(decl.name.clone(), (alloca, var_type));
        } else {
            // Global variable
            let global = self.module.add_global(
                value.get_type(),
                Some(AddressSpace::default()),
                &decl.name,
            );
            global.set_initializer(&value);
        }
        
        Ok(())
    }

    fn generate_function_decl(&mut self, decl: &FunctionDecl) -> Result<(), String> {
        // Convert parameter types to LLVM types
        let param_types: Vec<BasicTypeEnum> = decl.params.iter()
            .map(|p| self.type_to_llvm(&p.type_annotation.clone().unwrap_or(Type::Num)))
            .collect::<Result<Vec<_>, _>>()?;

        // Convert return type
        let return_type = self.type_to_llvm(
            &decl.return_type.clone().unwrap_or(Type::Num)
        )?;

        // Create function type - use a helper to convert BasicTypeEnum to BasicMetadataTypeEnum
        let fn_type = return_type.fn_type(
            &param_types.iter()
                .map(|t| (*t).into())
                .collect::<Vec<inkwell::types::BasicMetadataTypeEnum>>(),
            false
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
            self.builder.build_store(alloca, llvm_param)
                .map_err(|e| format!("Failed to build store: {:?}", e))?;
            
            self.variables.insert(param.name.clone(), (alloca, param_type));
        }

        // Generate function body
        let body_value = self.generate_expr(&decl.body)?;
        self.builder.build_return(Some(&body_value))
            .map_err(|e| format!("Failed to build return: {:?}", e))?;

        Ok(())
    }

    fn create_entry_block_alloca(
        &self,
        name: &str,
        ty: BasicTypeEnum<'ctx>,
    ) -> Result<PointerValue<'ctx>, String> {
        let builder = self.context.create_builder();
        
        let entry = self.current_function.unwrap().get_first_basic_block().unwrap();
        match entry.get_first_instruction() {
            Some(first_instr) => builder.position_before(&first_instr),
            None => builder.position_at_end(entry),
        }

        builder.build_alloca(ty, name)
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
                let string_val = self.builder.build_global_string_ptr(value, "str")
                    .map_err(|e| format!("Failed to build string: {:?}", e))?;
                Ok(string_val.as_pointer_value().into())
            }
            
            Expr::Bool { value, .. } => {
                Ok(self.context.bool_type().const_int(*value as u64, false).into())
            }
            
            Expr::Ident { name, .. } => {
                let (ptr, ty) = self.variables.get(name)
                    .ok_or_else(|| format!("Undefined variable: {}", name))?;
                
                self.builder.build_load(*ty, *ptr, name)
                    .map_err(|e| format!("Failed to build load: {:?}", e))
            }
            
            Expr::BinOp { left, op, right, .. } => {
                self.generate_binop(left, *op, right)
            }
            
            Expr::UnaryOp { op, expr, .. } => {
                self.generate_unary_op(*op, expr)
            }
            
            Expr::Call { func, args, .. } => {
                self.generate_call(func, args)
            }
            
            Expr::If { cond, then, else_, .. } => {
                self.generate_if(cond, then, else_)
            }
            
            Expr::Block { stmts, .. } => {
                self.generate_block(stmts)
            }
            
            Expr::Array { elements, .. } => {
                self.generate_array(elements)
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
            BinOp::Add => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self.builder.build_float_add(l, r, "addtmp")
                        .map_err(|e| format!("Failed to build add: {:?}", e))?.into())
                } else {
                    Err("Add operation requires float values".to_string())
                }
            }
            BinOp::Sub => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self.builder.build_float_sub(l, r, "subtmp")
                        .map_err(|e| format!("Failed to build sub: {:?}", e))?.into())
                } else {
                    Err("Sub operation requires float values".to_string())
                }
            }
            BinOp::Mul => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self.builder.build_float_mul(l, r, "multmp")
                        .map_err(|e| format!("Failed to build mul: {:?}", e))?.into())
                } else {
                    Err("Mul operation requires float values".to_string())
                }
            }
            BinOp::Div => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self.builder.build_float_div(l, r, "divtmp")
                        .map_err(|e| format!("Failed to build div: {:?}", e))?.into())
                } else {
                    Err("Div operation requires float values".to_string())
                }
            }
            BinOp::Eq => {
                if let (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) = (lhs, rhs) {
                    Ok(self.builder.build_float_compare(
                        inkwell::FloatPredicate::OEQ, l, r, "eqtmp"
                    ).map_err(|e| format!("Failed to build compare: {:?}", e))?.into())
                } else {
                    Err("Eq operation requires float values".to_string())
                }
            }
            _ => Err(format!("Unsupported binary operation: {:?}", op)),
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
                    Ok(self.builder.build_float_neg(f, "negtmp")
                        .map_err(|e| format!("Failed to build neg: {:?}", e))?.into())
                } else {
                    Err("Neg operation requires float value".to_string())
                }
            }
            UnaryOp::Not => {
                if let BasicValueEnum::IntValue(i) = val {
                    Ok(self.builder.build_not(i, "nottmp")
                        .map_err(|e| format!("Failed to build not: {:?}", e))?.into())
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

        // Get the function from the module
        let function = self.module.get_function(func_name)
            .ok_or_else(|| format!("Function not found: {}", func_name))?;

        // Generate argument values
        let arg_values: Vec<BasicValueEnum> = args.iter()
            .map(|arg| self.generate_expr(arg))
            .collect::<Result<Vec<_>, _>>()?;

        // Convert to BasicMetadataValueEnum for the call
        let arg_metadata: Vec<inkwell::values::BasicMetadataValueEnum> = arg_values.iter()
            .map(|v| (*v).into())
            .collect();

        // Build the call
        let call_site = self.builder.build_call(function, &arg_metadata, "calltmp")
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

        let function = self.current_function
            .ok_or_else(|| "If expression outside of function".to_string())?;

        // Create blocks
        let then_bb = self.context.append_basic_block(function, "then");
        let else_bb = self.context.append_basic_block(function, "else");
        let merge_bb = self.context.append_basic_block(function, "ifcont");

        // Build conditional branch
        self.builder.build_conditional_branch(cond_bool, then_bb, else_bb)
            .map_err(|e| format!("Failed to build conditional branch: {:?}", e))?;

        // Generate then block
        self.builder.position_at_end(then_bb);
        let then_val = self.generate_expr(then_expr)?;
        self.builder.build_unconditional_branch(merge_bb)
            .map_err(|e| format!("Failed to build branch: {:?}", e))?;
        let then_bb = self.builder.get_insert_block().unwrap();

        // Generate else block
        self.builder.position_at_end(else_bb);
        let else_val = self.generate_expr(else_expr)?;
        self.builder.build_unconditional_branch(merge_bb)
            .map_err(|e| format!("Failed to build branch: {:?}", e))?;
        let else_bb = self.builder.get_insert_block().unwrap();

        // Generate merge block
        self.builder.position_at_end(merge_bb);
        let phi = self.builder.build_phi(then_val.get_type(), "iftmp")
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

    fn generate_array(
        &mut self,
        elements: &[Expr],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if elements.is_empty() {
            // Empty array - create array of f64 for now
            let array_type = self.context.f64_type().array_type(0);
            return Ok(array_type.const_zero().into());
        }
        
        // Generate all element values
        let values: Vec<BasicValueEnum> = elements.iter()
            .map(|e| self.generate_expr(e))
            .collect::<Result<Vec<_>, _>>()?;
        
        // Get element type from first element
        let elem_type = values[0].get_type();
        
        // Create array type
        let array_type = elem_type.array_type(values.len() as u32);
        
        // Build constant array if all values are constants
        // For now, we'll allocate an array and store values
        if self.current_function.is_some() {
            let alloca = self.builder.build_alloca(array_type, "array")
                .map_err(|e| format!("Failed to build alloca: {:?}", e))?;
            
            // Store each element
            for (i, value) in values.iter().enumerate() {
                let index = self.context.i32_type().const_int(i as u64, false);
                let gep = unsafe {
                    self.builder.build_gep(
                        array_type,
                        alloca,
                        &[self.context.i32_type().const_zero(), index],
                        &format!("elem_{}", i)
                    ).map_err(|e| format!("Failed to build GEP: {:?}", e))?
                };
                self.builder.build_store(gep, *value)
                    .map_err(|e| format!("Failed to build store: {:?}", e))?;
            }
            
            Ok(alloca.into())
        } else {
            // For globals, we need constant values
            Err("Global arrays not yet implemented".to_string())
        }
    }


    fn type_to_llvm(&self, ty: &Type) -> Result<BasicTypeEnum<'ctx>, String> {
        match ty {
            Type::Num => Ok(self.context.f64_type().into()),
            Type::Bool => Ok(self.context.bool_type().into()),
            Type::String => Ok(self.context.i8_type().ptr_type(AddressSpace::default()).into()),
            Type::Array(elem_type) => {
                let elem = self.type_to_llvm(elem_type)?;
                // For now, represent arrays as pointers
                Ok(elem.ptr_type(AddressSpace::default()).into())
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
        let mut gen = CodeGenerator::new(&context, "test");
        
        let tokens = Lexer::tokenize("x = 42").unwrap();
        let program = parse(&tokens).unwrap();
        
        let result = gen.generate(&program);
        assert!(result.is_ok());
        
        let ir = result.unwrap();
        println!("Generated IR:\n{}", ir);
        // Global variable with float value
        assert!(ir.contains("4.2") || ir.contains("42"));
    }

    #[test]
    fn test_simple_function() {
        let context = Context::create();
        let mut gen = CodeGenerator::new(&context, "test");
        
        let tokens = Lexer::tokenize("add = (a :: Num, b :: Num) -> Num => a + b").unwrap();
        let program = parse(&tokens).unwrap();
        
        let result = gen.generate(&program);
        assert!(result.is_ok());
        
        let ir = result.unwrap();
        assert!(ir.contains("define"));
        assert!(ir.contains("add"));
    }

    #[test]
    fn test_local_variable() {
        let context = Context::create();
        let mut gen = CodeGenerator::new(&context, "test");
        
        let code = "double = x :: Num => < y = x + x y >";
        let tokens = Lexer::tokenize(code).unwrap();
        let program = parse(&tokens).unwrap();
        
        let result = gen.generate(&program);
        assert!(result.is_ok());
        
        let ir = result.unwrap();
        println!("Generated IR:\n{}", ir);
        assert!(ir.contains("alloca")); // Local variable
        assert!(ir.contains("load"));   // Variable load
        assert!(ir.contains("store"));  // Variable store
        assert!(ir.contains("fadd"));   // Addition
    }

    #[test]
    fn test_array() {
        let context = Context::create();
        let mut gen = CodeGenerator::new(&context, "test");
        
        // Test array in a function body - return the first element as a number
        let code = "sum = x :: Num => < arr = [x, x, x] x >";
        let tokens = Lexer::tokenize(code).unwrap();
        let program = parse(&tokens).unwrap();
        
        let result = gen.generate(&program);
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
        let mut gen = CodeGenerator::new(&context, "test");
        
        // Test calling a function
        let code = "
            add = (a :: Num, b :: Num) => a + b
            main = => add(3, 4)
        ";
        let tokens = Lexer::tokenize(code).unwrap();
        let program = parse(&tokens).unwrap();
        
        let result = gen.generate(&program);
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
}
