//! Checking calls: plain functions, overload members, methods, and the built-in array
//! and `Text` methods with their lambda arguments.
//!
//! Part of the type checker; see `super` for the `TypeChecker` state these methods
//! run against.

use super::*;
use crate::ast::Statement;

impl TypeChecker {
    /// Whether `name` names something callable here — a binding in scope, an overload set,
    /// or one of the forms the compiler provides. Asked only to decide whether an
    /// unresolved member call has a same-named function worth pointing the reader at, so a
    /// typo (or a sibling method declared below) is not sent after one that isn't there.
    fn names_a_callable(&self, name: &str) -> bool {
        self.env.lookup(name).is_some()
            || self.overloads.contains_key(name)
            || crate::ast::is_compiler_provided_name(name)
            || crate::ast::is_assertion(name)
    }

    /// Whether `ty` answers `name` through the `.` form — a method the record or sum
    /// declares, or a built-in reserved on `Text`/an array/a `Map`/a `Set`.
    fn type_has_member(&self, ty: &Type, name: &str) -> bool {
        match ty {
            Type::Named {
                name: type_name, ..
            }
            | Type::Sum {
                name: type_name, ..
            } => self
                .methods
                .contains_key(&(type_name.clone(), name.to_string())),
            Type::Array(_) => crate::ast::is_array_method(name),
            Type::Text => crate::ast::is_text_method(name),
            Type::Map(_, _) => crate::ast::is_map_method(name),
            Type::Set(_) => crate::ast::is_set_method(name),
            _ => false,
        }
    }

    /// Check a call to an output built-in (`print`/`eprint`/`write`): the first argument is
    /// rendered through its `` ` `` member, so any type but a function is accepted there,
    /// and the rest are checked against the built-in's fixed parameter types. Reached only
    /// for a call the built-in claims (see the guard in `check_call`), so a wrong argument
    /// count here is an error rather than a fall-through. Kept out of `check_call`'s frame.
    pub(super) fn check_renderable_builtin_call(
        &mut self,
        name: &str,
        builtin: &crate::ast::RenderableBuiltin,
        arguments: &[Expression],
        first_ty: &Option<Type>,
        span: &Span,
    ) -> Result<Type, TypeError> {
        if arguments.len() != builtin.arity() {
            return Err(TypeError::WrongNumberOfArguments {
                expected: builtin.arity(),
                got: arguments.len(),
                span: span.clone(),
            });
        }
        // The built-in's arity is always at least one, so the call has a first argument.
        // It is usually already inferred (`first_ty`) — a LAMBDA there is not, since the
        // dispatcher leaves a lambda's type to the signature the call resolves to, and this
        // position states none. Typing it here is what lets the rendering rule name it.
        let rendered = match first_ty {
            Some(ty) => ty.clone(),
            None => self.infer_argument(&arguments[0], LambdaTarget::None)?,
        };
        if !crate::ast::is_renderable(&rendered) {
            return Err(TypeError::NotRenderable {
                name: name.to_string(),
                got: Box::new(rendered),
                span: span.clone(),
            });
        }
        for (parameter, argument) in builtin.rest.iter().zip(&arguments[1..]) {
            let argument_type = self.infer_expression(argument)?;
            self.check_type_compatibility(parameter, &argument_type, span)?;
        }
        Ok(builtin.ret.clone())
    }

