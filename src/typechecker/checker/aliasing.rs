//! Deep immutability: which bindings an expression's VALUE may alias.
//!
//! `=` freezes the value, not just the binding: a value reached through an `=` binding is
//! never reachable through a `:=` binding, in either direction. Records, arrays of them,
//! and the containers that hold them have reference semantics, so the checker tracks, for
//! every reference-typed expression, the bindings its value may be shared with — and the
//! binding, field-write, and setter-call gates reject the forms that would put one value
//! on both sides of the `=`/`:=` line. Scalars (`Num`/`Bool`/`Text`) copy and are exempt.
//!
//! A function or method is classified once, at its definition: may its result alias its
//! receiver/parameters (per slot), or a binding it captured? The call site — the one place
//! the argument's mutability is known — then substitutes the arguments in, so a getter
//! returning `it` stays callable on any receiver and its result inherits that receiver's
//! mutability.
//!
//! Part of the type checker; see `super` for the `TypeChecker` state these methods run
//! against.

use super::*;

/// The bindings a reference-typed expression's VALUE may alias. Empty means the value is
/// fresh: newly constructed, reachable through no existing binding, bindable either way.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ValueAliasing {
    /// `:=` bindings the value may alias, as (owning declaration, name).
    pub(super) mutable: Vec<(u64, String)>,
    /// `=` bindings the value may alias, as (owning declaration, name).
    pub(super) immutable: Vec<(u64, String)>,
    /// Parameters (receiver included) the value may alias, as
    /// (declaration, argument slot, name). A parameter's argument belongs to the caller,
    /// so its mutability is unknown inside the body: the value may not be aliased into a
    /// `:=` binding there, and a result built from it inherits the argument's mutability
    /// at each call site.
    pub(super) parameters: Vec<(u64, usize, String)>,
    /// Whether this value's aliasing reaches the ENCLOSING setter's own receiver `it` —
    /// a flag, not a name lookup, because `it` is an ordinary identifier (not a
    /// keyword): a `:=` local that happens to be named `it` is an ordinary mutable
    /// binding, not the receiver, however its name reads.
    pub(super) reaches_setter_receiver: bool,
}

impl ValueAliasing {
    pub(super) fn merge(&mut self, other: ValueAliasing) {
        self.mutable.extend(other.mutable);
        self.immutable.extend(other.immutable);
        self.parameters.extend(other.parameters);
        self.reaches_setter_receiver |= other.reaches_setter_receiver;
    }

    /// A binding whose immutability forbids reaching this value through `:=` — a
    /// parameter (whose argument may be `=`-bound at the call site), or an `=` binding.
    /// The flag says whether it is a parameter; a parameter wins the report, since a
    /// parameter is also its own immutable binding and the parameter reading explains why.
    pub(super) fn immutable_witness(&self) -> Option<(&str, bool)> {
        self.parameters
            .first()
            .map(|(_, _, name)| (name.as_str(), true))
            .or_else(|| {
                self.immutable
                    .first()
                    .map(|(_, name)| (name.as_str(), false))
            })
    }

    /// A `:=` binding this value may alias — what forbids freezing it with `=`.
    pub(super) fn mutable_witness(&self) -> Option<&str> {
        self.mutable.first().map(|(_, name)| name.as_str())
    }
}

/// A function's (or method's) result aliasing, evaluated once at its definition and
/// substituted at each call site. Slot 0 of `argument_slots` is the first argument — for
/// a method, its receiver.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResultAliasing {
    /// Bindings outside the declaration (captures, or a setter's known-mutable receiver)
    /// the result may alias, regardless of the arguments.
    pub(super) fixed: ValueAliasing,
    /// Argument positions whose value the result may alias.
    pub(super) argument_slots: Vec<usize>,
}

/// Whether values of `ty` are shared by reference AND writable through an alias — the
/// types the deep-immutability gates apply to. A record (named or anonymous) is. So is
/// every array/`Map`/`Set`, regardless of what it holds: `arr[i] := v`, `m.set(...)`, and
/// `s.add(...)` all mutate the underlying storage in place, so two bindings of the same
/// array/map/set alias each other's writes even when the element type is a plain `Num`. A
/// sum is when any variant payload is. Scalars (`Num`/`Bool`/`Text`) copy.
pub(super) fn is_reference_type(ty: &Type) -> bool {
    match ty {
        Type::Named { .. } | Type::Record(_) | Type::Array(_) | Type::Set(_) | Type::Map(_, _) => {
            true
        }
        Type::Sum { variants, .. } => variants
            .iter()
            .any(|variant| variant.fields.iter().any(is_reference_type)),
        _ => false,
    }
}

