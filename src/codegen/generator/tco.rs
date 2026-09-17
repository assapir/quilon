//! Self-tail-call optimization: the tail-position analysis and the loop lowering it
//! enables (parameter slots plus a back-edge instead of a stack-growing call).
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state these
//! methods run against.

use super::*;

/// Where an if/match arm's body is emitted: [`generate_if`](CodeGenerator::generate_if) and
/// [`generate_match`](CodeGenerator::generate_match) always want a value back, while
/// [`generate_tail_if`](CodeGenerator::generate_tail_if) and
/// [`generate_tail_match`](CodeGenerator::generate_tail_match) want a self-tail-call left
/// free to end the arm's block with a back-edge instead. `CodeGenerator::emit_body` is the
/// one place that difference lives; `if_position`/`match_position` (in `exprs`/`matching`)
/// are otherwise identical for either position.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BodyPosition {
    /// A self-tail-call back-edge may end the body's block with no value — see
    /// `generate_tail_expression`.
    Tail,
    /// The body always yields a value.
    Value,
}

impl<'ctx> CodeGenerator<'ctx> {
    /// Emit an if/match arm's body at `position`. Value position always returns `Some`
    /// (`generate_expression` never leaves a block mid-air); tail position may return `None`
    /// with the current block already terminated — see `generate_tail_expression`.
    pub(super) fn emit_body(
        &mut self,
        position: BodyPosition,
        expression: &Expression,
    ) -> Result<Option<BasicValueEnum<'ctx>>, String> {
        match position {
            BodyPosition::Tail => self.generate_tail_expression(expression),
            BodyPosition::Value => self.generate_expression(expression).map(Some),
        }
    }

    // ---- Self-tail-call optimization (loop lowering) --------------------------------
    //
    // A call is in **tail position** when it is the value the enclosing function returns
    // directly — i.e. nothing happens to its result before the `ret`. Tail position flows
    // through exactly the constructs that yield a value as their tail without further
    // computation: a block's last expression, both arms of an `if`/ternary, every arm of a
    // `?`/`|` match, a parenthesizing pipeline's desugaring, and the function body itself.
    // It does NOT flow into the operand of a `+`/`*`/comparison, a call argument, an array
    // element, etc. — those consume the value, so a call there is not in tail position.
    //
    // `body_has_self_tail_call` is the pure analysis (no IR), used once to decide whether
    // to set up the loop. `generate_tail_expression` is the codegen counterpart: it walks the
    // SAME tail-position structure and, at a tail self-call, rewrites the parameter slots and
    // branches to the loop header; everything else (and every non-tail subexpression) goes
    // through the ordinary `generate_expression`. The two must agree on what "tail position" is.

    /// Does `declaration`'s body contain a self-call in tail position? Pure (emits no IR).
    /// `self_symbol` is the LLVM symbol the function is emitted under (mangled if
    /// overloaded) — passed in from `emit_module_function` so the "which symbol?" rule
    /// lives in one place, and a tail call is recognized as a SELF-call by matching it.
    pub(super) fn body_has_self_tail_call(
        &self,
        declaration: &FunctionDeclaration,
        self_symbol: &str,
    ) -> Result<bool, String> {
        self.expression_has_self_tail_call(
            &declaration.body,
            self_symbol,
            declaration.parameters.len(),
        )
    }

    /// Whether `expression`, evaluated in tail position, contains a self-call (to `self_symbol`
    /// with `arity` args). Recurses only through tail-position sub-expressions.
    pub(super) fn expression_has_self_tail_call(
        &self,
        expression: &Expression,
        self_symbol: &str,
        arity: usize,
    ) -> Result<bool, String> {
        match expression {
            Expression::Call { .. } => self.is_self_tail_call(expression, self_symbol, arity),
            Expression::Block { statements, .. } => match statements.last() {
                Some(crate::ast::Statement::Expression(tail)) => {
                    self.expression_has_self_tail_call(tail, self_symbol, arity)
                }
                _ => Ok(false),
            },
            Expression::If { then, else_, .. } => {
                Ok(
                    self.expression_has_self_tail_call(then, self_symbol, arity)?
                        || self.expression_has_self_tail_call(else_, self_symbol, arity)?,
                )
            }
            Expression::Match { arms, .. } => {
                for arm in arms {
                    if self.expression_has_self_tail_call(&arm.body, self_symbol, arity)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    /// Whether `expression` is a direct call that resolves to `self_symbol` with `arity` args —
    /// i.e. the function calling itself. Resolution mirrors `generate_call`'s: a plain
    /// name maps to itself, an overloaded name to its exact mangled member by argument
    /// types. A constructor/method/intrinsic call (which `generate_call` routes elsewhere)
    /// is never a self-call. NB only the *callee identity* matters here; the arguments are
    /// generated normally by `generate_tail_expression`.
    pub(super) fn is_self_tail_call(
        &self,
        expression: &Expression,
        self_symbol: &str,
        arity: usize,
    ) -> Result<bool, String> {
        let Expression::Call {
            function,
            arguments,
            member_call,
            ..
        } = expression
        else {
            return Ok(false);
        };
        let Expression::Identifier { name, .. } = function.as_ref() else {
            return Ok(false);
        };
        // A self-call may leave off a trailing `Site` for the compiler to fill in, and it is
        // still the same call — so its arity matches one short of the parameter slots. This
        // is what keeps a recursive function that adopts the facility a LOOP: without it the
        // call is emitted as a real call, and the language's only iteration mechanism
        // silently starts overflowing the stack.
        if arguments.len() != arity
            && !(arguments.len() + 1 == arity
                && self.fills_call_site(name, arguments, *member_call)?)
        {
            return Ok(false);
        }
        // A name shadowed by a sum-type constructor, or a call that lowers to a runtime
        // intrinsic instead of a Quilon function, is not a self-call — the same question
        // call lowering asks, so the two cannot disagree (see `intrinsic_lowering`).
        if self.sum_variants.contains_key(name.as_str())
            || self.intrinsic_lowering(name, arguments)?.is_some()
        {
            return Ok(false);
        }
        // A method on the receiver's type claims the name first, exactly as in call
        // lowering — so a function calling `recv.name(...)` on a type whose method shares
        // its name is a call to that method, not a self-call.
        let symbol = match self.method_symbol_for(name, arguments, *member_call)? {
            Some(method) => method,
            // A `.` call the receiver's type answers with a BUILT-IN declares no method
            // symbol, and it is still not this function: `t.contains(s)` inside a
            // top-level `contains` is `Text`'s, so taking it for recursion compiled the
            // call into this function's own loop back-edge.
            None if *member_call => return Ok(false),
            None if self.overloads.contains_key(name.as_str()) => {
                let arg_types = self.oracle_argument_types(arguments)?;
                match self.resolve_overload_symbol(name, &arg_types) {
                    Some(s) => s,
                    None => return Ok(false),
                }
            }
            None => name.clone(),
        };
        Ok(symbol == self_symbol)
    }

    /// Emit `expression` in tail position under an active [`Tco`] context. Returns `Some(value)`
    /// for an ordinary tail (the caller `ret`s it) or `None` when this path does not fall
    /// through to a normal return — every tail exit was a self-call. **Invariant:** on
    /// `None`, the current insert block is already TERMINATED (by the back-edge `br` of a
    /// tail self-call, or an `unreachable` for an if/match all of whose arms recurse), so
    /// the caller must not emit anything more into it. Walks the same tail-position
    /// structure as `expression_has_self_tail_call`; any non-tail node falls through to
    /// `generate_expression` (always `Some`).
    pub(super) fn generate_tail_expression(
        &mut self,
        expression: &Expression,
    ) -> Result<Option<BasicValueEnum<'ctx>>, String> {
        let arity = self
            .tco
            .as_ref()
            .expect("generate_tail_expression without a TCO context")
            .parameter_slots
            .len();

        match expression {
            // A call in tail position: if it resolves to THIS function, lower it to the
            // loop back-edge; otherwise it is an ordinary value. Clone `self_symbol` only
            // here (a call leaf), not on every tail node. A `Some` from
            // `emit_tail_self_call` means it declined the back-edge and emitted a plain
            // call instead, whose value is an ordinary tail value.
            Expression::Call {
                arguments, span, ..
            } => {
                let self_symbol = self.tco.as_ref().unwrap().self_symbol.clone();
                if self.is_self_tail_call(expression, &self_symbol, arity)? {
                    // A self-call that omitted its trailing `Site` gets one filled in for
                    // the loop's parameter slot, exactly as an ordinary call would.
                    let site = match arguments.len() < arity {
                        true => Some(span.clone()),
                        false => None,
                    };
                    self.emit_tail_self_call(arguments, site.as_ref())
                } else {
                    Ok(Some(self.generate_expression(expression)?))
                }
            }

            Expression::Block { statements, span } => {
                // Emit every statement normally except the tail expression, which stays in
                // tail position. A non-`Expression`-tail block (ends in an item) has no tail call
                // (the analysis returned false), so generating it whole is correct.
                match statements.split_last() {
                    Some((crate::ast::Statement::Expression(tail), init)) => {
                        for statement in init {
                            match statement {
                                crate::ast::Statement::Item(item) => self.generate_item(item)?,
                                crate::ast::Statement::Expression(e) => {
                                    self.generate_expression(e)?;
                                }
                            }
                        }
                        self.generate_tail_expression(tail)
                    }
                    _ => Ok(Some(self.generate_block(statements, span)?)),
                }
            }

            Expression::If {
                condition,
                then,
                else_,
                ..
            } => self.generate_tail_if(condition, then, else_),

            Expression::Match {
                expression: scrutinee,
                arms,
                ..
            } => self.generate_tail_match(expression, scrutinee, arms),

            // Anything else in tail position is an ordinary value.
            other => Ok(Some(self.generate_expression(other)?)),
        }
    }

    /// Lower a tail self-call: evaluate the argument expressions, write them into the
    /// parameter slots, then `br` back to the loop header — returning `None`, since this
    /// path never falls through to a return. All args are evaluated into temporaries
    /// BEFORE any slot is overwritten, so an argument that reads a parameter (e.g.
    /// `f(n - 1, acc + n)` reading `n` for `acc`) sees the current iteration's values,
    /// not a half-updated set.
    ///
    /// A slot only accepts a value of its parameter's type, and the values arrive here on
    /// the strength of a call resolution made from *inferred* argument types. Should that
    /// inference ever disagree with the declared type, storing anyway would write the
    /// wrong-sized value into the frame — silent corruption. So the values are checked
    /// against the function's own signature (the slots were built from the same
    /// declaration, in order) and a mismatch declines the loop: the already-evaluated
    /// values — each argument is evaluated exactly once, whichever way this goes — are
    /// passed to an ordinary call to the same function, and its result becomes the tail
    /// value. Recursion depth is then bounded by the stack again for that call, which is
    /// the conservative half of the trade.
    pub(super) fn emit_tail_self_call(
        &mut self,
        arguments: &[Expression],
        fill_site: Option<&Span>,
    ) -> Result<Option<BasicValueEnum<'ctx>>, String> {
        let mut new_vals: Vec<BasicValueEnum<'ctx>> = arguments
            .iter()
            .map(|a| self.generate_expression(a))
            .collect::<Result<Vec<_>, _>>()?;
        // The call's own location, for a callee (itself) whose last parameter is a `Site`.
        if let Some(span) = fill_site {
            new_vals.push(self.site_value(span)?);
        }
        let tco = self
            .tco
            .as_ref()
            .expect("emit_tail_self_call without a TCO context");
        let slots_fit = tco
            .function
            .get_params()
            .iter()
            .zip(&new_vals)
            .all(|(parameter, val)| val.get_type() == parameter.get_type());
        if !slots_fit {
            let function = tco.function;
            return self.emit_call(function, &new_vals).map(Some);
        }
        // Snapshot slots + header + body span before the mutable stores (releases the
        // `self.tco` borrow so the `&mut self` builder calls below are allowed).
        let slots: Vec<PointerValue<'ctx>> = tco.parameter_slots.clone();
        let header = tco.header;
        let body_span = tco.body_span.clone();
        for (slot, val) in slots.iter().zip(new_vals) {
            self.builder
                .build_store(*slot, val)
                .map_err(ctx("Failed to store tail-call arg"))?;
        }
        // This back-edge IS this iteration's block close: join whatever it launched
        // directly before looping back to the header, which opens the NEXT iteration's
        // scope fresh (a no-op pair for a body that launches nothing).
        self.exit_launch_scope(&body_span)?;
        self.builder
            .build_unconditional_branch(header)
            .map_err(ctx("Failed to branch to loop header"))?;
        Ok(None)
    }

    /// Tail-position `if`/ternary: [`if_position`](CodeGenerator::if_position) with each arm
    /// emitted in tail position, so an arm that tail-recurses branches to the loop header
    /// (yielding no value) instead of joining the merge block. We `phi` only over the
    /// value-producing arms — if both arms tail self-call, there is no merge value and we
    /// return `None`.
    pub(super) fn generate_tail_if(
        &mut self,
        condition: &Expression,
        then_expression: &Expression,
        else_expression: &Expression,
    ) -> Result<Option<BasicValueEnum<'ctx>>, String> {
        self.if_position(
            condition,
            then_expression,
            else_expression,
            BodyPosition::Tail,
        )
    }

    /// Tail-position `?`/`|` match: [`match_position`](CodeGenerator::match_position) with
    /// each arm body emitted in tail position. An arm that tail-recurses branches to the loop
    /// header and stores nothing; an arm that yields a value stores it into the shared result
    /// slot and falls through to the continuation. If EVERY arm tail-recurses, the
    /// continuation is unreachable and we return `None` (no result to load).
    pub(super) fn generate_tail_match(
        &mut self,
        match_expression: &Expression,
        scrutinee: &Expression,
        arms: &[MatchArm],
    ) -> Result<Option<BasicValueEnum<'ctx>>, String> {
        self.match_position(match_expression, scrutinee, arms, BodyPosition::Tail)
    }
}