    /// Infer an argument's type, handing a lambda argument the type its position states.
    /// This is **contextual typing**: `apply(10, (n) => n + 1)` types `n` from `apply`'s
    /// own `(Num) -> Num` parameter, so a higher-order call states the parameter types
    /// once — at the definition that receives them. Anything but a lambda is inferred on
    /// its own, `target` unused.
    pub(super) fn infer_argument(
        &mut self,
        argument: &Expression,
        target: LambdaTarget<'_>,
    ) -> Result<Type, TypeError> {
        // A block's value is its tail expression, so a type stated FOR the block is stated
        // for that tail. This is what carries a function's declared return through its body
        // — every body is a block, so without it a returned lambda
        // (`adder = (n :: Num) -> (Num) -> Num => < (x) => x + n >`) would have nothing to
        // take `x` from.
        if let Expression::Block { statements, .. } = argument
            && let Some((Statement::Expression(tail), leading)) = statements.split_last()
        {
            for statement in leading {
                match statement {
                    Statement::Item(item) => self.check_item(item, Nesting::Nested)?,
                    Statement::Expression(expression) => {
                        self.infer_expression(expression)?;
                    }
                }
            }
            return self.infer_argument(tail, target);
        }
        let Expression::Lambda {
            parameters,
            return_type,
            body,
            span,
        } = argument
        else {
            return self.infer_expression(argument);
        };
        let ty = self.check_lambda_against(parameters, return_type.as_ref(), body, target)?;
        self.type_table.insert(span.clone(), ty.clone());
        Ok(ty)
    }