impl TypeChecker {
    /// The bindings `expression`'s value may alias. Sound over-approximation: an
    /// expression this cannot see through reports every reference-typed constituent.
    /// Runs only after `expression` was inferred, so every sub-expression's type is in
    /// the type table.
    pub(super) fn value_aliasing(&self, expression: &Expression) -> ValueAliasing {
        let mut out = ValueAliasing::default();
        match self.type_table.get(expression.span()) {
            Some(ty) if is_reference_type(ty) => {}
            _ => return out,
        }
        match expression {
            Expression::Identifier { name, span: _ } => {
                let Some(symbol) = self.env.lookup(name) else {
                    return out;
                };
                if symbol.constant {
                    return out;
                }
                if symbol.setter_receiver {
                    // A setter's receiver is mutable at every call site (the setter-call
                    // gate enforces it), and outlives the call — never dropped by the
                    // return filter, which is why it is owned by the enclosing declaration.
                    out.mutable.push((symbol.owner, name.clone()));
                    out.reaches_setter_receiver = true;
                    return out;
                }
                out = symbol.value_aliasing.clone();
                match symbol.mutable {
                    true => out.mutable.push((symbol.owner, name.clone())),
                    false => out.immutable.push((symbol.owner, name.clone())),
                }
            }
            Expression::FieldAccess { expression, .. }
            | Expression::Index { expression, .. }
            | Expression::Spread { expression, .. } => {
                out = self.value_aliasing(expression);
            }
            Expression::Array { elements, .. } | Expression::SetLiteral { elements, .. } => {
                for element in elements {
                    out.merge(self.value_aliasing(element));
                }
            }
            Expression::MapLiteral { entries, .. } => {
                for (key, value) in entries {
                    out.merge(self.value_aliasing(key));
                    out.merge(self.value_aliasing(value));
                }
            }
            Expression::Record { fields, .. } | Expression::Constructor { fields, .. } => {
                for (_, value) in fields {
                    match value {
                        // `<-source` copies the source's fields into the new record: the
                        // record itself is fresh, but any reference-typed FIELD is shared
                        // with the source, so the source's aliasing carries over exactly
                        // when it has one.
                        Expression::Spread {
                            expression: source, ..
                        } => {
                            if self.spread_shares_fields(source) {
                                out.merge(self.value_aliasing(source));
                            }
                        }
                        _ => out.merge(self.value_aliasing(value)),
                    }
                }
            }
            Expression::Call {
                function,
                arguments,
                member_call,
                ..
            } => {
                out = self.call_result_aliasing(function, arguments, *member_call);
            }
            Expression::BinaryOperator {
                left,
                operator,
                right,
                ..
            } => {
                // A reference-typed operator result: a user operator member's classified
                // aliasing where one resolves, else conservatively both operands (the
                // built-in array/set forms share their operands' elements).
                out = match self.operator_member_aliasing(operator.symbol(), left, right) {
                    Some(result_aliasing) => {
                        self.apply_result_aliasing(&result_aliasing, &[left, right])
                    }
                    None => {
                        let mut both = self.value_aliasing(left);
                        both.merge(self.value_aliasing(right));
                        both
                    }
                };
            }
            Expression::If { then, else_, .. } => {
                out = self.value_aliasing(then);
                out.merge(self.value_aliasing(else_));
            }
            Expression::Match { span, .. } => {
                // The arms' union, computed by `check_match` while each arm's pattern
                // bindings were still in scope — a walk here could not resolve them.
                if let Some(cached) = self.match_aliasing.get(span) {
                    out = cached.clone();
                }
            }
            Expression::Block { statements, .. } => {
                if let Some(crate::ast::Statement::Expression(last)) = statements.last() {
                    out = self.value_aliasing(last);
                }
            }
            // Literals, lambdas, interpolations, unary operators, and ranges build fresh
            // values (or are not reference-typed at all, filtered above).
            _ => {}
        }
        out
    }

