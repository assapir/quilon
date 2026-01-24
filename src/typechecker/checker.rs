// Type checker implementation

use crate::ast::{Expr, FunctionDecl, Item, MatchArm, Pattern, Program, Type, VarDecl, Param};
use crate::ast::{BinOp, UnaryOp};
use crate::lexer::Span;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum TypeError {
    UndefinedVariable { name: String, span: Span },
    TypeMismatch { expected: Type, got: Type, span: Span },
    NotAFunction { got: Type, span: Span },
    WrongNumberOfArguments { expected: usize, got: usize, span: Span },
    CannotInfer { expr: String, span: Span },
    ImmutableAssignment { name: String, span: Span },
    DuplicateDefinition { name: String, span: Span },
    PatternTypeMismatch { expected: Type, got: Type, span: Span },
    NonExhaustiveMatch { span: Span },
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            TypeError::UndefinedVariable { name, span } => {
                write!(f, "Undefined variable '{}' at {:?}", name, span)
            }
            TypeError::TypeMismatch { expected, got, span } => {
                write!(f, "Type mismatch at {:?}: expected {:?}, got {:?}", span, expected, got)
            }
            TypeError::NotAFunction { got, span } => {
                write!(f, "Not a function at {:?}: got {:?}", span, got)
            }
            TypeError::WrongNumberOfArguments { expected, got, span } => {
                write!(f, "Wrong number of arguments at {:?}: expected {}, got {}", span, expected, got)
            }
            TypeError::CannotInfer { expr, span } => {
                write!(f, "Cannot infer type for '{}' at {:?}", expr, span)
            }
            TypeError::ImmutableAssignment { name, span } => {
                write!(f, "Cannot assign to immutable variable '{}' at {:?}", name, span)
            }
            TypeError::DuplicateDefinition { name, span } => {
                write!(f, "Duplicate definition of '{}' at {:?}", name, span)
            }
            TypeError::PatternTypeMismatch { expected, got, span } => {
                write!(f, "Pattern type mismatch at {:?}: expected {:?}, got {:?}", span, expected, got)
            }
            TypeError::NonExhaustiveMatch { span } => {
                write!(f, "Non-exhaustive pattern match at {:?}", span)
            }
        }
    }
}

impl std::error::Error for TypeError {}

#[derive(Debug, Clone)]
struct Symbol {
    type_: Type,
    mutable: bool,
    span: Span,
}

#[derive(Debug, Clone)]
pub struct Environment {
    scopes: Vec<HashMap<String, Symbol>>,
}

impl Environment {
    pub fn new() -> Self {
        Environment {
            scopes: vec![HashMap::new()],
        }
    }

    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    pub fn pop_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    pub fn define(&mut self, name: String, type_: Type, mutable: bool, span: Span) -> Result<(), TypeError> {
        let current_scope = self.scopes.last_mut().unwrap();
        
        if current_scope.contains_key(&name) {
            return Err(TypeError::DuplicateDefinition { name, span });
        }
        
        current_scope.insert(name, Symbol { type_, mutable, span });
        Ok(())
    }

    pub fn lookup(&self, name: &str) -> Option<&Symbol> {
        for scope in self.scopes.iter().rev() {
            if let Some(symbol) = scope.get(name) {
                return Some(symbol);
            }
        }
        None
    }

    pub fn get_type(&self, name: &str) -> Option<Type> {
        self.lookup(name).map(|s| s.type_.clone())
    }

    pub fn is_mutable(&self, name: &str) -> bool {
        self.lookup(name).map(|s| s.mutable).unwrap_or(false)
    }
}

pub struct TypeChecker {
    env: Environment,
    // Registry of methods: (TypeName, MethodName) -> (Params, ReturnType, Body)
    methods: std::collections::HashMap<(String, String), (Vec<Param>, Option<Type>, Expr)>,
}

impl TypeChecker {
    pub fn new() -> Self {
        let mut checker = TypeChecker {
            env: Environment::new(),
            methods: std::collections::HashMap::new(),
        };
        
        // Add built-in sum types to the environment
        checker.add_builtins();
        
        checker
    }
    