    /// Type-check a call. `member_call` marks the `recv.name(args)` form (see
    /// [`Expression::Call`]), and the two forms name two namespaces that never answer for
    /// each other: a member call is looked for on the receiver's type alone (a name it
    /// does not have is [`TypeError::UnknownMember`], never a fall-through to a function),
    /// and the plain form `name(recv, args)` is looked for in the top-level namespace
    /// alone — every receiver dispatch below is skipped for one, method or built-in.
    pub(super) fn check_call(
        &mut self,
        function: &Expression,
        arguments: &[Expression],
        member_call: bool,
        span: &Span,
    ) -> Result<Type, TypeError> {
        // The provided assertions, which take a matcher rather than ordinary arguments —
        // resolved here, ahead of every other dispatch, since they are the compiler's own.
        if let Expression::Identifier { name, .. } = function
            && !member_call
            && crate::ast::is_assertion(name)
        {
            return self.check_assertion(name, arguments, span);
        }

        // Check if this is a sum type constructor call: Ok(42), Circle(r), etc.
        if let Expression::Identifier {
            name: constructor_name,
            ..
        } = function
            && !member_call
            && let Some(sum_type) =
                self.check_constructor_call(constructor_name, arguments, span)?
        {
            return Ok(sum_type);
        }

        // Infer the FIRST argument exactly once and reuse the result in every probe
        // and branch below. The receiver probes (array/Text/method dispatch) and the
        // overload/fallback argument loops all need `arguments[0]`'s type; inferring it in
        // each place made every nesting level infer its subtree twice — 2^depth work,
        // which visibly hung the checker on ~25-deep call chains.
        // A LAMBDA first argument is left out: its type may depend on the signature this
        // call resolves to, which is only known further down. It is no loss — a lambda is
        // never a receiver, so none of the probes below can want it.
        let first_ty = match (function, arguments.first()) {
            (Expression::Identifier { .. }, Some(first))
                if !matches!(first, Expression::Lambda { .. }) =>
            {
                Some(self.infer_expression(first)?)
            }
            _ => None,
        };

        // Built-in array methods (`map`/`filter`/`reduce`/`each`/`find`/`at`) take
        // precedence over any user overload of the same name: when the receiver
        // (`arguments[0]`) is an array, the method is RESERVED and resolved here, before
        // overload dispatch. (A user can still define e.g. `map` on a non-array type;
        // dispatch only diverts to the built-in when the receiver is an array.)
        if member_call
            && let Expression::Identifier { name, .. } = function
            && crate::ast::is_array_method(name)
            && let Some(Type::Array(elem_type)) = first_ty.clone()
        {
            return self.check_array_method(name, *elem_type, arguments, span);
        }

        // Built-in `Text` methods (`split`/`trim`/`replace`/`contains`/`indexOf`/
        // `slice`/`toUpper`/`toLower`) — RESERVED on `Text`, exactly like the array
        // methods above: when the receiver (`arguments[0]`) is a `Text`, the built-in is
        // resolved here ahead of any same-named user overload. (A user may still define
        // e.g. `trim` on a non-Text type; dispatch only diverts on a Text receiver.)
        if member_call
            && let Expression::Identifier { name, .. } = function
            && crate::ast::is_text_method(name)
            && matches!(first_ty, Some(Type::Text))
        {
            return self.check_text_method(name, arguments, span);
        }

        // Built-in `Map` methods (`get`/`has`/`set`/`keys`/`values`/`each`) — RESERVED on
        // a `Map` receiver, exactly like the array/Text methods above.
        if member_call
            && let Expression::Identifier { name, .. } = function
            && crate::ast::is_map_method(name)
            && let Some(Type::Map(key_type, value_type)) = first_ty.clone()
        {
            return self.check_map_method(name, *key_type, *value_type, arguments, span);
        }

        // Built-in `Set` methods (`has`/`add`/`items`/`each`) — RESERVED on a `Set`
        // receiver.
        if member_call
            && let Expression::Identifier { name, .. } = function
            && crate::ast::is_set_method(name)
            && let Some(Type::Set(elem_type)) = first_ty.clone()
        {
            return self.check_set_method(name, *elem_type, arguments, span);
        }

        // `name(recv, …)` naming a member of the receiver's type with no function of that
        // name in scope: the plain form looks only in the top-level namespace, so point at
        // the `.` call rather than reporting the name as merely undefined.
        if !member_call
            && let Expression::Identifier { name, .. } = function
            && let Some(receiver_type) = &first_ty
            && !self.names_a_callable(name)
            && !self.overloaded_names.contains(name)
            && self.type_has_member(receiver_type, name)
        {
            return Err(TypeError::MethodCalledAsFunction {
                type_name: crate::ast::type_label(receiver_type),
                member: name.clone(),
                receiver: match &arguments[0] {
                    Expression::Identifier { name, .. } => Some(name.clone()),
                    _ => None,
                },
                more_arguments: arguments.len() > 1,
                span: span.clone(),
            });
        }

        // A method the receiver's type declares. Only the `.` form asks: `name(recv, …)`
        // names the top-level namespace, and a method is not in it.
        if member_call
            && let Expression::Identifier { name, .. } = function
            && let Some(first_arg_type) = &first_ty
        {
            // A record or a sum both carry methods, identified by their type name.
            if let Type::Named {
                name: type_name, ..
            }
            | Type::Sum {
                name: type_name, ..
            } = first_arg_type
            {
                // Look up method in the type's method list. Only the signature is taken —
                // cloning the whole entry would deep-copy the method's body at every call.
                if let Some((method_parameters, method_return_type)) = self
                    .methods
                    .get(&(type_name.clone(), name.clone()))
                    .map(|(parameters, return_type, _body)| {
                        (parameters.clone(), return_type.clone())
                    })
                {
                    // A mutating (setter) method requires a mutable (`:=`) receiver.
                    // The receiver is arguments[0]; `it` (a method calling a sibling
                    // setter on its own receiver) is allowed — its mutability is
                    // already enforced at the *outer* call site.
                    if self
                        .setter_methods
                        .contains(&(type_name.clone(), name.clone()))
                        && let Some(recv_name) = self.immutable_mutation_root(&arguments[0])
                    {
                        return Err(TypeError::MutatingMethodOnImmutable {
                            method: name.clone(),
                            receiver: recv_name,
                            span: span.clone(),
                        });
                    }

                    // Method parameters don't include the implicit receiver
                    // But arguments[0] is the receiver, so we need arguments[1..] to match method_parameters
                    let call_args = &arguments[1..];

                    if method_parameters.len() != call_args.len() {
                        return Err(TypeError::WrongNumberOfArguments {
                            expected: method_parameters.len(),
                            got: call_args.len(),
                            span: span.clone(),
                        });
                    }

                    // An unannotated parameter is not unchecked: the body was checked with it
                    // defaulted to `Num`, so the call has to meet that same commitment —
                    // exactly as a plain function's unannotated parameter already does.
                    // Skipping these let a `Text` argument reach codegen and surface as a raw
                    // LLVM verifier dump.
                    //
                    // The annotation is deliberately NOT resolved here, unlike at the
                    // definition site: a user-typed parameter is broken end to end today, and
                    // resolving it only moves the failure from the checker into codegen, which
                    // has no field types for a method parameter.
                    for (parameter, arg) in method_parameters.iter().zip(call_args.iter()) {
                        let parameter_type = parameter.type_annotation.clone().unwrap_or(Type::Num);
                        let arg_type =
                            self.infer_argument(arg, LambdaTarget::Declared(&parameter_type))?;
                        self.check_type_compatibility(&parameter_type, &arg_type, span)?;
                    }

                    // Return the method's return type (or Num if not specified)
                    return Ok(method_return_type.unwrap_or(Type::Num));
                }
            }
        }

        // Everything below resolves the name in the TOP-LEVEL namespace — a user function,
        // an overload set, one of the compiler's own output built-ins — which a member call
        // never reaches: `recv.name(...)` asks the receiver's type for `name`, and a
        // top-level name that happens to match is unrelated to it. So a member call ALWAYS
        // stops here, never conditionally: a shape that slipped past the destructure would
        // fall through and be hijacked after all. The parser builds one only as
        // `name(recv, …)`, so both halves always bind.
        if member_call {
            let (Expression::Identifier { name, .. }, Some(receiver)) =
                (function, arguments.first())
            else {
                return Err(TypeError::NotAFunction {
                    got: self.infer_expression(function)?,
                    span: span.clone(),
                });
            };
            // The receiver is usually already inferred (`first_ty`) — a LAMBDA there is
            // not, since the dispatcher leaves a lambda's type to the signature the call
            // resolves to, and a member call resolves to none. Type it now: the member is
            // unknown either way, and the report has to name what it was looked for on.
            let receiver_type = match &first_ty {
                Some(ty) => ty.clone(),
                None => self.infer_argument(receiver, LambdaTarget::None)?,
            };
            return Err(TypeError::UnknownMember {
                type_name: crate::ast::type_label(&receiver_type),
                member: name.clone(),
                in_scope: self.names_a_callable(name),
                // The advice spells out the plain call to write instead, so it needs the
                // receiver as the reader wrote it — available when that is a plain name.
                receiver: match receiver {
                    Expression::Identifier { name, .. } => Some(name.clone()),
                    _ => None,
                },
                more_arguments: arguments.len() > 1,
                span: span.clone(),
            });
        }

        // `print`/`eprint`/`write` render their first argument through its `` ` `` operator,
        // so a value of any type is accepted there. The built-in claims every call at its own
        // arity — codegen asks the same question — and another arity belongs to a user set of
        // the same name, where there is one. (The check itself is a separate method so its
        // locals stay out of this hot, deeply-recursive frame.)
        if let Expression::Identifier { name, .. } = function
            && let Some(builtin) = crate::ast::renderable_builtin(name)
            && (arguments.len() == builtin.arity() || !self.overloaded_names.contains(name))
        {
            return self.check_renderable_builtin_call(name, builtin, arguments, &first_ty, span);
        }

        // A name that forms an overload set but has no member registered yet is one whose
        // every definition sits below this call. Say so, rather than falling through to
        // the plain-function path and reporting the name as undefined — it is defined,
        // just not yet.
        if let Expression::Identifier { name, .. } = function
            && self.overloaded_names.contains(name)
            && !self.overloads.contains_key(name)
        {
            return Err(TypeError::OverloadCallBeforeDefinition {
                name: name.clone(),
                span: span.clone(),
            });
        }

        // Overload-set dispatch: if `function` names an overload set (a user overload set
        // OR a built-in like `now`), resolve it by EXACT argument types.
        if let Expression::Identifier { name, .. } = function
            && self.overloads.contains_key(name)
        {
            return self.check_overloaded_call(name, arguments, first_ty.as_ref(), span);
        }

        // Fall back to regular function call
        let func_type = self.infer_expression(function)?;

        match func_type {
            Type::Function {
                parameters,
                return_type,
            } => {
                // A trailing `Site` parameter receives the CALLER's location, which the
                // compiler fills in (`CodeGenerator::site_value`), so a call may leave that
                // last argument off — and the arity a caller sees excludes it.
                if parameters.len() != arguments.len()
                    && !crate::ast::fills_call_site(&parameters, arguments.len())
                {
                    return Err(TypeError::WrongNumberOfArguments {
                        expected: crate::ast::visible_parameters(&parameters).len(),
                        got: arguments.len(),
                        span: span.clone(),
                    });
                }

                // Type the arguments once, then check against the resolved signature. The
                // signature is known here, so a lambda argument takes its parameter types
                // from the matching parameter rather than having to repeat them.
                for (i, (parameter_type, arg)) in
                    parameters.iter().zip(arguments.iter()).enumerate()
                {
                    let arg_type = match (i, &first_ty) {
                        (0, Some(ty)) => ty.clone(),
                        _ => self.infer_argument(arg, LambdaTarget::Declared(parameter_type))?,
                    };
                    self.check_type_compatibility(parameter_type, &arg_type, span)?;
                }
                Ok(*return_type)
            }
            _ => Err(TypeError::NotAFunction {
                got: func_type,
                span: span.clone(),
            }),
        }
    }

