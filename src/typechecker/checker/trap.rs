//! The top-level signal trap (`!>`): only the file that defines `^` may declare one, a
//! program declares at most one, and its arms match `process.Signal` — bare variant names
//! (`Interrupt`, `Terminate`, …) resolve against that sum the way `Ok`/`NotOk` resolve for
//! a `Result`, since `core.process` merges `Signal`'s variants in under their qualified
//! spelling (`core.process.Interrupt`) while a trap arm is written bare.
//!
//! Part of the type checker; see `super` for the `TypeChecker` state these methods run
//! against. Runs after the main item-by-item pass (see `check_program`), once every
//! top-level item — including a merged `<< core.process` — has been checked and
//! registered, exactly like `check_fiber_sharing`/`check_dead_functions`.

use super::*;
use crate::ast::{TrapDeclaration, display_name};
use crate::lexer::ROOT_FILE;

/// `core.process`'s own sum type, once merged in by `<< core.process` — see
/// `corelib/process.qn`.
const PROCESS_SIGNAL: &str = "core.process.Signal";

impl TypeChecker {
    /// Enforce the trap's placement rules (only the root file, at most one per program),
    /// then check the one trap's arms, if any.
    pub(super) fn check_traps(&mut self, program: &Program) -> Result<(), TypeError> {
        let traps: Vec<&TrapDeclaration> = program
            .items
            .iter()
            .filter_map(|item| match item {
                Item::TrapDeclaration(trap) => Some(trap),
                _ => None,
            })
            .collect();

        let Some(first) = traps.first() else {
            return Ok(());
        };

        if let Some(second) = traps.get(1) {
            return Err(TypeError::SecondTrap {
                first: first.span.clone(),
                span: second.span.clone(),
            });
        }

        if first.span.file != ROOT_FILE {
            return Err(TypeError::TrapOutsideRootFile {
                span: first.span.clone(),
            });
        }

        self.check_trap_arms(first)
    }

    /// Check one trap's arms against `process.Signal`: each pattern names one of its
    /// variants (bare — see the module doc), no two arms name the same variant, and each
    /// arm's body is checked with its payload (`Sender`) bound at its real type. No
    /// exhaustiveness requirement: a signal with no arm keeps the OS default.
    fn check_trap_arms(&mut self, trap: &TrapDeclaration) -> Result<(), TypeError> {
        let Some(Type::Sum { variants, .. }) = self.sum_types.get(PROCESS_SIGNAL).cloned() else {
            return Err(TypeError::TrapWithoutProcessImport {
                span: trap.span.clone(),
            });
        };

        let mut seen: std::collections::HashMap<String, Span> = std::collections::HashMap::new();

        for arm in &trap.arms {
            let Pattern::Constructor {
                name, arguments, ..
            } = &arm.pattern
            else {
                return Err(TypeError::TrapArmNotASignalPattern {
                    span: arm.pattern.span().clone(),
                });
            };

            let Some(variant) = variants
                .iter()
                .find(|variant| display_name(&variant.name) == name)
            else {
                return Err(TypeError::UnknownConstructor {
                    constructor: name.clone(),
                    sum: "process.Signal".to_string(),
                    known: variants
                        .iter()
                        .map(|variant| display_name(&variant.name).to_string())
                        .collect(),
                    span: arm.pattern.span().clone(),
                });
            };

            if let Some(first) = seen.insert(name.clone(), arm.pattern.span().clone()) {
                return Err(TypeError::DuplicateTrapArm {
                    variant: name.clone(),
                    first,
                    span: arm.pattern.span().clone(),
                });
            }

            if arguments.len() != variant.fields.len() {
                return Err(TypeError::WrongNumberOfArguments {
                    expected: variant.fields.len(),
                    got: arguments.len(),
                    span: arm.pattern.span().clone(),
                });
            }
            for argument in arguments {
                if !argument.is_irrefutable() {
                    return Err(TypeError::RefutableConstructorArg {
                        constructor: name.clone(),
                        span: argument.span().clone(),
                    });
                }
            }

            self.env.push_scope();
            let enclosing_declaration = self.enter_declaration();
            for (slot, (argument, field_type)) in
                arguments.iter().zip(variant.fields.iter()).enumerate()
            {
                if let Pattern::Identifier { name, span } = argument {
                    // Recorded in the type oracle by the payload's own span — codegen has
                    // no checker `env` to read back from, and this is exactly how a
                    // context-inferred parameter's type is recovered (see
                    // `record_parameter_types`).
                    self.type_table.insert(span.clone(), field_type.clone());
                    self.env.define_parameter(
                        name.clone(),
                        field_type.clone(),
                        self.current_declaration,
                        slot,
                        span.clone(),
                    )?;
                }
            }
            self.infer_expression(&arm.body)?;
            self.leave_declaration(enclosing_declaration);
            self.env.pop_scope();
        }

        Ok(())
    }
}