    /// Whether spreading `source` into a record literal shares anything: true exactly
    /// when the source record has a reference-typed field.
    fn spread_shares_fields(&self, source: &Expression) -> bool {
        match self.type_table.get(source.span()) {
            Some(Type::Named { fields, .. }) => fields.iter().any(|(_, ty)| is_reference_type(ty)),
            Some(Type::Record(fields)) => fields.iter().any(|(_, ty)| is_reference_type(ty)),
            _ => false,
        }
    }

    /// Substitute a call's arguments into the callee's classified result aliasing.
    fn apply_result_aliasing(
        &self,
        result_aliasing: &ResultAliasing,
        arguments: &[&Expression],
    ) -> ValueAliasing {
        let mut out = result_aliasing.fixed.clone();
        for &slot in &result_aliasing.argument_slots {
            if let Some(argument) = arguments.get(slot) {
                out.merge(self.value_aliasing(argument));
            }
        }
        out
    }

    /// A call whose callee's aliasing is unknown (a function value, a not-yet-classified
    /// overload member): assume the result may alias every argument.
    fn every_argument_aliasing(&self, arguments: &[Expression]) -> ValueAliasing {
        let mut out = ValueAliasing::default();
        for argument in arguments {
            out.merge(self.value_aliasing(argument));
        }
        out
    }

    /// The aliasing of a reference-typed call result, resolved the way `check_call`
    /// resolved the call: a declared method by its receiver's type, a built-in collection
    /// method by table, an overload member by exact argument types, a named function by
    /// its classified binding — and conservatively (every argument) where none of those
    /// answer.
    fn call_result_aliasing(
        &self,
        function: &Expression,
        arguments: &[Expression],
        member_call: bool,
    ) -> ValueAliasing {
        let Expression::Identifier { name, .. } = function else {
            // The callee is an expression rather than a plain name — most notably an
            // immediately invoked lambda (`(() => c)()`). Its classified CAPTURES
            // (resolved the same way a named function's are, see `callable_result_aliasing`)
            // are folded in on top of the conservative "every argument" aliasing below —
            // which already covers a returned PARAMETER (an argument slot is always one of
            // "every argument"), so only `.fixed` is needed here, not the slot
            // substitution `apply_result_aliasing` would repeat.
            let mut out = self.callable_result_aliasing(function).fixed;
            out.merge(self.every_argument_aliasing(arguments));
            return out;
        };

        if member_call {
            let receiver_type = arguments
                .first()
                .and_then(|receiver| self.type_table.get(receiver.span()));
            let borrowed: Vec<&Expression> = arguments.iter().collect();
            return match receiver_type {
                Some(Type::Named {
                    name: type_name, ..
                })
                | Some(Type::Sum {
                    name: type_name, ..
                }) => {
                    let result_aliasing = self
                        .method_result_aliasing
                        .get(&(type_name.clone(), name.clone()))
                        .cloned()
                        .unwrap_or_default();
                    self.apply_result_aliasing(&result_aliasing, &borrowed)
                }
                Some(receiver_ty @ (Type::Array(_) | Type::Map(_, _) | Type::Set(_))) => {
                    let result_aliasing = ResultAliasing {
                        fixed: ValueAliasing::default(),
                        argument_slots: self.builtin_collection_method_slots(
                            name,
                            receiver_ty,
                            &borrowed,
                        ),
                    };
                    let mut out = self.apply_result_aliasing(&result_aliasing, &borrowed);
                    // `map` and `reduce` thread their callback's RETURN value into the
                    // built-in's own result, so a callback that captures and returns an
                    // outer value regardless of its input leaks that capture the same way
                    // returning a receiver element does. `each`/`filter`/`find`/`at` don't
                    // — their result comes from the receiver's own elements, the callback's
                    // return discarded or used only as a predicate — so they need no fold.
                    if let Some(callback) =
                        callback_argument_slot(name).and_then(|slot| arguments.get(slot))
                    {
                        out.merge(self.callable_result_aliasing(callback).fixed);
                    }
                    out
                }
                // `Text` methods return fresh values.
                Some(Type::Text) => ValueAliasing::default(),
                _ => self.every_argument_aliasing(arguments),
            };
        }

        // A sum constructor is a container: the built value holds its payload arguments.
        if self.sum_variant_owner(name).is_some() {
            return self.every_argument_aliasing(arguments);
        }

        let borrowed: Vec<&Expression> = arguments.iter().collect();
        if let Some(set) = self.overloads.get(name) {
            let argument_types: Option<Vec<Type>> = arguments
                .iter()
                .map(|argument| self.type_table.get(argument.span()).cloned())
                .collect();
            if let Some(argument_types) = argument_types
                && let Some(member) = set.iter().find(|overload| {
                    crate::ast::parameters_accept(&overload.parameters, &argument_types, |p, a| {
                        types_match(p, a)
                    })
                })
            {
                return match &member.result_aliasing {
                    Some(result_aliasing) => self.apply_result_aliasing(result_aliasing, &borrowed),
                    None => self.every_argument_aliasing(arguments),
                };
            }
            return self.every_argument_aliasing(arguments);
        }

        match self
            .env
            .lookup(name)
            .and_then(|symbol| symbol.result_aliasing.as_ref())
        {
            Some(result_aliasing) => {
                self.apply_result_aliasing(&result_aliasing.clone(), &borrowed)
            }
            None => self.every_argument_aliasing(arguments),
        }
    }