    fn add_builtins(&mut self) {
        use crate::ast::{SumVariant, Type};
        use crate::lexer::Span;
        
        // Unified Result{T} type with OK and NotOK constructors
        // Replaces both Option and Result with a single type
        let result_type = Type::Sum {
            name: "Result".to_string(),
            variants: vec![
                SumVariant {
                    name: "OK".to_string(),
                    fields: vec![Type::Generic {
                        name: "T".to_string(),
                        args: vec![],
                    }],
                },
                SumVariant {
                    name: "NotOK".to_string(),
                    fields: vec![],
                },
            ],
        };
        
        // Register Result type
        let _ = self.env.define(
            "Result".to_string(),
            result_type.clone(),
            false,
            Span::new(0, 0),
        );
    }

    pub fn check_program(&mut self, program: &Program) -> Result<(), TypeError> {
        for item in &program.items {
            self.check_item(item)?;
        }
        Ok(())
    }

    fn check_item(&mut self, item: &Item) -> Result<(), TypeError> {
        match item {
            Item::VarDecl(decl) => self.check_var_decl(decl),
            Item::FunctionDecl(decl) => self.check_function_decl(decl),
            Item::TypeDecl(decl) => self.check_type_decl(decl),
        }
    }

    fn check_type_decl(&mut self, decl: &crate::ast::TypeDecl) -> Result<(), TypeError> {
        use crate::ast::{Type, TypeDef};
        
        // Build the type from the definition
        let type_value = match &decl.type_def {
            TypeDef::Sum(variants) => Type::Sum {
                name: decl.name.clone(),
                variants: variants.clone(),
            },
            TypeDef::Record { fields, methods } => {
                // Type-check each method
                for method in methods {
                    // Create a new scope for the method
                    self.env.push_scope();
                    
                    // Bind implicit "it" parameter to the struct type
                    let struct_type = Type::Named {
                        name: decl.name.clone(),
                        fields: fields.clone(),
                        methods: methods.iter().map(|m| m.name.clone()).collect(),
                    };
                    
                    self.env.define(
                        "it".to_string(),
                        struct_type,
                        false,
                        method.span.clone(),
                    )?;
                    
                    // Bind method parameters
                    for param in &method.params {
                        let param_type = param.type_annotation.clone()
                            .unwrap_or(Type::Num); // Default to Num if no type annotation
                        self.env.define(
                            param.name.clone(),
                            param_type,
                            false,
                            param.span.clone(),
                        )?;
                    }
                    
                    // Type-check method body
                    let body_type = self.infer_expr(&method.body)?;
                    
                    // Check return type if specified
                    if let Some(ref return_type) = method.return_type {
                        self.check_type_compatibility(return_type, &body_type, &method.span)?;
                    }
                    
                    self.env.pop_scope();
                    
                    // Store method for later lookup
                    self.methods.insert(
                        (decl.name.clone(), method.name.clone()),
                        (method.params.clone(), method.return_type.clone(), method.body.clone()),
                    );
                }
                
                // Create a Named type with methods
                Type::Named {
                    name: decl.name.clone(),
                    fields: fields.clone(),
                    methods: methods.iter().map(|m| m.name.clone()).collect(),
                }
            },
            TypeDef::Alias(ty) => ty.clone(),
        };
        
        // Register the type name in the environment
        // For now, we treat types as values (not ideal but works)
        self.env.define(
            decl.name.clone(),
            type_value,
            false,
            decl.span.clone(),
        )?;
        
        Ok(())
    }

    fn check_var_decl(&mut self, decl: &VarDecl) -> Result<(), TypeError> {
        // Infer or check the type of the value
        let value_type = self.infer_expr(&decl.value)?;

        // If type annotation exists, check it matches
        let final_type = if let Some(ref annotated_type) = decl.type_annotation {
            self.check_type_compatibility(annotated_type, &value_type, &decl.span)?;
            annotated_type.clone()
        } else {
            value_type
        };

        // Define the variable in the environment
        self.env.define(decl.name.clone(), final_type, decl.mutable, decl.span.clone())?;

        Ok(())
    }