    /// Type-check a built-in array method call. `arguments[0]` is the receiver array (already
    /// known to be `Array(_)`); the remaining arguments are the method's own arguments,
    /// typically a lambda whose parameters bind to the element (or accumulator) type:
    ///   - `map(f: elem => R)`            -> `[]R`
    ///   - `filter(pred: elem => Bool)`   -> `[]elem`  (same element type)
    ///   - `reduce(init: A, f: (A, elem) => A)` -> `A`
    ///   - `each(f: elem => _)`           -> the receiver array itself (`[]elem`)
    ///   - `find(pred: elem => Bool)`     -> `Result` with `Ok(elem)` / `NotOk`
    ///   - `at(n: Num)`                   -> `Result` with `Ok(elem)` / `NotOk`
    pub(super) fn check_array_method(
        &mut self,
        method: &str,
        elem_type: Type,
        arguments: &[Expression],
        span: &Span,
    ) -> Result<Type, TypeError> {
        // `arguments[0]` (the receiver array) was already inferred by the dispatch guard in
        // `check_call`, which passes its element type in — no need to re-infer it here.
        let method_args = &arguments[1..];

        // Arity check for every method (the lambda/value argument count).
        let expected = match method {
            "reduce" => 2,
            _ => 1,
        };
        if method_args.len() != expected {
            return Err(TypeError::WrongNumberOfArguments {
                expected,
                got: method_args.len(),
                span: span.clone(),
            });
        }

        match method {
            "map" => {
                let ret = self.check_lambda_arg(&method_args[0], &[elem_type], span)?;
                Ok(Type::Array(Box::new(ret)))
            }
            "filter" => {
                let ret =
                    self.check_lambda_arg(&method_args[0], std::slice::from_ref(&elem_type), span)?;
                self.check_type_compatibility(&Type::Bool, &ret, span)?;
                Ok(Type::Array(Box::new(elem_type)))
            }
            "reduce" => {
                let init_type = self.infer_expression(&method_args[0])?;
                let ret =
                    self.check_lambda_arg(&method_args[1], &[init_type.clone(), elem_type], span)?;
                // The accumulator/result type is the init's type; the reducer must agree.
                self.check_type_compatibility(&init_type, &ret, span)?;
                Ok(init_type)
            }
            "each" => {
                // Side-effecting; result is ignored. Returns the receiver array (decision
                // 19: a Unit-bodied method yields `it`, here the array), so `.each` chains.
                self.check_lambda_arg(&method_args[0], std::slice::from_ref(&elem_type), span)?;
                Ok(Type::Array(Box::new(elem_type)))
            }
            "find" => {
                let ret =
                    self.check_lambda_arg(&method_args[0], std::slice::from_ref(&elem_type), span)?;
                self.check_type_compatibility(&Type::Bool, &ret, span)?;
                Ok(result_of(elem_type))
            }
            "at" => {
                let idx_type = self.infer_expression(&method_args[0])?;
                self.check_type_compatibility(&Type::Num, &idx_type, span)?;
                Ok(result_of(elem_type))
            }
            other => unreachable!("unhandled array method {other}"),
        }
    }

