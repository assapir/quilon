// LLVM code generator for Quilon

use crate::ast::{
    BinOp, Expr, FunctionDecl, Item, MatchArm, MethodDecl, Pattern, Program, Type, TypeDecl,
    TypeDef, UnaryOp, VarDecl, is_operator_symbol,
};
use inkwell::AddressSpace;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::types::{BasicType, BasicTypeEnum};
use inkwell::values::{BasicValue, BasicValueEnum, FunctionValue, PointerValue};
use std::collections::HashMap;

/// Names that the compiler provides built-in overloads for (`print`/`eprint`, lowered
/// to runtime intrinsics). A user definition of one ADDS an overload member (and is
/// mangled), rather than shadowing the built-in single-arg Num/Text/Bool forms.
fn is_builtin_overload_name(name: &str) -> bool {
    matches!(name, "print" | "eprint")
}

/// A short, mangling-safe tag for a Quilon type used in overload name mangling. Must be
/// deterministic and identical at definition and call sites (built from the declared
/// parameter type and from the inferred argument type respectively).
fn type_mangle(ty: &Type) -> String {
    match ty {
        Type::Num => "N".to_string(),
        Type::Text => "T".to_string(),
        Type::Bool => "B".to_string(),
        Type::Unit => "U".to_string(),
        Type::Array(elem) => format!("A{}", type_mangle(elem)),
        Type::Named { name, .. } | Type::Sum { name, .. } => format!("named${}", name),
        // A not-yet-concrete sum payload (`Generic`) resolves as `Num` for overload
        // dispatch (see the type checker's `types_match`), so it mangles to the Num tag
        // — keeping codegen's chosen symbol in agreement with the checker.
        Type::Generic { .. } => "N".to_string(),
        // Any other shape (e.g. a function type) — a stable, mangling-safe fallback.
        other => format!("X{:?}", other)
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '$')
            .collect(),
    }
}

/// The distinct LLVM symbol for one overload member: its name plus a per-parameter
/// type tag. Operator symbols (which aren't valid LLVM identifiers) are spelled out so
/// e.g. `+` on `(Point, Point)` becomes `op.add$named$Point$named$Point`.
fn mangle_overload(name: &str, params: &[Type]) -> String {
    let base = operator_word(name)
        .map(|w| format!("op.{}", w))
        .unwrap_or_else(|| name.to_string());
    let mut s = base;
    for p in params {
        s.push('$');
        s.push_str(&type_mangle(p));
    }
    s
}

/// A pronounceable word for an operator symbol, for use in a mangled LLVM name (which
/// can't contain the raw symbol). Returns `None` for non-operator (ordinary) names.
fn operator_word(name: &str) -> Option<&'static str> {
    Some(match name {
        "+" => "add",
        "-" => "sub",
        "*" => "mul",
        "/" => "div",
        "%" => "mod",
        "==" => "eq",
        "!=" => "ne",
        "<" => "lt",
        "<=" => "le",
        ">" => "gt",
        ">=" => "ge",
        "&&" => "and",
        "||" => "or",
        _ => return None,
    })
}

/// A zero/`undef`-free constant of any basic LLVM type, used to fill a payload slot that
/// carries no information (a `$` Unit payload stored into a sized slot).
fn zeroed(ty: BasicTypeEnum<'_>) -> BasicValueEnum<'_> {
    match ty {
        BasicTypeEnum::IntType(t) => t.const_zero().into(),
        BasicTypeEnum::FloatType(t) => t.const_zero().into(),
        BasicTypeEnum::PointerType(t) => t.const_zero().into(),
        BasicTypeEnum::StructType(t) => t.const_zero().into(),
        BasicTypeEnum::ArrayType(t) => t.const_zero().into(),
        BasicTypeEnum::VectorType(t) => t.const_zero().into(),
        BasicTypeEnum::ScalableVectorType(t) => t.const_zero().into(),
    }
}

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
    // Sum-type variant registry: variant (constructor) name -> (tag, owning type name).
    // Built from user `TypeDef::Sum` declarations plus the built-in Result. The tag is
    // the variant's declaration index. Drives constructor codegen and tag-based pattern
    // dispatch (generalizing the old hardcoded Ok=0/NotOk=1).
    sum_variants: HashMap<String, (u8, String)>,
    // Declared payload Quilon types per variant (constructor name -> field types).
    // Lets `bind_pattern` record a matched payload binding's type in `var_types`, so an
    // overloaded call on that binding (e.g. `Circle(n) => area(n)`) mangles by the
    // payload's concrete type. (Result's `Ok`/`NotOk` carry `Generic`, which resolves
    // as Num for overloads — see the type checker's `types_match`.)
    variant_payloads: HashMap<String, Vec<Type>>,
    // Per-sum-type canonical payload layout (one LLVM type per payload slot), sized to
    // the widest variant so EVERY value of the type has the same struct shape
    // `{ i8 tag, slot0, slot1, ... }`. This lets a match arm extract any variant's
    // payload slots without going out of range, even when the runtime value was built
    // from a narrower variant. Keyed by sum-type name. Only USER sum types are entered
    // here; the predefined `Result` is intentionally absent (its generic, heterogeneous
    // payloads are sized per-value at construction — see `register_builtin_sum_types`).
    sum_layouts: HashMap<String, Vec<BasicTypeEnum<'ctx>>>,
    current_function: Option<FunctionValue<'ctx>>,
    // Overload sets, keyed by name (function names AND operator symbols like `"+"`).
    // Each entry is the list of that name's overload parameter-type signatures. A name
    // is present here iff it is an overload set (operator-named, or 2+ same-named
    // top-level defs); calls/operators to these names dispatch to a NAME-MANGLED
    // function (`mangle_overload`) chosen by exact argument types. Operator builtins
    // (Num/Text `+`, comparisons) are NOT entered here — they keep their inline
    // lowering; only USER operator overloads add an operator symbol to this map.
    // Each member is its `(parameter types, return type)`.
    overloads: HashMap<String, Vec<(Vec<Type>, Type)>>,
    // Quilon type of each in-scope local/param, for argument-type inference at
    // overloaded call sites (codegen lacks the type checker's full inference, so it
    // tracks just enough — locals, params, and constructor results — to mangle).
    var_types: HashMap<String, Type>,
    // Declared return type of each NON-overloaded top-level function, so `infer_type`
    // can give a call's result its real type (not a `Num` default) when that result is
    // an argument to an overloaded call/operator — keeping codegen dispatch in sync
    // with the type checker. (Overloaded callees' returns come from `overloads`.)
    fn_return_types: HashMap<String, Type>,
}

impl<'ctx> CodeGenerator<'ctx> {
    pub fn new(context: &'ctx Context, module_name: &str) -> Self {
        let module = context.create_module(module_name);
        let builder = context.create_builder();

        let mut codegen = CodeGenerator {
            context,
            module,
            builder,
            variables: HashMap::new(),
            record_types: HashMap::new(),
            named_type_fields: HashMap::new(),
            var_named_types: HashMap::new(),
            sum_variants: HashMap::new(),
            variant_payloads: HashMap::new(),
            sum_layouts: HashMap::new(),
            current_function: None,
            overloads: HashMap::new(),
            var_types: HashMap::new(),
            fn_return_types: HashMap::new(),
        };
        codegen.register_builtin_sum_types();
        codegen
    }