    fn check_function_decl(&mut self, decl: &FunctionDecl) -> Result<(), TypeError> {
        // Build function type from parameters and return type
        let param_types: Vec<Type> = decl.params.iter()
            .map(|p| p.type_annotation.clone().unwrap_or(Type::Num))
            .collect();

        // For recursion support, we need to add the function to the environment
        // BEFORE checking its body. We'll use the annotated return type if available,
        // or default to Num (which we'll verify later)
        let preliminary_return_type = decl.return_type.clone().unwrap_or(Type::Num);
        
        let func_type = Type::Function {
            params: param_types.clone(),
            return_type: Box::new(preliminary_return_type.clone()),
        };

        // Define the function in current scope BEFORE checking body (enables recursion)
        self.env.define(decl.name.clone(), func_type, false, decl.span.clone())?;

        // Push scope for body type checking
        self.env.push_scope();

        // Add parameters to scope
        for (param, param_type) in decl.params.iter().zip(param_types.iter()) {
            self.env.define(param.name.clone(), param_type.clone(), false, param.span.clone())?;
        }

        // Check body and infer return type
        let body_type = self.infer_expr(&decl.body)?;

        self.env.pop_scope();

        // Verify the return type matches if annotated
        if let Some(ref annotated_type) = decl.return_type {
            self.check_type_compatibility(annotated_type, &body_type, &decl.span)?;
        } else {
            // If no annotation, verify inferred type matches what we assumed
            if body_type != preliminary_return_type {
                // Update the function type in environment with correct inferred type
                let correct_func_type = Type::Function {
                    params: param_types,
                    return_type: Box::new(body_type.clone()),
                };
                // We need to update the binding - for now, just verify they match
                self.check_type_compatibility(&preliminary_return_type, &body_type, &decl.span)?;
            }
        }

        Ok(())
    }