    /// Type-check a built-in `Map` method call. `arguments[0]` is the receiver map (already
    /// known to be `Map(K, V)`); the remaining arguments are the method's own arguments.
    ///   - `get(k :: K)`        -> `Result` (`Ok(V)` / `NotOk`) — the safe lookup
    ///   - `has(k :: K)`        -> `Bool`
    ///   - `set(k :: K, v :: V)`-> `Map(K, V)` (a NEW map; the receiver is unchanged)
    ///   - `remove(k :: K)`     -> `Map(K, V)` (a NEW map without `k`; the receiver is unchanged)
    ///   - `keys()`             -> `[]K`
    ///   - `values()`           -> `[]V`
    ///   - `each(f: (K, V) => _)` -> the receiver map (so `.each` chains)
    pub(super) fn check_map_method(
        &mut self,
        method: &str,
        key_type: Type,
        value_type: Type,
        arguments: &[Expression],
        span: &Span,
    ) -> Result<Type, TypeError> {
        let method_args = &arguments[1..];
        let map_type = Type::Map(Box::new(key_type.clone()), Box::new(value_type.clone()));

        let expected = match method {
            "keys" | "values" => 0,
            "set" => 2,
            _ => 1,
        };
        if method_args.len() != expected {
            return Err(TypeError::WrongNumberOfArguments {
                expected,
                got: method_args.len(),
                span: span.clone(),
            });
        }

        match method {
            "get" => {
                let k = self.infer_expression(&method_args[0])?;
                self.check_type_compatibility(&key_type, &k, span)?;
                Ok(result_of(value_type))
            }
            "has" => {
                let k = self.infer_expression(&method_args[0])?;
                self.check_type_compatibility(&key_type, &k, span)?;
                Ok(Type::Bool)
            }
            "set" => {
                let k = self.infer_expression(&method_args[0])?;
                self.check_type_compatibility(&key_type, &k, span)?;
                let v = self.infer_expression(&method_args[1])?;
                self.check_type_compatibility(&value_type, &v, span)?;
                Ok(map_type)
            }
            "remove" => {
                let k = self.infer_expression(&method_args[0])?;
                self.check_type_compatibility(&key_type, &k, span)?;
                Ok(map_type)
            }
            "keys" => Ok(Type::Array(Box::new(key_type))),
            "values" => Ok(Type::Array(Box::new(value_type))),
            "each" => {
                // `f` binds the key AND value; result ignored. Returns the receiver map.
                self.check_lambda_arg(&method_args[0], &[key_type, value_type], span)?;
                Ok(map_type)
            }
            other => unreachable!("unhandled map method {other}"),
        }
    }