    /// Register the predefined `Result` variants: `Ok` is tag 0, `NotOk` is tag 1.
    /// Unlike user sum types, Result is NOT given a fixed payload layout: its variants
    /// have generic payloads (`Ok(T)` / `NotOk(E)`) whose concrete type is only known at
    /// each construction site, and the two variants routinely carry DIFFERENT payload
    /// types (e.g. `Ok(num)` vs `NotOk(text)`). So a Result value is sized to its
    /// actual payload at construction (`generate_sum_constructor`'s no-registered-layout
    /// path), preserving the historical per-value representation
    /// (`Ok(42) -> { i8, double }`, `NotOk("e") -> { i8, ptr }`).
    fn register_builtin_sum_types(&mut self) {
        self.sum_variants
            .insert("Ok".to_string(), (0u8, "Result".to_string()));
        self.sum_variants
            .insert("NotOk".to_string(), (1u8, "Result".to_string()));
        // Result's payloads are generic (`Ok(T)` / `NotOk(E)`); a `Generic` binding
        // resolves as Num for overload dispatch (see the type checker's `types_match`).
        let generic = |n: &str| Type::Generic {
            name: n.to_string(),
            args: vec![],
        };
        self.variant_payloads
            .insert("Ok".to_string(), vec![generic("T")]);
        self.variant_payloads
            .insert("NotOk".to_string(), vec![generic("E")]);
    }