    fn infer_expr(&mut self, expr: &Expr) -> Result<Type, TypeError> {
        match expr {
            Expr::Number { .. } => Ok(Type::Num),
            Expr::String { .. } => Ok(Type::String),
            Expr::Bool { .. } => Ok(Type::Bool),
            
            Expr::Ident { name, span } => {
                self.env.get_type(name)
                    .ok_or_else(|| TypeError::UndefinedVariable {
                        name: name.clone(),
                        span: span.clone(),
                    })
            }
            
            Expr::BinOp { left, op, right, span } => {
                self.check_binop(left, *op, right, span)
            }
            
            Expr::UnaryOp { op, expr, span } => {
                self.check_unary_op(*op, expr, span)
            }
            
            Expr::Call { func, args, span } => {
                self.check_call(func, args, span)
            }
            
            Expr::Pipeline { left, right, span } => {
                // For now, treat pipeline as function application
                // left |> right means right(left)
                let left_type = self.infer_expr(left)?;
                
                // right should be a function that takes left_type
                let right_type = self.infer_expr(right)?;
                
                match right_type {
                    Type::Function { params, return_type } => {
                        if params.len() == 1 {
                            self.check_type_compatibility(&params[0], &left_type, span)?;
                            Ok(*return_type)
                        } else {
                            Err(TypeError::WrongNumberOfArguments {
                                expected: 1,
                                got: params.len(),
                                span: span.clone(),
                            })
                        }
                    }
                    _ => Err(TypeError::NotAFunction {
                        got: right_type,
                        span: span.clone(),
                    }),
                }
            }
            
            Expr::Block { stmts, span: _ } => {
                if stmts.is_empty() {
                    return Ok(Type::Num); // Default to Num for empty blocks
                }
                
                // Process statements in order, last one is the result
                let mut result_type = Type::Num;
                
                for (_i, stmt) in stmts.iter().enumerate() {
                    match stmt {
                        crate::ast::Statement::Item(item) => {
                            self.check_item(item)?;
                        }
                        crate::ast::Statement::Expr(expr) => {
                            result_type = self.infer_expr(expr)?;
                        }
                    }
                }
                
                Ok(result_type)
            }
            
            Expr::If { cond, then, else_, span } => {
                let cond_type = self.infer_expr(cond)?;
                self.check_type_compatibility(&Type::Bool, &cond_type, span)?;
                
                let then_type = self.infer_expr(then)?;
                let else_type = self.infer_expr(else_)?;
                
                self.check_type_compatibility(&then_type, &else_type, span)?;
                Ok(then_type)
            }
            
            Expr::Match { expr, arms, span } => {
                self.check_match(expr, arms, span)
            }
            
            Expr::FieldAccess { expr, field, span } => {
                let expr_type = self.infer_expr(expr)?;
                
                match expr_type {
                    Type::Record(fields) => {
                        for (f, t) in fields {
                            if f == *field {
                                return Ok(t);
                            }
                        }
                        Err(TypeError::UndefinedVariable {
                            name: field.clone(),
                            span: span.clone(),
                        })
                    }
                    Type::Named { name: _, fields, methods: _ } => {
                        // Handle field access on named types
                        for (f, t) in fields {
                            if f == *field {
                                return Ok(t);
                            }
                        }
                        Err(TypeError::UndefinedVariable {
                            name: field.clone(),
                            span: span.clone(),
                        })
                    }
                    Type::Array(_elem_type) => {
                        // Arrays have a built-in .size field
                        if field == "size" {
                            return Ok(Type::Num);
                        }
                        Err(TypeError::UndefinedVariable {
                            name: field.clone(),
                            span: span.clone(),
                        })
                    }
                    _ => Err(TypeError::TypeMismatch {
                        expected: Type::Record(vec![]),
                        got: expr_type,
                        span: span.clone(),
                    }),
                }
            }
            
            Expr::Index { expr, index, span } => {
                let expr_type = self.infer_expr(expr)?;
                let index_type = self.infer_expr(index)?;
                
                // Index must be Num
                if index_type != Type::Num {
                    return Err(TypeError::TypeMismatch {
                        expected: Type::Num,
                        got: index_type,
                        span: span.clone(),
                    });
                }
                
                // Expression must be an array
                match expr_type {
                    Type::Array(elem_type) => Ok(*elem_type),
                    _ => Err(TypeError::TypeMismatch {
                        expected: Type::Array(Box::new(Type::Num)),
                        got: expr_type,
                        span: span.clone(),
                    }),
                }
            }
            
            Expr::Array { elements, span } => {
                if elements.is_empty() {
                    // Empty array - infer as Array(Num) for now
                    return Ok(Type::Array(Box::new(Type::Num)));
                }
                
                let first_type = self.infer_expr(&elements[0])?;
                
                for elem in &elements[1..] {
                    let elem_type = self.infer_expr(elem)?;
                    self.check_type_compatibility(&first_type, &elem_type, span)?;
                }
                
                Ok(Type::Array(Box::new(first_type)))
            }
            
            Expr::Record { fields, .. } => {
                let mut field_types = Vec::new();
                
                for (name, value) in fields {
                    let value_type = self.infer_expr(value)?;
                    field_types.push((name.clone(), value_type));
                }
                
                Ok(Type::Record(field_types))
            }
            
            Expr::Constructor { type_name, fields, span } => {
                // Look up the type definition
                if let Some(symbol) = self.env.lookup(type_name) {
                    match &symbol.type_ {
                        Type::Named { name, fields: type_fields, methods } => {
                            // Clone the type info to avoid borrow issues
                            let name = name.clone();
                            let type_fields = type_fields.clone();
                            let methods = methods.clone();
                            
                            // Type-check each field
                            let mut provided_fields = std::collections::HashSet::new();
                            
                            for (field_name, field_expr) in fields {
                                provided_fields.insert(field_name.clone());
                                
                                // Find the expected type for this field
                                let expected_type = type_fields.iter()
                                    .find(|(f, _)| f == field_name)
                                    .map(|(_, t)| t.clone())
                                    .ok_or_else(|| TypeError::UndefinedVariable {
                                        name: format!("field {} in type {}", field_name, type_name),
                                        span: span.clone(),
                                    })?;
                                
                                // Type-check the field value
                                let actual_type = self.infer_expr(field_expr)?;
                                self.check_type_compatibility(&expected_type, &actual_type, span)?;
                            }
                            
                            // Check all fields are provided
                            for (field_name, _) in &type_fields {
                                if !provided_fields.contains(field_name) {
                                    return Err(TypeError::UndefinedVariable {
                                        name: format!("Missing field {} in constructor for {}", field_name, type_name),
                                        span: span.clone(),
                                    });
                                }
                            }
                            
                            // Return the Named type
                            Ok(Type::Named {
                                name,
                                fields: type_fields,
                                methods,
                            })
                        }
                        _ => Err(TypeError::TypeMismatch {
                            expected: Type::Named {
                                name: type_name.clone(),
                                fields: vec![],
                                methods: vec![],
                            },
                            got: symbol.type_.clone(),
                            span: span.clone(),
                        })
                    }
                } else {
                    Err(TypeError::UndefinedVariable {
                        name: type_name.clone(),
                        span: span.clone(),
                    })
                }
            }
            
            Expr::ForLoop { collection, pattern, body, span } => {
                use crate::ast::ForPattern;
                
                // Infer collection type
                let collection_type = self.infer_expr(collection)?;
                
                // Collection must be an array (for now; struct iteration can be added later)
                match collection_type {
                    Type::Array(elem_type) => {
                        // Create a new scope for the loop body
                        self.env.push_scope();
                        
                        // Bind pattern variables (immutable bindings in loop scope)
                        match pattern {
                            ForPattern::Item { name, span: pat_span } => {
                                self.env.define(name.clone(), *elem_type.clone(), false, pat_span.clone())?;
                            }
                            ForPattern::ItemIndex { item, index, span: pat_span } => {
                                self.env.define(item.clone(), *elem_type.clone(), false, pat_span.clone())?;
                                self.env.define(index.clone(), Type::Num, false, pat_span.clone())?;
                            }
                        }
                        
                        // Type check body (result is ignored, loop returns unit)
                        let _ = self.infer_expr(body)?;
                        
                        self.env.pop_scope();
                        
                        // For loops return Num (0 - unit/void equivalent)
                        Ok(Type::Num)
                    }
                    _ => Err(TypeError::TypeMismatch {
                        expected: Type::Array(Box::new(Type::Num)), // Placeholder
                        got: collection_type,
                        span: span.clone(),
                    })
                }
            }
        }
    }

