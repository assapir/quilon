//! Each signal trap arm lowers to its own top-level function, `void (double pid, double
//! uid)`. Unlike a `net.@tcpServe` handler, a trap arm captures nothing (only the root
//! file may declare one), so its body is generated directly — no closure bundle, no
//! indirect call.
//!
//! Part of the LLVM code generator; see `super` for the `CodeGenerator` state these
//! methods run against.

use super::*;
use crate::ast::TrapDeclaration;

impl<'ctx> CodeGenerator<'ctx> {
    /// The checker already rejected anything but a known `process.Signal` constructor
    /// pattern here, so both `Err`s below are unreachable in a checked program.
    pub(super) fn generate_trap(&mut self, trap: &TrapDeclaration) -> Result<(), String> {
        for (arm_index, arm) in trap.arms.iter().enumerate() {
            let Pattern::Constructor { name, .. } = &arm.pattern else {
                return Err(format!(
                    "internal error: a signal trap arm's pattern must name a process.Signal \
                     variant, got {:?}",
                    arm.pattern
                ));
            };
            // The runtime's own `TRAP_SIGNALS` (quilon-rt/src/trap.rs) follows this same
            // declaration order, pinned by a test against corelib/process.qn.
            let Some((tag, _)) = self
                .sum_variants
                .iter()
                .find(|(qualified, _)| crate::ast::display_name(qualified) == name.as_str())
                .map(|(_, entry)| entry)
            else {
                return Err(format!(
                    "internal error: '{name}' is not a known process.Signal variant"
                ));
            };
            let signal_index = *tag as usize;
            let symbol = format!("__trap_arm_{arm_index}_{name}");
            let function = self.generate_trap_arm_function(arm, &symbol)?;
            self.trap_arms.push((signal_index, function));
        }
        Ok(())
    }

    /// Builds the `Sender` record from the two `Num` parameters, binds it to the arm's
    /// payload pattern (skipped for `_`), then generates and discards the body's result.
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
            // The checker recorded the payload's type in the oracle by this pattern's span.
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