    /// Type-check a built-in `Set` method call. `arguments[0]` is the receiver set (already
    /// known to be `Set(T)`); the remaining arguments are the method's own arguments.
    ///   - `has(x :: T)`   -> `Bool`
    ///   - `add(x :: T)`   -> `Set(T)` (a NEW set; the receiver is unchanged)
    ///   - `items()`       -> `[]T`
    ///   - `each(f: T => _)` -> the receiver set (so `.each` chains)
    pub(super) fn check_set_method(
        &mut self,
        method: &str,
        elem_type: Type,
        arguments: &[Expression],
        span: &Span,
    ) -> Result<Type, TypeError> {
        let method_args = &arguments[1..];
        let set_type = Type::Set(Box::new(elem_type.clone()));

        let expected = match method {
            "items" => 0,
            _ => 1,
        };
        if method_args.len() != expected {
            return Err(TypeError::WrongNumberOfArguments {
                expected,
                got: method_args.len(),
                span: span.clone(),
            });
        }

        match method {
            "has" => {
                let x = self.infer_expression(&method_args[0])?;
                self.check_type_compatibility(&elem_type, &x, span)?;
                Ok(Type::Bool)
            }
            "add" => {
                let x = self.infer_expression(&method_args[0])?;
                self.check_type_compatibility(&elem_type, &x, span)?;
                Ok(set_type)
            }
            "remove" => {
                let x = self.infer_expression(&method_args[0])?;
                self.check_type_compatibility(&elem_type, &x, span)?;
                Ok(set_type)
            }
            "items" => Ok(Type::Array(Box::new(elem_type))),
            "each" => {
                self.check_lambda_arg(&method_args[0], &[elem_type], span)?;
                Ok(set_type)
            }
            other => unreachable!("unhandled set method {other}"),
        }
    }