    /// What CALLING a function-typed `expression` may alias — a lambda literal's own
    /// classified captures, a name bound to a classified function or closure, either
    /// branch of an `If` (mirroring `value_aliasing`'s own handling of one), a block's
    /// tail, or a call whose own result is itself a closure (a function returning one,
    /// resolved through that function's classification, substituted against the call's
    /// OWN arguments exactly as a reference-typed result would be). `Expression::Lambda`
    /// aside, only forms the two issues' repros need are covered — an expression this
    /// cannot see through answers fresh, same as `value_aliasing`'s own gaps elsewhere.
    ///
    /// Used wherever a function value is called without going through `check_call`'s named
    /// path: an immediately invoked lambda, and a higher-order built-in's callback
    /// argument.
    pub(super) fn callable_result_aliasing(&self, expression: &Expression) -> ResultAliasing {
        match expression {
            Expression::Lambda { body, .. } => self
                .lambda_result_aliasing
                .get(body.span())
                .cloned()
                .unwrap_or_default(),
            Expression::Identifier { name, .. } => self
                .env
                .lookup(name)
                .and_then(|symbol| symbol.result_aliasing.clone())
                .unwrap_or_default(),
            Expression::Block { statements, .. } => match statements.last() {
                Some(crate::ast::Statement::Expression(tail)) => {
                    self.callable_result_aliasing(tail)
                }
                _ => ResultAliasing::default(),
            },
            Expression::If { then, else_, .. } => {
                let mut result = self.callable_result_aliasing(then);
                let other = self.callable_result_aliasing(else_);
                result.fixed.merge(other.fixed);
                result.argument_slots.extend(other.argument_slots);
                result
            }
            // A call whose own result is itself a closure (a function returning one) is
            // resolved through what CALLING its callee returns, substituted against THIS
            // call's own arguments — the same substitution `apply_result_aliasing` does
            // for a reference-typed result, one layer in. Without it, a curried function
            // returning a closure over one of ITS OWN parameters (`mk = (v :: T) -> () ->
            // T => < () -> T => < v > >`) would report every call's result as aliasing
            // `mk`'s parameter unconditionally, rather than whatever `mk` was actually
            // called with.
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                let callee_result_aliasing = self.callable_result_aliasing(function);
                let borrowed: Vec<&Expression> = arguments.iter().collect();
                ResultAliasing {
                    fixed: self.apply_result_aliasing(&callee_result_aliasing, &borrowed),
                    argument_slots: Vec::new(),
                }
            }
            _ => ResultAliasing::default(),
        }
    }

    /// Re-bucket a returned closure's classification — computed one declaration deeper,
    /// at the lambda literal itself — as THIS declaration's own: a reference to this
    /// declaration's own parameter, forwarded there as a `fixed.parameters` entry (an
    /// outer declaration's parameter, from the inner lambda's point of view), becomes
    /// this declaration's own argument slot, substituted at each of ITS OWN call sites —
    /// exactly as a directly-returned parameter already is
    /// (`declaration_result_aliasing`'s `Equal` case). The duplicate `fixed.mutable`/
    /// `fixed.immutable` entry `value_aliasing` also records for every parameter
    /// reference is dropped for the same (declaration, name), so a resolved parameter
    /// does not ALSO linger as a permanent witness alongside its real, per-call one.
    pub(super) fn reclassify_returned_closure(&self, inner: ResultAliasing) -> ResultAliasing {
        let current = self.current_declaration;
        let mut result = ResultAliasing::default();
        let mut resolved: Vec<(u64, String)> = Vec::new();
        for (declaration, slot, name) in inner.fixed.parameters {
            match declaration.cmp(&current) {
                std::cmp::Ordering::Equal => {
                    result.argument_slots.push(slot);
                    resolved.push((declaration, name));
                }
                std::cmp::Ordering::Less => {
                    result.fixed.parameters.push((declaration, slot, name));
                }
                std::cmp::Ordering::Greater => {}
            }
        }
        result.fixed.mutable = inner
            .fixed
            .mutable
            .into_iter()
            .filter(|entry| !resolved.contains(entry))
            .collect();
        result.fixed.immutable = inner
            .fixed
            .immutable
            .into_iter()
            .filter(|entry| !resolved.contains(entry))
            .collect();
        result
    }

    /// A user operator member's classified result aliasing for these operands, where the
    /// operands resolve one. `None` says nothing was classified — the caller falls back to
    /// both operands.
    fn operator_member_aliasing(
        &self,
        operator_name: &str,
        left: &Expression,
        right: &Expression,
    ) -> Option<ResultAliasing> {
        let left_type = self.type_table.get(left.span())?;
        let right_type = self.type_table.get(right.span())?;
        let operand_types = [left_type.clone(), right_type.clone()];
        self.overloads
            .get(operator_name)?
            .iter()
            .find(|overload| {
                crate::ast::parameters_accept(&overload.parameters, &operand_types, |p, a| {
                    types_match(p, a)
                })
            })?
            .result_aliasing
            .clone()
    }

    /// Classify a declaration's result: the bindings its returned value may alias, split
    /// into this declaration's own argument slots (substituted per call) and everything
    /// that outlives the call (captures). Bindings the declaration itself owns — locals,
    /// and anything nested deeper — die at the return, so they drop out: a function
    /// building a record in a local and returning it returns a fresh value.
    ///
    /// Runs while the body's scope is still pushed, so parameters and locals resolve.
    pub(super) fn declaration_result_aliasing(
        &self,
        body: &Expression,
        return_type: &Type,
    ) -> ResultAliasing {
        if !is_reference_type(return_type) {
            return ResultAliasing::default();
        }
        let aliasing = self.value_aliasing(body);
        let current = self.current_declaration;
        let mut result = ResultAliasing::default();
        for (owner, name) in aliasing.mutable {
            if owner < current {
                result.fixed.mutable.push((owner, name));
            }
        }
        for (owner, name) in aliasing.immutable {
            if owner < current {
                result.fixed.immutable.push((owner, name));
            }
        }
        for (declaration, slot, name) in aliasing.parameters {
            match declaration.cmp(&current) {
                std::cmp::Ordering::Equal => result.argument_slots.push(slot),
                std::cmp::Ordering::Less => result.fixed.parameters.push((declaration, slot, name)),
                std::cmp::Ordering::Greater => {}
            }
        }
        result
    }

    /// Classify a lambda's own captures and cache them keyed by the BODY's span, for
    /// [`Self::callable_result_aliasing`] to find later at a call site the lambda itself
    /// never sees (an immediately invoked lambda, or a higher-order built-in's callback
    /// argument). Run while the lambda's scope is still pushed and BEFORE it pops, the
    /// same window `declaration_result_aliasing` needs.
    ///
    /// A lambda whose OWN body is itself a closure (`() -> () -> T => < () -> T => < c > >`,
    /// three levels of nesting) is classified the same way `check_function_declaration`
    /// classifies a function returning one: through what CALLING the returned closure
    /// aliases, re-bucketed as this lambda's own — `declaration_result_aliasing` alone
    /// would see a `Type::Function` body and answer fresh, the same gap a closure-
    /// returning named function has without that branch.
    pub(super) fn record_lambda_result_aliasing(&mut self, body: &Expression, body_type: &Type) {
        let captured_aliasing = match body_type {
            Type::Function { .. } => {
                self.reclassify_returned_closure(self.callable_result_aliasing(body))
            }
            _ => self.declaration_result_aliasing(body, body_type),
        };
        if captured_aliasing != ResultAliasing::default() {
            self.lambda_result_aliasing
                .insert(body.span().clone(), captured_aliasing);
        }
    }

    /// The name to blame when writing through `receiver` would break an `=` guarantee:
    /// the immutable binding or parameter its value may alias. `None` means the write is
    /// allowed — the value is fresh, or reached only through `:=` bindings.
    pub(super) fn immutable_write_witness(&self, receiver: &Expression) -> Option<String> {
        self.value_aliasing(receiver)
            .immutable_witness()
            .map(|(name, _)| name.to_string())
    }

    /// A store across the line is a compile error at the store: `value` may not alias an
    /// `=` binding or a parameter once it is confirmed reaching a `:=`-reachable
    /// container — the shared check behind a field write's stored value and a setter
    /// argument the setter stores into `it`.
    pub(super) fn check_store_not_crossing(
        &self,
        value: &Expression,
        span: &Span,
    ) -> Result<(), TypeError> {
        if let Some((witness, parameter)) = self.value_aliasing(value).immutable_witness() {
            return Err(TypeError::MutableStoreOfImmutable {
                aliased: witness.to_string(),
                parameter,
                span: span.clone(),
            });
        }
        Ok(())
    }

    /// Enter a new declaration (function, method, or lambda) for aliasing bookkeeping;
    /// returns the enclosing declaration's id for [`Self::leave_declaration`].
    pub(super) fn enter_declaration(&mut self) -> u64 {
        let previous = self.current_declaration;
        self.declaration_counter += 1;
        self.current_declaration = self.declaration_counter;
        previous
    }

    pub(super) fn leave_declaration(&mut self, previous: u64) {
        self.current_declaration = previous;
    }

    /// Which argument slots (0 = the receiver) a built-in collection method's result
    /// actually aliases. `each`/`set`/`add`/`remove` mutate the receiver in place and
    /// literally return it — slot 0 regardless of element type. Every other method
    /// (`filter`, `map`, `at`, `find`, `keys`, `values`, `items`, `get`, and `reduce`'s own
    /// slot, the init argument) builds a FRESH container instead — or extracts a single
    /// element — which shares an alias only through the value it carries: a `[]Point`
    /// result still shares its records, but a `[]Num` result shares nothing a write could
    /// reach, so `is_reference_type` on that shared type gates the slot.
    /// `filter`/`at`/`find`/`keys`/`values`/`get`/`items` share the receiver's own
    /// element/key/value type; `map` and `reduce` may change it, so those two are resolved
    /// from the callback's return type (already in the type table — the callback was
    /// inferred at the definition/check site) and the init argument's type respectively.
    fn builtin_collection_method_slots(
        &self,
        name: &str,
        receiver_type: &Type,
        arguments: &[&Expression],
    ) -> Vec<usize> {
        if matches!(name, "each" | "set" | "add" | "remove") {
            return vec![0];
        }
        let shared_type = match (name, receiver_type) {
            ("filter" | "at" | "find", Type::Array(elem)) => Some((**elem).clone()),
            ("keys", Type::Map(key, _)) => Some((**key).clone()),
            ("values" | "get", Type::Map(_, value)) => Some((**value).clone()),
            ("items", Type::Set(elem)) => Some((**elem).clone()),
            ("map", _) => arguments.get(1).and_then(|callback| match callback {
                Expression::Lambda { body, .. } => self.type_table.get(body.span()).cloned(),
                _ => None,
            }),
            ("reduce", _) => arguments
                .get(1)
                .and_then(|init| self.type_table.get(init.span()).cloned()),
            _ => None,
        };
        let slot = if name == "reduce" { 1 } else { 0 };
        match shared_type {
            Some(ty) if is_reference_type(&ty) => vec![slot],
            _ => vec![],
        }
    }
}

/// The argument index (0 = the receiver) of `map`'s and `reduce`'s callback — the only
/// two built-in collection methods whose RESULT threads the callback's return value
/// through, so a captured value the callback returns regardless of its own arguments
/// leaks into the call's result there. `each`'s and `filter`'s callbacks run for effect
/// or as a predicate; their return value never reaches the result.
fn callback_argument_slot(name: &str) -> Option<usize> {
    match name {
        "map" => Some(1),
        "reduce" => Some(2),
        _ => None,
    }
}
