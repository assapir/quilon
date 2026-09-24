//! The top-level signal trap (`!>`): each arm lowers to its own top-level function taking
//! the sender's `pid`/`uid` as two `Num`s and returning nothing, registered with the
//! runtime at program start (`emit_entry_dispatch`, right after `__ql_init` and before `^`
//! runs) through one `__trap_install(signalIndex, armFn)` call per arm.
//!
//! Unlike a `net.@tcpServe` handler (a closure value, called back indirectly through a
//! bundled environment — see `calls::emit_tcp_serve_handler_thunk`), a trap arm captures
//! nothing: only the file that defines `^` may declare a trap, so its body is ordinary
//! top-level code, generated directly into the arm's own function — no closure bundle, no
//! indirect call.
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state these
//! methods run against.

use super::*;
use crate::ast::TrapDeclaration;

impl<'ctx> CodeGenerator<'ctx> {
    /// Bare `process.Signal` variant names, in the fixed order both `corelib/process.qn`'s
    /// `Signal` declaration and the runtime's own `TRAP_SIGNALS`
    /// (`quilon-rt/src/trap.rs`) use. The index into this array — never a raw OS signal
    /// number, which differs across targets (Linux's `SIGUSR1`/`SIGUSR2` are 10/12;
    /// macOS's are 30/31) — is what `__trap_install` takes.
    const SIGNAL_VARIANT_ORDER: [&'static str; 7] = [
        "Hangup",
        "Interrupt",
        "Quit",
        "Terminate",
        "Alarm",
        "UserDefined1",
        "UserDefined2",
    ];

    /// Generate every arm of `trap` as its own function, recording each as `(signal
    /// index, function)` in `self.trap_arms` for `emit_entry_dispatch` to install. The
    /// checker has already rejected anything but a `process.Signal` constructor pattern
    /// here (see `TypeChecker::check_trap_arms`), so both lookups below are infallible.
    pub(super) fn generate_trap(&mut self, trap: &TrapDeclaration) -> Result<(), String> {
        for (arm_index, arm) in trap.arms.iter().enumerate() {
            let Pattern::Constructor { name, .. } = &arm.pattern else {
                return Err(
                    "a signal trap arm's pattern must name a process.Signal variant".to_string(),
                );
            };
            let signal_index = Self::SIGNAL_VARIANT_ORDER
                .iter()
                .position(|variant| *variant == name.as_str())
                .ok_or_else(|| format!("'{name}' is not a known process.Signal variant"))?;
            let symbol = format!("__trap_arm_{arm_index}_{name}");
            let function = self.generate_trap_arm_function(arm, &symbol)?;
            self.trap_arms.push((signal_index, function));
        }
        Ok(())
    }

    /// Build one arm's function: `void (double pid, double uid)`, internal linkage. Builds
    /// the `Sender { pid, uid }` record from the two `Num` parameters the runtime passes
    /// (built the way `build_plain_record` builds any other shape-only record — see
    /// `Connection`/`Server`), binds it to the arm's payload pattern (skipped for `_`),
    /// generates the body, and discards its result — a trap arm returns nothing.
    fn generate_trap_arm_function(
        &mut self,
        arm: &MatchArm,
        symbol: &str,
    ) -> Result<FunctionValue<'ctx>, String> {
        let f64_type = self.context.f64_type();
        let fn_type = self
            .context
            .void_type()
            .fn_type(&[f64_type.into(), f64_type.into()], false);
        let function = self.module.add_function(symbol, fn_type, None);
        function.set_linkage(inkwell::module::Linkage::Internal);

        let suspended = self.suspend_enclosing_function();
        self.current_function = Some(function);
        let saved_scope = self.begin_di_function(function, symbol, &arm.span);

        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);

        self.take_frame(); // fresh frame: the previously emitted function's entries are dead
        self.boxed_vars = self.compute_boxed_vars(&arm.body);

        let pid = function.get_nth_param(0).unwrap().into_float_value();
        let uid = function.get_nth_param(1).unwrap().into_float_value();
        let sender = self.build_plain_record(&[pid.into(), uid.into()])?;

        if let Pattern::Constructor { arguments, .. } = &arm.pattern
            && let Some(Pattern::Identifier { name, span }) = arguments.first()
        {
            let alloca = self.create_entry_block_alloca(name, sender.get_type())?;
            self.builder
                .build_store(alloca, sender)
                .map_err(ctx("Failed to store a trap arm's sender"))?;
            self.variables
                .insert(name.clone(), (alloca, sender.get_type()));
            // The payload's type — always `process.Sender` — was recorded in the type
            // oracle by the CHECKER, keyed by this same pattern span (see
            // `TypeChecker::check_trap_arms`): codegen has no checker `env` of its own to
            // read back from, and this is exactly how a context-inferred parameter's type
            // is recovered elsewhere (`record_parameter_types`).
            if let Some(qty) = self.oracle.type_at(span).cloned() {
                self.var_types.insert(name.clone(), qty.clone());
                self.track_named_record_binding(name, &qty);
                self.declare_variable(name, alloca, &qty, span, None);
            }
        }

        self.generate_expression(&arm.body)?;
        self.builder
            .build_return(None)
            .map_err(ctx("Failed to return from a trap arm"))?;

        self.end_di_scope(saved_scope);
        self.resume_enclosing_function(suspended);
        Ok(function)
    }
}