    /// Type-check a built-in `Text` method call. `arguments[0]` is the receiver (already
    /// known to be `Text`); the remaining arguments are the method's own arguments.
    ///
    /// The composable methods' bodies live in `corelib/text.qn` (receiver first,
    /// fail-loud ones with a trailing `Site`), but their TYPE surface is this table — a
    /// signature change there needs a matching edit here, and nothing checks the pair
    /// beyond the end-to-end tests. Signatures (see docs/types/text.md):
    ///   - `split(sep :: Text)`                 -> `[]Text`
    ///   - `trim()` / `trimStart()` / `trimEnd()` / `toUpper()` / `toLower()` -> `Text`
    ///   - `replaceAll(from :: Text, to :: Text)` -> `Text`
    ///   - `replace(from :: Text, to :: Text, count :: Num)` -> `Text` (first `count`)
    ///   - `contains(sub :: Text)`              -> `Bool`
    ///   - `indexOf(sub :: Text)`               -> `Result` (`Ok(Num)` / `NotOk`)
    ///   - `slice(start :: Num, end :: Num)`    -> `Text`
    ///   - `repeat(count :: Num)`               -> `Text` (`count` copies, joined)
    ///   - `at(index :: Num)`                   -> `Result` (`Ok(Text)` / `NotOk`)
    ///   - `graphemes()`                        -> `[]Text` (one length-1 Text each)
    pub(super) fn check_text_method(
        &mut self,
        method: &str,
        arguments: &[Expression],
        span: &Span,
    ) -> Result<Type, TypeError> {
        // `arguments[0]` (the receiver Text) was already inferred by the dispatch guard.
        let method_args = &arguments[1..];

        // Each method's expected parameter types and result type — the single table that
        // drives both the arity check and the per-argument type check below. `indexOf`
        // returns `Ok(Num)`/`NotOk` (no -1 sentinel); `split` returns `[]Text`.
        use Type::{Bool, Num, Text};
        let (parameters, result): (Vec<Type>, Type) = match method {
            "trim" | "trimStart" | "trimEnd" | "toUpper" | "toLower" => (vec![], Text),
            "split" => (vec![Text], Type::Array(Box::new(Text))),
            "graphemes" => (vec![], Type::Array(Box::new(Text))),
            "contains" => (vec![Text], Bool),
            "indexOf" => (vec![Text], result_of(Num)),
            "at" => (vec![Num], result_of(Text)),
            "slice" => (vec![Num, Num], Text),
            "repeat" => (vec![Num], Text),
            "replaceAll" => (vec![Text, Text], Text),
            "replace" => (vec![Text, Text, Num], Text),
            other => unreachable!("unhandled text method {other}"),
        };

        if method_args.len() != parameters.len() {
            return Err(TypeError::WrongNumberOfArguments {
                expected: parameters.len(),
                got: method_args.len(),
                span: span.clone(),
            });
        }
        for (arg, parameter_ty) in method_args.iter().zip(&parameters) {
            let arg_type = self.infer_expression(arg)?;
            self.check_type_compatibility(parameter_ty, &arg_type, span)?;
        }

        // Fail-loud contract for `replace`/`replaceAll`: reject at COMPILE time whatever is
        // determinable from literals (the runtime aborts on the rest — no silent no-ops).
        if method == "replace" || method == "replaceAll" {
            self.check_replace_literals(method, arguments, span)?;
        }
        // Same contract for `repeat`: a literal count that is negative or fractional is a
        // compile error; a computed one is the runtime's to reject.
        if method == "repeat"
            && let Some(count) = literal_number(&arguments[1])
            && (count < 0.0 || count.fract() != 0.0)
        {
            return Err(TypeError::InvalidBuiltinArgument {
                message: format!(
                    "repeat: `count` must be a whole number of 0 or more (got {count})"
                ),
                span: span.clone(),
            });
        }

        Ok(result)
    }