    /// Access the underlying LLVM module after `generate` has populated it.
    /// Used by the JIT runner to create an execution engine in-process.
    pub fn module(&self) -> &Module<'ctx> {
        &self.module
    }

    pub fn generate(&mut self, program: &Program) -> Result<String, String> {
        // Pre-pass: register all user sum-type variants so constructors and pattern
        // dispatch resolve regardless of declaration order relative to their uses.
        for item in &program.items {
            if let Item::TypeDecl(TypeDecl {
                name,
                type_def: TypeDef::Sum(variants),
                ..
            }) = item
            {
                self.register_sum_variants(name, variants)?;
            }
        }

        // Pre-pass: discover overload sets (operator-named, or 2+ same-named defs),
        // mirroring the type checker. Their definitions are name-mangled by parameter
        // type and dispatched by exact argument type at each call/operator site.
        let mut fn_counts: HashMap<&str, usize> = HashMap::new();
        for item in &program.items {
            if let Item::FunctionDecl(decl) = item
                && !decl.is_inert_io_placeholder()
            {
                *fn_counts.entry(decl.name.as_str()).or_insert(0) += 1;
            }
        }
        for item in &program.items {
            if let Item::FunctionDecl(decl) = item
                && !decl.is_inert_io_placeholder()
                && (is_operator_symbol(&decl.name)
                    || fn_counts.get(decl.name.as_str()).copied().unwrap_or(0) > 1
                    || is_builtin_overload_name(&decl.name))
                && decl.name != "^"
            {
                let params: Vec<Type> = decl
                    .params
                    .iter()
                    .map(|p| p.type_annotation.clone().unwrap_or(Type::Num))
                    .collect();
                // The return type drives argument-type inference for a value bound to
                // an overloaded call/operator (e.g. a user `+` returning a record).
                let ret = decl.return_type.clone().unwrap_or(Type::Num);
                self.overloads
                    .entry(decl.name.clone())
                    .or_default()
                    .push((params, ret));
            }
        }

        // Pre-pass: record each NON-overloaded top-level function's declared return
        // type, so `infer_type` can give a call result its real type when it feeds an
        // overloaded call/operator (keeps codegen dispatch in sync with the checker).
        for item in &program.items {
            if let Item::FunctionDecl(decl) = item
                && !decl.is_inert_io_placeholder()
                && !self.overloads.contains_key(&decl.name)
                && let Some(ret) = &decl.return_type
            {
                self.fn_return_types.insert(decl.name.clone(), ret.clone());
            }
        }

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

    /// Register a sum type: map each variant to `(tag, type_name)` (tag = declaration
    /// index) and compute the type's canonical payload layout — one LLVM slot per
    /// payload position, sized to the widest variant. Per position, the slot type is the
    /// first NON-Unit field at that position (the type checker has validated that all
    /// concrete fields at a position agree). `$` (Unit) payload fields are zero-sized and
    /// contribute no slot, so a position that is `$` in every variant is dropped; a
    /// position mixing `$` with a concrete type uses the concrete type.
    fn register_sum_variants(
        &mut self,
        type_name: &str,
        variants: &[crate::ast::SumVariant],
    ) -> Result<(), String> {
        for (tag, variant) in variants.iter().enumerate() {
            self.sum_variants
                .insert(variant.name.clone(), (tag as u8, type_name.to_string()));
            // Record the variant's declared (concrete) payload types so a match arm's
            // payload binding gets its real type for overloaded-call mangling.
            self.variant_payloads
                .insert(variant.name.clone(), variant.fields.clone());
        }

        let max_arity = variants.iter().map(|v| v.fields.len()).max().unwrap_or(0);
        let mut layout: Vec<BasicTypeEnum<'ctx>> = Vec::with_capacity(max_arity);
        for pos in 0..max_arity {
            // Slot type = the first NON-Unit field at this position (all concrete fields
            // here agree, per the checker). If every variant has `$` (or nothing) here,
            // the slot is a zero `i8` Unit placeholder — keeps the struct shape uniform
            // (one field per position) while storing nothing meaningful.
            let concrete = variants
                .iter()
                .find_map(|v| v.fields.get(pos).filter(|f| **f != Type::Unit));
            let slot = match concrete {
                Some(ty) => self.type_to_llvm(ty)?,
                None => self.context.i8_type().into(),
            };
            layout.push(slot);
        }
        // A purely nullary enum still needs a payload slot so its `{ i8, .. }` value has
        // a uniform shape; use a single `double` placeholder (matches constructor codegen).
        if layout.is_empty() {
            layout.push(self.context.f64_type().into());
        }
        self.sum_layouts.insert(type_name.to_string(), layout);
        Ok(())
    }

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
                // Unannotated return type defaults to Num, except a setter body whose
                // tail is an in-place field write (`it.field := v`) yields `$` (i8).
                let inferred_ret = match &method.return_type {
                    Some(t) => t.clone(),
                    None if self.expr_is_unit(&method.body) => Type::Unit,
                    None => Type::Num,
                };
                let return_type = self.type_to_llvm(&inferred_ret)?;
                let fn_type = return_type.fn_type(&param_types, false);
                let method_fn = self.module.add_function(&mangled, fn_type, None);
                // Internal linkage: method symbols are module-private (see generate_function_decl).
                method_fn.set_linkage(inkwell::module::Linkage::Internal);
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

        // Remember the binding's Quilon type for overloaded-call argument mangling.
        let inferred_qty = self.infer_type(&decl.value);
        // If the value is a named record (e.g. bound to a user operator overload's
        // result), track its type/fields so later `name.field` / method calls resolve.
        if let Type::Named { name, .. } = &inferred_qty
            && let Some(fields) = self.named_type_fields.get(name).cloned()
        {
            self.record_types.insert(decl.name.clone(), fields);
            self.var_named_types.insert(decl.name.clone(), name.clone());
        }
        self.var_types.insert(decl.name.clone(), inferred_qty);

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
        // The inert core.io print/eprint placeholder is never emitted (the compiler
        // lowers print/eprint to runtime intrinsics).
        if decl.is_inert_io_placeholder() {
            return Ok(());
        }

        // Convert parameter types to LLVM types
        let param_types: Vec<BasicTypeEnum> = decl
            .params
            .iter()
            .map(|p| self.type_to_llvm(&p.type_annotation.clone().unwrap_or(Type::Num)))
            .collect::<Result<Vec<_>, _>>()?;

        // Convert return type. The entry point `^` always returns a Num exit code at
        // the LLVM level (the C `main` wrapper expects an f64), regardless of its body
        // type — so a side-effecting main can omit the trailing `0`.
        let return_type = if decl.name == "^" {
            self.context.f64_type().into()
        } else {
            // An unannotated body defaults to `Num`, except a Unit (`$`) tail — e.g.
            // `log = m => print(m)` — which must be `i8`, not f64, or `build_return`
            // would emit `ret i8` into an f64 function and fail module verification.
            let inferred = match &decl.return_type {
                Some(t) => t.clone(),
                None if self.expr_is_unit(&decl.body) => Type::Unit,
                None => Type::Num,
            };
            self.type_to_llvm(&inferred)?
        };

        // Create function type - use a helper to convert BasicTypeEnum to BasicMetadataTypeEnum
        let fn_type = return_type.fn_type(
            &param_types
                .iter()
                .map(|t| (*t).into())
                .collect::<Vec<inkwell::types::BasicMetadataTypeEnum>>(),
            false,
        );

        // Create the function. Use internal linkage so a Quilon function name never
        // collides with a C library / runtime symbol when the whole program is linked
        // into one native binary (AOT). For example core.io's `write` placeholder, or
        // a user function named `read`/`open`, would otherwise shadow libc and break
        // the runtime intrinsics. Only the generated `main` wrapper is exported.
        //
        // An overloaded member (operator-named, or one of several same-named defs) is
        // emitted under a per-signature MANGLED name so the members don't collide; each
        // call site dispatches to the matching mangled symbol by exact argument type.
        let symbol = if self.overloads.contains_key(&decl.name) {
            let params: Vec<Type> = decl
                .params
                .iter()
                .map(|p| p.type_annotation.clone().unwrap_or(Type::Num))
                .collect();
            mangle_overload(&decl.name, &params)
        } else {
            decl.name.clone()
        };
        let function = self.module.add_function(&symbol, fn_type, None);
        function.set_linkage(inkwell::module::Linkage::Internal);
        self.current_function = Some(function);

        // Create entry block
        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);

        // Store parameters in variables map
        self.variables.clear();
        self.var_types.clear();
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
            // Track the parameter's Quilon type for overloaded-call mangling, and so a
            // record/sum parameter's methods/fields resolve.
            let qty = param.type_annotation.clone().unwrap_or(Type::Num);
            if let Type::Named { name, .. } | Type::Sum { name, .. } = &qty {
                self.var_named_types
                    .insert(param.name.clone(), name.clone());
                if let Some(fields) = self.named_type_fields.get(name) {
                    self.record_types.insert(param.name.clone(), fields.clone());
                }
            }
            self.var_types.insert(param.name.clone(), qty);
        }

        // Generate function body
        let body_value = self.generate_expr(&decl.body)?;

        // Entry point `^`: if the body's value isn't a Num (f64) — e.g. a side-effecting
        // main ending in a Text/Bool/record expression — discard it and implicitly
        // return 0 (C `main`-style success). A Num body is used as the exit code as
        // usual. Scoped to `^`; ordinary functions return their body's actual type.
        let return_value: inkwell::values::BasicValueEnum =
            if decl.name == "^" && !body_value.is_float_value() {
                self.context.f64_type().const_float(0.0).into()
            } else {
                body_value
            };
        self.builder
            .build_return(Some(&return_value))
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

            // The unit value `$`: a zero `i8` placeholder. The value is never
            // inspected; it just needs a concrete, single-inhabitant representation.
            Expr::Unit { .. } => Ok(self.unit_value().into()),

            Expr::Ident { name, .. } => {
                // A bare nullary sum-type constructor (e.g. `Red`) builds its tagged
                // struct here. Payload-carrying constructors are calls, handled above.
                // (We only treat it as a constructor when it isn't a bound variable.)
                if !self.variables.contains_key(name)
                    && let Some((tag, type_name)) = self.sum_variants.get(name).cloned()
                {
                    return self.generate_sum_constructor(tag, &type_name, &[]);
                }
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

            Expr::FieldAssign { target, value, .. } => self.generate_field_assign(target, value),

            Expr::Index { expr, index, .. } => self.generate_index(expr, index),

            Expr::Match { expr, arms, .. } => self.generate_match(expr, arms),

            Expr::Range { start, end, .. } => self.generate_range(start, end),

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
        // A USER operator overload (e.g. `+`/`==` on a record type) lowers to a direct
        // call to its mangled function — the operator is just a named overload set.
        // Built-in operators (Num arithmetic/compare, Text `+`/comparison) keep their
        // inline lowering below; they are not entered in `self.overloads`.
        let sym = op.symbol();
        if self.overloads.contains_key(sym) {
            let arg_types = [self.infer_type(left), self.infer_type(right)];
            if let Some(symbol) = self.resolve_overload_symbol(sym, &arg_types) {
                let l = self.generate_expr(left)?;
                let r = self.generate_expr(right)?;
                return self.build_direct_call(&symbol, &[l, r]);
            }
        }

        let lhs = self.generate_expr(left)?;
        let rhs = self.generate_expr(right)?;

        // Text comparison: both operands are `Text` { ptr, i64 } structs. Lower
        // equality and lexicographic ordering via the `__text_cmp` runtime intrinsic
        // (returns -1/0/1), then compare its result against 0 with the matching
        // integer predicate. (Num operands fall through to the float paths below.)
        if matches!(
            op,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        ) && matches!(lhs, BasicValueEnum::StructValue(_))
            && matches!(rhs, BasicValueEnum::StructValue(_))
        {
            return self.generate_text_compare(op, lhs, rhs);
        }

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
            BinOp::Eq => match (lhs, rhs) {
                (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) => Ok(self
                    .builder
                    .build_float_compare(inkwell::FloatPredicate::OEQ, l, r, "eqtmp")
                    .map_err(|e| format!("Failed to build compare: {:?}", e))?
                    .into()),
                // Bool == Bool (both i1) compares the integer values.
                (BasicValueEnum::IntValue(l), BasicValueEnum::IntValue(r)) => Ok(self
                    .builder
                    .build_int_compare(inkwell::IntPredicate::EQ, l, r, "eqtmp")
                    .map_err(|e| format!("Failed to build compare: {:?}", e))?
                    .into()),
                _ => Err("Eq requires two Nums or two Bools".to_string()),
            },
            BinOp::Ne => match (lhs, rhs) {
                (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) => Ok(self
                    .builder
                    .build_float_compare(inkwell::FloatPredicate::ONE, l, r, "netmp")
                    .map_err(|e| format!("Failed to build compare: {:?}", e))?
                    .into()),
                (BasicValueEnum::IntValue(l), BasicValueEnum::IntValue(r)) => Ok(self
                    .builder
                    .build_int_compare(inkwell::IntPredicate::NE, l, r, "netmp")
                    .map_err(|e| format!("Failed to build compare: {:?}", e))?
                    .into()),
                _ => Err("Ne requires two Nums or two Bools".to_string()),
            },
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
            // i32 __text_cmp(i8* a, i64 alen, i8* b, i64 blen) — lexicographic byte
            // comparison, returning -1 / 0 / 1. Backs Text ==/!=/</<=/>/>=.
            "__text_cmp" => ctx
                .i32_type()
                .fn_type(&[ptr.into(), i64t.into(), ptr.into(), i64t.into()], false),
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
        // `print`/`eprint` yield Unit (`$`); their result is meaningless.
        Ok(self.unit_value().into())
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
        // `print`/`eprint` are the built-in single-arg Num/Text/Bool overloads; a
        // *user* overload of the same name (a different signature) is dispatched as a
        // mangled function below, so only use the intrinsic when no user overload
        // matches the argument types.
        match func_name.as_str() {
            "print" | "eprint" => {
                let arg_types: Vec<Type> = args.iter().map(|a| self.infer_type(a)).collect();
                let is_builtin_print = arg_types.len() == 1
                    && matches!(arg_types[0], Type::Num | Type::Text | Type::Bool);
                let has_user_match = self
                    .resolve_overload_symbol(func_name, &arg_types)
                    .is_some();
                if is_builtin_print && !has_user_match {
                    return self.generate_print(func_name, args);
                }
            }
            "write" => return self.generate_write(args),
            _ => {}
        }

        // Sum-type constructor with a payload (e.g. `Ok(x)`, `Circle(r)`, `Rect(w, h)`):
        // resolved from the variant registry built from the predefined Result and all
        // user `TypeDef::Sum` declarations.
        if let Some((tag, type_name)) = self.sum_variants.get(func_name.as_str()).cloned() {
            return self.generate_sum_constructor(tag, &type_name, args);
        }

        // Overloaded function call: dispatch to the per-signature mangled symbol chosen
        // by exact argument types (the type checker has already verified a unique match).
        let overload_symbol = if self.overloads.contains_key(func_name.as_str()) {
            let arg_types: Vec<Type> = args.iter().map(|a| self.infer_type(a)).collect();
            self.resolve_overload_symbol(func_name, &arg_types)
        } else {
            None
        };

        // Get the function from the module. If there is no plain top-level function with this
        // name, it may be a method call: the parser desugars `recv.method(a, b)` to
        // `method(recv, a, b)`, so resolve `recv`'s named type and dispatch to `Type_method`.
        let function = if let Some(sym) = &overload_symbol {
            self.module
                .get_function(sym)
                .ok_or_else(|| format!("Overload not found: {}", sym))?
        } else {
            match self.module.get_function(func_name) {
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
        type_name: &str,
        args: &[Expr],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        // Tagged-union value: { i8 tag, slot0, slot1, ... }.
        //
        // The slot types come from one of two sources:
        //  - USER sum types have a registered canonical layout (`sum_layouts`), sized to
        //    the widest variant, so EVERY value of the type shares one struct shape and a
        //    match arm can extract any variant's slots without going out of range:
        //      Rect(3, 4) -> { i8 1, double 3.0, double 4.0 }
        //      Circle(9)  -> { i8 0, double 9.0, double <undef> }   (slot 1 unused)
        //  - `Result` has NO registered layout: it's sized to the actual payload value,
        //    preserving the historical per-value representation across its generic,
        //    possibly-heterogeneous variants:
        //      Ok(42)       -> { i8 0, double 42.0 }
        //      NotOk("err") -> { i8 1, ptr <str> }
        //
        // Num/Bool payloads are normalized to f64. A `$` (Unit) payload is zero-sized; it
        // is stored as a zero of the slot type so the value still matches the slot/return
        // shape (e.g. `Ok($)` -> { i8 0, double 0.0 }) — the bits are never read.
        let i8_type = self.context.i8_type();
        let f64_type = self.context.f64_type();
        let registered_layout = self.sum_layouts.get(type_name).cloned();

        let tag_val = i8_type.const_int(tag as u64, false);

        // Determine each payload slot's value and type. For a registered layout, the slot
        // type is fixed by position; otherwise (Result) it follows the value, with a `$`
        // payload defaulting to the canonical `double` slot.
        let mut payload_vals: Vec<BasicValueEnum> = Vec::with_capacity(args.len());
        for (pos, arg) in args.iter().enumerate() {
            let arg_val = self.generate_expr(arg)?;
            // With a registered layout (user type), the slot type is fixed by position.
            // Without one (Result), the slot follows the value's own type so a Text/Bool
            // payload keeps its real representation — except a `$` (Unit) value, which is
            // zero-sized and defaults to the canonical `double` slot.
            let slot_ty = match registered_layout.as_ref().and_then(|l| l.get(pos).copied()) {
                Some(ty) => ty,
                None if self.expr_is_unit(arg) => f64_type.into(),
                None => self.payload_slot_type(arg_val),
            };
            payload_vals.push(self.coerce_payload(arg_val, slot_ty)?);
        }

        // Build the struct type: tag + (registered layout, or the actual payload types).
        let mut field_types: Vec<BasicTypeEnum> = vec![i8_type.into()];
        match &registered_layout {
            Some(layout) => field_types.extend(layout.iter().copied()),
            None => field_types.extend(payload_vals.iter().map(|v| v.get_type())),
        }
        let sum_struct = self.context.struct_type(&field_types, false);

        let mut agg = sum_struct.get_undef();
        agg = self
            .builder
            .build_insert_value(agg, tag_val, 0, "with_tag")
            .map_err(|e| format!("Failed to insert tag: {:?}", e))?
            .into_struct_value();
        // Fill the leading slots with this variant's payloads; trailing slots (unused by
        // this variant, in a wider registered layout) stay `undef` — they're only read by
        // an arm matching a different, wider variant, which never runs for this value.
        for (i, payload) in payload_vals.iter().enumerate() {
            agg = self
                .builder
                .build_insert_value(agg, *payload, (i + 1) as u32, "with_payload")
                .map_err(|e| format!("Failed to insert payload: {:?}", e))?
                .into_struct_value();
        }

        Ok(agg.into())
    }

    /// The slot type for a Result payload sized to its actual value: a non-`i1` integer
    /// widens to f64 (the canonical numeric payload), everything else keeps its own type.
    fn payload_slot_type(&self, value: BasicValueEnum<'ctx>) -> BasicTypeEnum<'ctx> {
        match value {
            BasicValueEnum::IntValue(i) if i.get_type().get_bit_width() != 1 => {
                self.context.f64_type().into()
            }
            other => other.get_type(),
        }
    }

    /// Coerce a payload argument value to its target slot type. Integers (incl. the unit
    /// `i8`) widen to f64 for a numeric slot; a `$` (Unit) value targeting a non-`i8` slot
    /// becomes a zero of that slot type (it carries no information). Otherwise the value
    /// is stored as-is (e.g. a Text struct into a Text slot).
    fn coerce_payload(
        &self,
        value: BasicValueEnum<'ctx>,
        slot_ty: BasicTypeEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        match value {
            BasicValueEnum::IntValue(i) if slot_ty.is_float_type() => Ok(self
                .builder
                .build_unsigned_int_to_float(i, slot_ty.into_float_type(), "inttofloat")
                .map_err(|e| format!("Failed to convert payload to float: {:?}", e))?
                .into()),
            // A value already matching the slot type passes through unchanged.
            other if other.get_type() == slot_ty => Ok(other),
            // A `$` (Unit) value — the zero `i8` — carries no information; stored into a
            // differently-typed slot it becomes that slot's zero (e.g. a `$` payload in a
            // `Done($) / Pending(Text)` Text slot). The type checker guarantees concrete
            // payload types agree per position, so ANY other mismatch is an internal bug,
            // surfaced rather than silently zeroed.
            BasicValueEnum::IntValue(i) if i.get_type().get_bit_width() == 8 => Ok(zeroed(slot_ty)),
            other => Err(format!(
                "internal error: sum-type payload of type {:?} does not fit slot {:?}",
                other.get_type(),
                slot_ty
            )),
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

    /// Materialize an inclusive range `lo <- hi` into a `[]Num` (the `{ptr, size}`
    /// array shape, same as `generate_array`). The element count is `|hi - lo| + 1`
    /// and the direction (ascending vs descending) is decided at runtime, since the
    /// ends can be dynamic: `lo <= hi` counts up (`1 <- 4` → `[1,2,3,4]`), otherwise
    /// down (`4 <- 1` → `[4,3,2,1]`). The backing storage is GC-allocated (`__alloc`)
    /// so the array may safely escape the current frame.
    fn generate_range(&mut self, start: &Expr, end: &Expr) -> Result<BasicValueEnum<'ctx>, String> {
        let function = self
            .current_function
            .ok_or_else(|| "Range must be in a function".to_string())?;

        let i64_type = self.context.i64_type();
        let f64_type = self.context.f64_type();

        // Evaluate both ends (Num = f64) and truncate to i64 endpoints.
        let lo_f = self.generate_expr(start)?.into_float_value();
        let hi_f = self.generate_expr(end)?.into_float_value();
        let lo = self
            .builder
            .build_float_to_signed_int(lo_f, i64_type, "range_lo")
            .map_err(|e| format!("Failed to convert range start: {:?}", e))?;
        let hi = self
            .builder
            .build_float_to_signed_int(hi_f, i64_type, "range_hi")
            .map_err(|e| format!("Failed to convert range end: {:?}", e))?;

        // Ascending iff lo <= hi; pick step = +1 / -1 and the inclusive span.
        let ascending = self
            .builder
            .build_int_compare(inkwell::IntPredicate::SLE, lo, hi, "range_asc")
            .map_err(|e| format!("Failed to compare range ends: {:?}", e))?;
        let one = i64_type.const_int(1, false);
        let neg_one = i64_type.const_all_ones(); // -1 in two's complement
        let step = self
            .builder
            .build_select(ascending, one, neg_one, "range_step")
            .map_err(|e| format!("Failed to select range step: {:?}", e))?
            .into_int_value();
        // |hi - lo| + 1: compute the signed delta once, then pick it or its
        // negation so the span is non-negative in either direction.
        let delta = self
            .builder
            .build_int_sub(hi, lo, "range_delta")
            .map_err(|e| format!("Failed to subtract range ends: {:?}", e))?;
        let neg_delta = self
            .builder
            .build_int_neg(delta, "range_neg_delta")
            .map_err(|e| format!("Failed to negate range delta: {:?}", e))?;
        let span_abs = self
            .builder
            .build_select(ascending, delta, neg_delta, "range_span")
            .map_err(|e| format!("Failed to select range span: {:?}", e))?
            .into_int_value();
        let count = self
            .builder
            .build_int_add(span_abs, one, "range_count")
            .map_err(|e| format!("Failed to add range count: {:?}", e))?;

        // GC-allocate count * sizeof(f64) bytes for the backing data.
        let eight = i64_type.const_int(8, false);
        let bytes = self
            .builder
            .build_int_mul(count, eight, "range_bytes")
            .map_err(|e| format!("Failed to size range alloc: {:?}", e))?;
        let alloc = self.get_intrinsic("__alloc")?;
        let alloc_call = self
            .builder
            .build_call(alloc, &[bytes.into()], "range_data")
            .map_err(|e| format!("Failed to allocate range: {:?}", e))?;
        let data_ptr = {
            use inkwell::values::AnyValue;
            alloc_call.as_any_value_enum().into_pointer_value()
        };

        // Fill loop: for i in 0..count: data[i] = (f64)(lo + i*step).
        let counter = self.create_entry_block_alloca("range_i", i64_type.into())?;
        self.builder
            .build_store(counter, i64_type.const_zero())
            .map_err(|e| format!("Failed to init range counter: {:?}", e))?;

        let header = self.context.append_basic_block(function, "range_header");
        let body = self.context.append_basic_block(function, "range_body");
        let exit = self.context.append_basic_block(function, "range_exit");

        self.builder
            .build_unconditional_branch(header)
            .map_err(|e| format!("Failed to branch to range header: {:?}", e))?;

        self.builder.position_at_end(header);
        let i = self
            .builder
            .build_load(i64_type, counter, "i")
            .map_err(|e| format!("Failed to load range counter: {:?}", e))?
            .into_int_value();
        let cond = self
            .builder
            .build_int_compare(inkwell::IntPredicate::SLT, i, count, "range_cond")
            .map_err(|e| format!("Failed to build range condition: {:?}", e))?;
        self.builder
            .build_conditional_branch(cond, body, exit)
            .map_err(|e| format!("Failed to build range branch: {:?}", e))?;

        self.builder.position_at_end(body);
        // value = lo + i*step
        let i_step = self
            .builder
            .build_int_mul(i, step, "range_i_step")
            .map_err(|e| format!("Failed to scale range index: {:?}", e))?;
        let val_i = self
            .builder
            .build_int_add(lo, i_step, "range_val_i")
            .map_err(|e| format!("Failed to compute range element: {:?}", e))?;
        let val_f = self
            .builder
            .build_signed_int_to_float(val_i, f64_type, "range_val")
            .map_err(|e| format!("Failed to convert range element: {:?}", e))?;
        let elem_ptr = unsafe {
            self.builder
                .build_gep(f64_type, data_ptr, &[i], "range_elem")
                .map_err(|e| format!("Failed to index range data: {:?}", e))?
        };
        self.builder
            .build_store(elem_ptr, val_f)
            .map_err(|e| format!("Failed to store range element: {:?}", e))?;
        let next = self
            .builder
            .build_int_add(i, one, "range_next")
            .map_err(|e| format!("Failed to increment range counter: {:?}", e))?;
        self.builder
            .build_store(counter, next)
            .map_err(|e| format!("Failed to store range counter: {:?}", e))?;
        self.builder
            .build_unconditional_branch(header)
            .map_err(|e| format!("Failed to loop range: {:?}", e))?;

        // Build the { ptr, size } array struct (the shared array/Text shape).
        self.builder.position_at_end(exit);
        let array_struct_type = self.ptr_len_struct_type();
        let array_struct = self
            .builder
            .build_alloca(array_struct_type, "range_array")
            .map_err(|e| format!("Failed to allocate range struct: {:?}", e))?;
        let ptr_field = self
            .builder
            .build_struct_gep(array_struct_type, array_struct, 0, "range_ptr_field")
            .map_err(|e| format!("Failed to get range ptr field: {:?}", e))?;
        self.builder
            .build_store(ptr_field, data_ptr)
            .map_err(|e| format!("Failed to store range ptr: {:?}", e))?;
        let size_field = self
            .builder
            .build_struct_gep(array_struct_type, array_struct, 1, "range_size_field")
            .map_err(|e| format!("Failed to get range size field: {:?}", e))?;
        self.builder
            .build_store(size_field, count)
            .map_err(|e| format!("Failed to store range size: {:?}", e))?;
        self.builder
            .build_load(array_struct_type, array_struct, "range_array")
            .map_err(|e| format!("Failed to load range struct: {:?}", e))
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
            // GC-allocate the struct (not a stack alloca) so a record VALUE can outlive
            // the frame that built it — e.g. a record returned from a function or a user
            // operator overload (`+ = (a :: Vec, b :: Vec) -> Vec => Vec { ... }`). A
            // stack alloca would dangle once the callee returned.
            use inkwell::values::AnyValue;
            let size = struct_type
                .size_of()
                .ok_or_else(|| "record struct type has no compile-time size".to_string())?;
            let alloc_fn = self.get_intrinsic("__alloc")?;
            let record_ptr = self
                .builder
                .build_call(alloc_fn, &[size.into()], "record")
                .map_err(|e| format!("Failed to call __alloc for record: {:?}", e))?
                .as_any_value_enum()
                .into_pointer_value();

            // Store each field
            for (i, value) in field_values.iter().enumerate() {
                let gep = self
                    .builder
                    .build_struct_gep(struct_type, record_ptr, i as u32, &format!("field_{}", i))
                    .map_err(|e| format!("Failed to build GEP: {:?}", e))?;
                self.builder
                    .build_store(gep, *value)
                    .map_err(|e| format!("Failed to build store: {:?}", e))?;
            }

            Ok(record_ptr.into())
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

    /// Build a direct call to an already-emitted function by symbol, given the
    /// already-generated argument values. Used to lower a resolved operator/function
    /// overload to its mangled target.
    fn build_direct_call(
        &mut self,
        symbol: &str,
        arg_values: &[BasicValueEnum<'ctx>],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let function = self
            .module
            .get_function(symbol)
            .ok_or_else(|| format!("Overload not found: {}", symbol))?;
        let arg_metadata: Vec<inkwell::values::BasicMetadataValueEnum> =
            arg_values.iter().map(|v| (*v).into()).collect();
        use inkwell::values::AnyValue;
        let call_site = self
            .builder
            .build_call(function, &arg_metadata, "calltmp")
            .map_err(|e| format!("Failed to build call: {:?}", e))?;
        match call_site.as_any_value_enum() {
            inkwell::values::AnyValueEnum::IntValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::FloatValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::PointerValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::ArrayValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::StructValue(v) => Ok(v.into()),
            inkwell::values::AnyValueEnum::VectorValue(v) => Ok(v.into()),
            _ => Err("Overloaded function does not return a basic value".to_string()),
        }
    }

    /// Lower a `Text`-vs-`Text` comparison: call `__text_cmp(aptr, alen, bptr, blen)`
    /// (returns -1/0/1, memcmp-style with the shorter string ordering first on a common
    /// prefix), then compare that i32 result against 0 with the predicate matching `op`.
    /// Backs `Text` equality and lexicographic ordering (`==`/`!=`/`<`/`<=`/`>`/`>=`).
    fn generate_text_compare(
        &mut self,
        op: BinOp,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let (BasicValueEnum::StructValue(l), BasicValueEnum::StructValue(r)) = (lhs, rhs) else {
            return Err("Text comparison requires two Text values".to_string());
        };
        let extract = |s: inkwell::values::StructValue<'ctx>,
                       idx: u32,
                       name: &str|
         -> Result<BasicValueEnum<'ctx>, String> {
            self.builder
                .build_extract_value(s, idx, name)
                .map_err(|e| format!("Failed to extract text field: {:?}", e))
        };
        let l_ptr = extract(l, 0, "lcmp_ptr")?.into_pointer_value();
        let l_len = extract(l, 1, "lcmp_len")?.into_int_value();
        let r_ptr = extract(r, 0, "rcmp_ptr")?.into_pointer_value();
        let r_len = extract(r, 1, "rcmp_len")?.into_int_value();

        let cmp_fn = self.get_intrinsic("__text_cmp")?;
        use inkwell::values::AnyValue;
        let cmp = self
            .builder
            .build_call(
                cmp_fn,
                &[l_ptr.into(), l_len.into(), r_ptr.into(), r_len.into()],
                "text_cmp",
            )
            .map_err(|e| format!("Failed to call __text_cmp: {:?}", e))?
            .as_any_value_enum()
            .into_int_value();

        let pred = match op {
            BinOp::Eq => inkwell::IntPredicate::EQ,
            BinOp::Ne => inkwell::IntPredicate::NE,
            BinOp::Lt => inkwell::IntPredicate::SLT,
            BinOp::Le => inkwell::IntPredicate::SLE,
            BinOp::Gt => inkwell::IntPredicate::SGT,
            BinOp::Ge => inkwell::IntPredicate::SGE,
            _ => return Err("non-comparison operator in text compare".to_string()),
        };
        let zero = cmp.get_type().const_zero();
        Ok(self
            .builder
            .build_int_compare(pred, cmp, zero, "text_cmp_res")
            .map_err(|e| format!("Failed to build text compare: {:?}", e))?
            .into())
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

        // Regular record field access: resolve a pointer to the field inside the
        // record's memory (shared by the in-place field-write path) and load it.
        if let Some(field_ptr) = self.record_field_pointer(expr, field_name)? {
            return self
                .builder
                .build_load(self.context.f64_type(), field_ptr, field_name)
                .map_err(|e| format!("Failed to load field: {:?}", e));
        }

        Err(format!(
            "Field access not fully implemented. Need type information for field '{}'",
            field_name
        ))
    }

    /// In-place field write `target := value`, where `target` is a field access
    /// `obj.field`. Computes a pointer into the existing record memory via GEP and
    /// stores `value` there — no re-allocation — so the mutation is observable
    /// through every alias of the record. Yields `$` (a unit i8), matching the
    /// type checker's `Unit` result for a field write.
    fn generate_field_assign(
        &mut self,
        target: &Expr,
        value: &Expr,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let Expr::FieldAccess { expr, field, .. } = target else {
            return Err("Field-write target must be a field access".to_string());
        };
        let new_value = self.generate_expr(value)?;
        let field_ptr = self
            .record_field_pointer(expr, field)?
            .ok_or_else(|| format!("Unknown record for field write: {}", field))?;
        self.builder
            .build_store(field_ptr, new_value)
            .map_err(|e| format!("Failed to store field: {:?}", e))?;
        Ok(self.unit_value().into())
    }

    /// Pointer to `base.field` inside the record's memory, the shared primitive for
    /// both reads (`generate_field_access`) and in-place writes
    /// (`generate_field_assign`). `base` must be a record-typed identifier (a
    /// variable such as `u`, or the method receiver `it`); the variable's alloca
    /// holds a pointer to the struct. Records are a flat struct of f64 fields (the
    /// current numeric-record layout), so a field cannot itself hold a record —
    /// chained paths (`a.b.c`) are rejected by the type checker before reaching
    /// codegen. Returns `Ok(None)` when `base` isn't a tracked record (so the read
    /// path can fall through to its Text/array `.size` handling).
    fn record_field_pointer(
        &mut self,
        base: &Expr,
        field: &str,
    ) -> Result<Option<PointerValue<'ctx>>, String> {
        let Expr::Ident { name, .. } = base else {
            return Ok(None);
        };
        let Some(field_names) = self.record_types.get(name).cloned() else {
            return Ok(None);
        };
        let Some(field_idx) = field_names.iter().position(|f| f == field) else {
            return Ok(None);
        };

        // The variable's alloca holds a pointer to the struct; load it.
        let (var_ptr, _) = self
            .variables
            .get(name)
            .ok_or_else(|| format!("Variable not found: {}", name))?;
        let struct_ptr = self
            .builder
            .build_load(
                self.context.ptr_type(AddressSpace::default()),
                *var_ptr,
                "load_struct_ptr",
            )
            .map_err(|e| format!("Failed to load struct pointer: {:?}", e))?
            .into_pointer_value();

        // Reconstruct the (all-f64) struct type for the GEP.
        let field_types: Vec<BasicTypeEnum> =
            vec![self.context.f64_type().into(); field_names.len()];
        let struct_type = self.context.struct_type(&field_types, false);

        let field_ptr = self
            .builder
            .build_struct_gep(
                struct_type,
                struct_ptr,
                field_idx as u32,
                &format!("field_{}_ptr", field),
            )
            .map_err(|e| format!("Failed to build field GEP: {:?}", e))?;
        Ok(Some(field_ptr))
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
                // Tagged-union dispatch: a value is `{ i8 tag, <payload> }`; the tag is
                // the variant's declaration index, looked up from the sum-variant
                // registry (generalizes the old hardcoded Ok=0/NotOk=1).
                let expected_tag = self
                    .sum_variants
                    .get(name.as_str())
                    .map(|(tag, _)| *tag)
                    .ok_or_else(|| format!("Unknown constructor: {}", name))?;

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
                // Extract each payload field and bind it to the corresponding sub-pattern.
                // The value is `{ i8 tag, payload0, payload1, ... }`, so payload `i` is
                // struct field `i + 1`. Only identifier sub-patterns bind a name; others
                // (wildcards, nested constructors) are matched structurally elsewhere.
                let payload_types = self.variant_payloads.get(name).cloned();
                if let BasicValueEnum::StructValue(struct_val) = value {
                    for (i, arg) in args.iter().enumerate() {
                        if let Pattern::Ident { name: arg_name, .. } = arg {
                            let payload = self
                                .builder
                                .build_extract_value(struct_val, (i + 1) as u32, "payload")
                                .map_err(|e| format!("Failed to extract payload: {:?}", e))?;
                            let alloca =
                                self.create_entry_block_alloca(arg_name, payload.get_type())?;
                            self.builder
                                .build_store(alloca, payload)
                                .map_err(|e| format!("Failed to store constructor arg: {:?}", e))?;
                            self.variables
                                .insert(arg_name.clone(), (alloca, payload.get_type()));
                            // Track the payload binding's declared Quilon type so an
                            // overloaded call on it (`Circle(n) => area(n)`) mangles by
                            // the concrete payload type, agreeing with the type checker.
                            if let Some(ty) = payload_types.as_ref().and_then(|t| t.get(i)) {
                                self.var_types.insert(arg_name.clone(), ty.clone());
                            }
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
                // Loop elements are loaded as Num (codegen supports numeric arrays);
                // track it so an overloaded call on the loop var mangles correctly.
                self.var_types.insert(name.clone(), Type::Num);
            }
            ForPattern::ItemIndex { item, index, .. } => {
                // Bind item
                let item_alloca = self.create_entry_block_alloca(item, elem_val.get_type())?;
                self.builder
                    .build_store(item_alloca, elem_val)
                    .map_err(|e| format!("Failed to store item: {:?}", e))?;
                self.variables
                    .insert(item.clone(), (item_alloca, elem_val.get_type()));
                self.var_types.insert(item.clone(), Type::Num);
                self.var_types.insert(index.clone(), Type::Num);

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

    /// The tagged-union LLVM struct for a sum type: `{ i8 tag, slot0, slot1, ... }`,
    /// where the slots come from the registered canonical payload layout. Falls back to
    /// the Result-style `{ i8, double }` for an unregistered name (e.g. a `-> Result`
    /// annotation reached before any user declaration), keeping the historical shape.
    fn sum_struct_type(&self, name: &str) -> inkwell::types::StructType<'ctx> {
        let mut field_types: Vec<BasicTypeEnum> = vec![self.context.i8_type().into()];
        match self.sum_layouts.get(name) {
            Some(layout) => field_types.extend(layout.iter().copied()),
            None => field_types.push(self.context.f64_type().into()),
        }
        self.context.struct_type(&field_types, false)
    }

    /// The single value of the Unit type (`$`), lowered as a zero `i8`. Its bits are
    /// never observed; the entry-point wrapper coerces a non-Num body to exit code 0.
    fn unit_value(&self) -> inkwell::values::IntValue<'ctx> {
        self.context.i8_type().const_int(0, false)
    }

    /// Whether `expr`'s value has type Unit (`$`). Codegen lacks the checker's full
    /// inference, so for an *unannotated* function we look at the body's tail to pick
    /// the LLVM return type: a Unit tail must be `i8`, not the `Num`/f64 default. The
    /// only Unit-producing expressions are the `$` literal and `print`/`eprint` calls
    /// (which return `$`); a block/ternary is Unit when its tail is. Other unannotated
    /// non-Num bodies (Text, Bool, ...) keep the pre-existing `Num`-default behavior.
    fn expr_is_unit(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Unit { .. } => true,
            // An in-place field write `obj.field := v` is an effect; it yields `$`.
            Expr::FieldAssign { .. } => true,
            Expr::Call { func, .. } => {
                matches!(func.as_ref(), Expr::Ident { name, .. } if name == "print" || name == "eprint")
            }
            Expr::Block { stmts, .. } => match stmts.last() {
                Some(crate::ast::Statement::Expr(tail)) => self.expr_is_unit(tail),
                _ => false,
            },
            Expr::If { then, else_, .. } => self.expr_is_unit(then) && self.expr_is_unit(else_),
            _ => false,
        }
    }

    /// Best-effort Quilon type of `expr`, sufficient to mangle overloaded call sites.
    /// Codegen lacks the type checker's full inference, so this covers exactly the
    /// shapes that can be an overloaded argument: literals, locals/params (tracked in
    /// `var_types`), constructor results, field access on a known record, and the
    /// result types of the supported operators. Falls back to `Num` (the historical
    /// default) when it can't tell — overloaded dispatch then simply won't match and a
    /// clear "function not found" surfaces, never a silent miscompile.
    fn infer_type(&self, expr: &Expr) -> Type {
        match expr {
            Expr::Number { .. } => Type::Num,
            Expr::String { .. } => Type::Text,
            Expr::Bool { .. } => Type::Bool,
            Expr::Unit { .. } => Type::Unit,
            Expr::Ident { name, .. } => {
                // A bare nullary sum-type constructor (not a bound variable) is a value
                // of its sum type.
                if let Some((_, type_name)) = self.sum_variants.get(name)
                    && !self.var_types.contains_key(name)
                {
                    return self.sum_or_named(type_name);
                }
                self.var_types.get(name).cloned().unwrap_or(Type::Num)
            }
            Expr::Constructor { type_name, .. } => self.sum_or_named(type_name),
            Expr::Call { func, args, .. } => {
                if let Expr::Ident { name, .. } = func.as_ref() {
                    // A constructor call yields its sum type.
                    if let Some((_, type_name)) = self.sum_variants.get(name) {
                        return self.sum_or_named(type_name);
                    }
                    // An overloaded function call yields its resolved member's return.
                    let arg_types: Vec<Type> = args.iter().map(|a| self.infer_type(a)).collect();
                    if let Some((_, ret)) = self.matching_overload(name, &arg_types) {
                        return ret.clone();
                    }
                    // A non-overloaded top-level function yields its declared return
                    // type — so a call result that feeds an overloaded call/operator
                    // mangles to the right member (codegen agrees with the checker).
                    if let Some(ret) = self.fn_return_types.get(name) {
                        return self.resolve_named(ret);
                    }
                }
                // Unknown callee (no declared return, e.g. an unannotated function):
                // default to Num, the historical inference default.
                Type::Num
            }
            Expr::BinOp {
                left, op, right, ..
            } => {
                // A user operator overload yields its resolved member's return type.
                let sym = op.symbol();
                if self.overloads.contains_key(sym) {
                    let arg_types = [self.infer_type(left), self.infer_type(right)];
                    if let Some((_, ret)) = self.matching_overload(sym, &arg_types) {
                        return ret.clone();
                    }
                }
                // Built-ins: comparisons/logicals yield Bool; `+` on Text yields Text;
                // arithmetic yields Num. Matches the type checker's operator results
                // closely enough to mangle a nested overloaded argument.
                match op {
                    BinOp::Eq
                    | BinOp::Ne
                    | BinOp::Lt
                    | BinOp::Le
                    | BinOp::Gt
                    | BinOp::Ge
                    | BinOp::And
                    | BinOp::Or => Type::Bool,
                    BinOp::Add
                        if self.infer_type(left) == Type::Text
                            || self.infer_type(right) == Type::Text =>
                    {
                        Type::Text
                    }
                    _ => Type::Num,
                }
            }
            Expr::If { then, .. } => self.infer_type(then),
            Expr::Block { stmts, .. } => match stmts.last() {
                Some(crate::ast::Statement::Expr(tail)) => self.infer_type(tail),
                _ => Type::Num,
            },
            Expr::FieldAccess { field, .. } if field == "size" || field == "length" => Type::Num,
            _ => Type::Num,
        }
    }

    /// Normalize a declared type annotation for `infer_type`: a bare `Named { name }`
    /// (the parser's form for a `:: SomeType` reference) becomes the canonical sum/named
    /// tag so it mangles identically to an inferred value of that type. Built-ins pass
    /// through unchanged.
    fn resolve_named(&self, ty: &Type) -> Type {
        match ty {
            Type::Named { name, .. } | Type::Sum { name, .. } => self.sum_or_named(name),
            other => other.clone(),
        }
    }

    /// The `Type` for a registered type name: a sum type if known, else a `Named`.
    fn sum_or_named(&self, name: &str) -> Type {
        if self.sum_layouts.contains_key(name) || name == "Result" {
            Type::Sum {
                name: name.to_string(),
                variants: vec![],
            }
        } else {
            Type::Named {
                name: name.to_string(),
                fields: vec![],
                methods: vec![],
            }
        }
    }

    /// If `name` is an overload set, pick the member matching `arg_types` exactly and
    /// return its mangled LLVM symbol. `None` if `name` isn't overloaded or nothing
    /// matches (the caller then falls back to its non-overloaded path).
    fn resolve_overload_symbol(&self, name: &str, arg_types: &[Type]) -> Option<String> {
        let (params, _) = self.matching_overload(name, arg_types)?;
        Some(mangle_overload(name, params))
    }

    /// The overload member of `name` whose parameter types match `arg_types` exactly
    /// (by type tag), if any. Shared by symbol resolution and return-type inference.
    fn matching_overload(&self, name: &str, arg_types: &[Type]) -> Option<&(Vec<Type>, Type)> {
        self.overloads.get(name)?.iter().find(|(params, _)| {
            params.len() == arg_types.len()
                && params
                    .iter()
                    .zip(arg_types)
                    .all(|(p, a)| type_mangle(p) == type_mangle(a))
        })
    }

    fn type_to_llvm(&self, ty: &Type) -> Result<BasicTypeEnum<'ctx>, String> {
        match ty {
            Type::Num => Ok(self.context.f64_type().into()),
            Type::Bool => Ok(self.context.bool_type().into()),
            // Unit (`$`) is a zero `i8` — a concrete one-inhabitant placeholder.
            Type::Unit => Ok(self.context.i8_type().into()),
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
            Type::Sum { name, .. } => Ok(self.sum_struct_type(name).into()),
            // A `Named` reference with no fields is a parsed type annotation (e.g. a
            // function param `s :: Shape`). If it names a registered sum type, lower it
            // to that type's tagged-union struct.
            Type::Named { name, fields, .. }
                if fields.is_empty() && self.sum_layouts.contains_key(name) =>
            {
                Ok(self.sum_struct_type(name).into())
            }
            // Any other named RECORD type (a `:: SomeRecord` parameter/return, e.g. on a
            // user operator overload) is passed by pointer — record instances are
            // represented as a pointer to their struct alloca (see `generate_record`).
            Type::Named { .. } => Ok(self.context.ptr_type(AddressSpace::default()).into()),
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