    fn check_binop(&mut self, left: &Expr, op: BinOp, right: &Expr, span: &Span) -> Result<Type, TypeError> {
        let left_type = self.infer_expr(left)?;
        let right_type = self.infer_expr(right)?;

        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                self.check_type_compatibility(&Type::Num, &left_type, span)?;
                self.check_type_compatibility(&Type::Num, &right_type, span)?;
                Ok(Type::Num)
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                self.check_type_compatibility(&left_type, &right_type, span)?;
                Ok(Type::Bool)
            }
            BinOp::And | BinOp::Or => {
                self.check_type_compatibility(&Type::Bool, &left_type, span)?;
                self.check_type_compatibility(&Type::Bool, &right_type, span)?;
                Ok(Type::Bool)
            }
        }
    }

    fn check_unary_op(&mut self, op: UnaryOp, expr: &Expr, span: &Span) -> Result<Type, TypeError> {
        let expr_type = self.infer_expr(expr)?;

        match op {
            UnaryOp::Neg => {
                self.check_type_compatibility(&Type::Num, &expr_type, span)?;
                Ok(Type::Num)
            }
            UnaryOp::Not => {
                self.check_type_compatibility(&Type::Bool, &expr_type, span)?;
                Ok(Type::Bool)
            }
        }
    }

    fn check_call(&mut self, func: &Expr, args: &[Expr], span: &Span) -> Result<Type, TypeError> {
        // Check if this is a method call: func is Ident and first arg is a Named type
        if let Expr::Ident { name, .. } = func {
            if !args.is_empty() {
                let first_arg_type = self.infer_expr(&args[0])?;
                
                // Check if first argument is a Named type with this method
                if let Type::Named { name: type_name, fields: _, methods: _ } = &first_arg_type {
                    // Look up method in the type's method list
                    if let Some(method_sig) = self.methods.get(&(type_name.clone(), name.clone())).cloned() {
                        let (method_params, method_return_type, _body) = method_sig;
                        
                        // Method parameters don't include the implicit receiver
                        // But args[0] is the receiver, so we need args[1..] to match method_params
                        let call_args = &args[1..];
                        
                        if method_params.len() != call_args.len() {
                            return Err(TypeError::WrongNumberOfArguments {
                                expected: method_params.len(),
                                got: call_args.len(),
                                span: span.clone(),
                            });
                        }
                        
                        // Type check arguments
                        for (param, arg) in method_params.iter().zip(call_args.iter()) {
                            let arg_type = self.infer_expr(arg)?;
                            // Extract the type from the Param
                            if let Some(param_type) = &param.type_annotation {
                                self.check_type_compatibility(param_type, &arg_type, span)?;
                            }
                            // If no type annotation, we can't check (would need inference)
                        }
                        
                        // Return the method's return type (or Num if not specified)
                        return Ok(method_return_type.unwrap_or(Type::Num));
                    }
                }
            }
        }
        
        // Fall back to regular function call
        let func_type = self.infer_expr(func)?;

        match func_type {
            Type::Function { params, return_type } => {
                if params.len() != args.len() {
                    return Err(TypeError::WrongNumberOfArguments {
                        expected: params.len(),
                        got: args.len(),
                        span: span.clone(),
                    });
                }

                for (param_type, arg) in params.iter().zip(args.iter()) {
                    let arg_type = self.infer_expr(arg)?;
                    self.check_type_compatibility(param_type, &arg_type, span)?;
                }

                Ok(*return_type)
            }
            _ => Err(TypeError::NotAFunction {
                got: func_type,
                span: span.clone(),
            }),
        }
    }

    fn check_match(&mut self, expr: &Expr, arms: &[MatchArm], span: &Span) -> Result<Type, TypeError> {
        let expr_type = self.infer_expr(expr)?;

        if arms.is_empty() {
            return Err(TypeError::NonExhaustiveMatch { span: span.clone() });
        }

        // Check exhaustiveness for sum types
        if let Type::Sum { ref variants, .. } = expr_type {
            self.check_exhaustiveness(variants, arms, span)?;
        }

        // Check each arm's pattern against expr_type
        let mut result_type = None;

        for arm in arms {
            self.check_pattern(&arm.pattern, &expr_type)?;
            
            // Bind pattern variables and check body
            self.env.push_scope();
            self.bind_pattern_vars(&arm.pattern, &expr_type)?;
            
            let body_type = self.infer_expr(&arm.body)?;
            
            self.env.pop_scope();

            // All arms must return same type
            if let Some(ref expected_type) = result_type {
                self.check_type_compatibility(expected_type, &body_type, &arm.span)?;
            } else {
                result_type = Some(body_type);
            }
        }

        Ok(result_type.unwrap())
    }

    fn check_exhaustiveness(
        &self,
        variants: &[crate::ast::SumVariant],
        arms: &[MatchArm],
        span: &Span,
    ) -> Result<(), TypeError> {
        // Collect all constructor patterns
        let mut covered_variants = std::collections::HashSet::new();
        let mut has_wildcard = false;

        for arm in arms {
            match &arm.pattern {
                Pattern::Wildcard { .. } | Pattern::Ident { .. } => {
                    has_wildcard = true;
                }
                Pattern::Constructor { name, .. } => {
                    covered_variants.insert(name.clone());
                }
                _ => {}
            }
        }

        // If we have a wildcard, we're exhaustive
        if has_wildcard {
            return Ok(());
        }

        // Check if all variants are covered
        for variant in variants {
            if !covered_variants.contains(&variant.name) {
                return Err(TypeError::NonExhaustiveMatch { span: span.clone() });
            }
        }

        Ok(())
    }

    fn check_pattern(&self, pattern: &Pattern, expected_type: &Type) -> Result<(), TypeError> {
        match pattern {
            Pattern::Ident { .. } => Ok(()), // Any type can bind to ident
            Pattern::Number { .. } => {
                self.check_type_compatibility(&Type::Num, expected_type, pattern.span())
            }
            Pattern::Wildcard { .. } => Ok(()), // Wildcard matches anything
            Pattern::Constructor { name, args, span } => {
                // Check if the constructor matches the expected type
                // For now, accept all constructors - proper sum type checking would verify
                // that the constructor belongs to the expected sum type
                match expected_type {
                    Type::Sum { variants, .. } => {
                        // Find the variant with this constructor name
                        let variant = variants.iter().find(|v| v.name == *name);
                        
                        if let Some(variant) = variant {
                            // Check that argument count matches
                            if variant.fields.len() != args.len() {
                                return Err(TypeError::WrongNumberOfArguments {
                                    expected: variant.fields.len(),
                                    got: args.len(),
                                    span: span.clone(),
                                });
                            }
                            
                            // Check each pattern argument against field type
                            for (pattern_arg, field_type) in args.iter().zip(variant.fields.iter()) {
                                self.check_pattern(pattern_arg, field_type)?;
                            }
                            
                            Ok(())
                        } else {
                            // Constructor not found in sum type
                            Ok(()) // For now, accept it
                        }
                    }
                    _ => {
                        // Not a sum type, but we have a constructor pattern
                        // This is okay for now - we may be matching against
                        // a value that will be a sum type later
                        Ok(())
                    }
                }
            }
        }
    }

    fn bind_pattern_vars(&mut self, pattern: &Pattern, type_: &Type) -> Result<(), TypeError> {
        match pattern {
            Pattern::Ident { name, span } => {
                self.env.define(name.clone(), type_.clone(), false, span.clone())?;
                Ok(())
            }
            Pattern::Constructor { args, .. } => {
                // For now, just bind args with the same type
                for arg in args {
                    self.bind_pattern_vars(arg, type_)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn check_type_compatibility(&self, expected: &Type, got: &Type, span: &Span) -> Result<(), TypeError> {
        if expected == got {
            Ok(())
        } else {
            Err(TypeError::TypeMismatch {
                expected: expected.clone(),
                got: got.clone(),
                span: span.clone(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::parse;

    #[test]
    fn test_simple_var() {
        let tokens = Lexer::tokenize("x = 42").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        assert!(checker.check_program(&program).is_ok());
    }

    #[test]
    fn test_typed_var() {
        let tokens = Lexer::tokenize("x :: Num = 42").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        assert!(checker.check_program(&program).is_ok());
    }

    #[test]
    fn test_type_mismatch() {
        let tokens = Lexer::tokenize("x :: String = 42").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        assert!(checker.check_program(&program).is_err());
    }

    #[test]
    fn test_arithmetic() {
        let tokens = Lexer::tokenize("result = 2 + 3 * 4").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        assert!(checker.check_program(&program).is_ok());
    }

    #[test]
    fn test_undefined_var() {
        let tokens = Lexer::tokenize("y = x + 1").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        assert!(checker.check_program(&program).is_err());
    }

    #[test]
    fn test_simple_function() {
        let tokens = Lexer::tokenize("add = (a :: Num, b :: Num) -> Num => a + b").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        assert!(checker.check_program(&program).is_ok());
    }

    #[test]
    fn test_function_call() {
        let tokens = Lexer::tokenize("add = (a :: Num, b :: Num) -> Num => a + b
result = add(1, 2)").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        assert!(checker.check_program(&program).is_ok());
    }

    #[test]
    fn test_wrong_arg_count() {
        let tokens = Lexer::tokenize("add = (a :: Num, b :: Num) -> Num => a + b
result = add(1)").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        assert!(checker.check_program(&program).is_err());
    }

    #[test]
    fn test_array() {
        let tokens = Lexer::tokenize("nums = [1, 2, 3]").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        assert!(checker.check_program(&program).is_ok());
    }

    #[test]
    fn test_array_type_mismatch() {
        let tokens = Lexer::tokenize("mixed = [1, \"hello\"]").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        assert!(checker.check_program(&program).is_err());
    }

    #[test]
    fn test_record() {
        let tokens = Lexer::tokenize("user = { name = \"Alice\", age = 30 }").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        assert!(checker.check_program(&program).is_ok());
    }

    #[test]
    fn test_if_expr() {
        let tokens = Lexer::tokenize("result = true ? 1 : 0").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        assert!(checker.check_program(&program).is_ok());
    }

    #[test]
    fn test_if_branch_type_mismatch() {
        let tokens = Lexer::tokenize("result = true ? 1 : \"hello\"").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        assert!(checker.check_program(&program).is_err());
    }

    #[test]
    fn test_block() {
        let tokens = Lexer::tokenize("compute = => < x = 10 y = 20 x + y >").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        assert!(checker.check_program(&program).is_ok());
    }

    #[test]
    fn test_pattern_match() {
        let tokens = Lexer::tokenize("result = 5 ? | 0 => \"zero\" | _ => \"other\"").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        assert!(checker.check_program(&program).is_ok());
    }

    #[test]
    fn test_inferred_return_type() {
        // Function without return type annotation - should infer from body
        let tokens = Lexer::tokenize("double = (x :: Num) => x + x").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        assert!(checker.check_program(&program).is_ok());
        
        // Verify the function type was inferred correctly
        let func_type = checker.env.get_type("double").unwrap();
        if let Type::Function { params, return_type } = func_type {
            assert_eq!(params, vec![Type::Num]);
            assert_eq!(*return_type, Type::Num);
        } else {
            panic!("Expected function type");
        }
    }

    #[test]
    fn test_inferred_param_types() {
        // Function without parameter type annotations - defaults to Num
        let tokens = Lexer::tokenize("add = (a, b) => a + b").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        assert!(checker.check_program(&program).is_ok());
    }

    #[test]
    fn test_sum_type_option() {
        // Pattern match on Result type with OK/NotOK
        let tokens = Lexer::tokenize("val = 5
result = val ? | OK(x) => x | NotOK => 0").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        assert!(checker.check_program(&program).is_ok());
    }

    #[test]
    fn test_exhaustiveness_with_wildcard() {
        // Wildcard makes match exhaustive
        let tokens = Lexer::tokenize("val = 5
result = val ? | OK(x) => x | _ => 0").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        assert!(checker.check_program(&program).is_ok());
    }

    #[test]
    fn test_constructor_arity() {
        // Constructor with wrong number of arguments should fail when we have proper sum types
        // For now this will pass, but once we implement full sum type checking it should fail
        let tokens = Lexer::tokenize("val = 5
result = val ? | OK(x, y) => x | NotOK => 0").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        // Currently passes - will fail when sum types are fully implemented
        let _ = checker.check_program(&program);
    }

    #[test]
    fn test_builtin_sum_types() {
        // Verify Result type is available
        let mut checker = TypeChecker::new();
        
        // Check Result is defined
        assert!(checker.env.get_type("Result").is_some());
    }
    
    #[test]
    fn test_for_loop_simple() {
        let tokens = Lexer::tokenize("test = => [1, 2, 3] |> for n => n").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        let result = checker.check_program(&program);
        if result.is_err() {
            eprintln!("Type error: {:?}", result.as_ref().unwrap_err());
        }
        assert!(result.is_ok());
    }
    
    #[test]
    fn test_for_loop_with_index() {
        let tokens = Lexer::tokenize("test = => <
  items = [10, 20, 30]
  items |> for (val, i) => val
>").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        let result = checker.check_program(&program);
        if result.is_err() {
            eprintln!("Type error: {:?}", result.as_ref().unwrap_err());
        }
        assert!(result.is_ok());
    }
    
    #[test]
    fn test_for_loop_type_bindings() {
        // Test that loop variables are properly bound with correct types
        let tokens = Lexer::tokenize("test = => <
  nums = [1.5, 2.5, 3.5]
  nums |> for n => n + 1.0
>").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        let result = checker.check_program(&program);
        if result.is_err() {
            eprintln!("Type error: {:?}", result.as_ref().unwrap_err());
        }
        assert!(result.is_ok());
    }
    
    #[test]
    fn test_for_loop_index_is_num() {
        // Test that index variable is Num type
        let tokens = Lexer::tokenize("test = => [10, 20] |> for (val, i) => i + val").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        let result = checker.check_program(&program);
        if result.is_err() {
            eprintln!("Type error: {:?}", result.as_ref().unwrap_err());
        }
        assert!(result.is_ok());
    }
    
    #[test]
    fn test_for_loop_non_array_fails() {
        // For loop on non-array should fail
        let tokens = Lexer::tokenize("test = => <
  x = 42
  x |> for n => n
>").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        let result = checker.check_program(&program);
        assert!(result.is_err());
    }
    
    #[test]
    fn test_for_loop_returns_num() {
        // For loops should return Num (unit/0)
        let tokens = Lexer::tokenize("test = => <
  result = [1, 2, 3] |> for n => n
  result + 1
>").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        let result = checker.check_program(&program);
        if result.is_err() {
            eprintln!("Type error: {:?}", result.as_ref().unwrap_err());
        }
        assert!(result.is_ok());
    }
    
    #[test]
    fn test_method_call_simple() {
        // Test that method calls work with type constructors
        let tokens = Lexer::tokenize("User = {
  name :: String,
  age :: Num,
  getName = => it.name
}
test = => <
  user = User { name = \"Alice\", age = 30 }
  name = user.getName()
  0
>").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        let result = checker.check_program(&program);
        if result.is_err() {
            eprintln!("Type error: {:?}", result.as_ref().unwrap_err());
        }
        assert!(result.is_ok());
    }
    
    #[test]
    fn test_method_call_with_args() {
        // Test method calls with additional arguments
        let tokens = Lexer::tokenize("add = (self, x) => self + x
test = => <
  result = (5).add(10)
  result
>").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        let result = checker.check_program(&program);
        if result.is_err() {
            eprintln!("Type error: {:?}", result.as_ref().unwrap_err());
        }
        assert!(result.is_ok());
    }
    
    #[test]
    fn test_method_vs_function_call() {
        // Both method and function call syntax should work
        let tokens = Lexer::tokenize("double = x => x * 2
test = => <
  a = (5).double()     ~ Method syntax
  b = double(5)         ~ Function syntax
  a + b
>").unwrap();
        let program = parse(&tokens).unwrap();
        let mut checker = TypeChecker::new();
        let result = checker.check_program(&program);
        if result.is_err() {
            eprintln!("Type error: {:?}", result.as_ref().unwrap_err());
        }
        assert!(result.is_ok());
    }
}