    /// Compile-time validation of `replace`/`replaceAll` arguments that are literals — the
    /// static half of the fail-loud contract (`core.text`'s implementations abort on the
    /// same conditions when they aren't literal-determinable):
    ///   - an empty `from` (`""`) is ill-defined → error (both methods);
    ///   - `replace`'s `count` literal `<= 0` → error (use `replaceAll` for "all");
    ///   - when the receiver, `from`, and `count` are ALL literals, a `count` greater than
    ///     the occurrences actually present → error.
    ///
    /// `arguments[0]` is the receiver; `arguments[1]` = `from`, `arguments[2]` = `to`, `arguments[3]` = `count`.
    pub(super) fn check_replace_literals(
        &self,
        method: &str,
        arguments: &[Expression],
        span: &Span,
    ) -> Result<(), TypeError> {
        let err = |message: String| {
            Err(TypeError::InvalidBuiltinArgument {
                message,
                span: span.clone(),
            })
        };

        // Empty `from` — a literal "" is ill-defined for both methods.
        if let Expression::String { value: from, .. } = &arguments[1]
            && from.is_empty()
        {
            return err("replace: `from` must not be empty".to_string());
        }

        if method != "replace" {
            return Ok(());
        }
        let Some(count) = literal_number(&arguments[3]) else {
            return Ok(()); // non-literal count — checked at runtime
        };
        // Truncate toward zero, matching codegen/runtime integer handling.
        let count = count.trunc();
        if count <= 0.0 {
            return err(format!(
                "replace count must be a positive integer (got {count}); \
                 use replaceAll to replace all occurrences"
            ));
        }
        // All-literal case: count occurrences of the literal `from` in the literal receiver
        // and reject a count that exceeds them (non-overlapping, matching `str::replacen`).
        if let (Expression::String { value: hay, .. }, Expression::String { value: from, .. }) =
            (&arguments[0], &arguments[1])
            && !from.is_empty()
        {
            let occurrences = hay.matches(from.as_str()).count() as f64;
            if count > occurrences {
                return err(format!(
                    "replace: count {count} exceeds {occurrences} occurrences of `{from}`"
                ));
            }
        }
        Ok(())
    }

    /// Type-check a lambda argument to an array method by binding its parameters to the
    /// given types and inferring the body. Records the lambda body's type in the type
    /// table (so codegen's oracle can size the inlined result). The lambda must declare
    /// exactly `parameter_types.len()` parameters; any parameter type annotation it carries
    /// must agree with the expected type. Returns the inferred body type.
    pub(super) fn check_lambda_arg(
        &mut self,
        arg: &Expression,
        parameter_types: &[Type],
        span: &Span,
    ) -> Result<Type, TypeError> {
        let Expression::Lambda {
            parameters, body, ..
        } = arg
        else {
            // An array method expects a *literal* lambda here, which it inlines per
            // element. Passing anything else (e.g. a bare name or a closure value) is
            // not supported in this position — higher-order values aren't accepted.
            return Err(TypeError::NotAFunction {
                got: self.infer_expression(arg)?,
                span: arg.span().clone(),
            });
        };
        if parameters.len() != parameter_types.len() {
            return Err(TypeError::WrongNumberOfArguments {
                expected: parameter_types.len(),
                got: parameters.len(),
                span: span.clone(),
            });
        }
        self.record_parameter_types(parameters, parameter_types);
        self.env.push_scope();
        for (parameter, ty) in parameters.iter().zip(parameter_types) {
            if let Some(ann) = &parameter.type_annotation {
                self.check_type_compatibility(ann, ty, &parameter.span)?;
            }
            self.env.define(
                parameter.name.clone(),
                ty.clone(),
                false,
                parameter.span.clone(),
            )?;
        }
        let body_type = self.infer_expression(body);
        self.env.pop_scope();
        let body_type = body_type?;
        // Record the lambda node's own type as its body type, for completeness.
        self.type_table
            .insert(arg.span().clone(), body_type.clone());
        Ok(body_type)
    }
}
