//! The top-level signal trap (`!>`): its arms match `process.Signal` by bare variant name
//! (`Interrupt`, `Terminate`, …), the way `Ok`/`NotOk` resolve for a `Result`.
//!
//! Part of the type checker; see `super` for the `TypeChecker` state these methods run
//! against.

use super::*;
use crate::ast::{TrapDeclaration, display_name};
use crate::lexer::ROOT_FILE;

const PROCESS_SIGNAL: &str = "core.process.Signal";

impl TypeChecker {
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

    /// Resolves each arm's bare name to its variant, then reuses `check_pattern`/
    /// `bind_pattern_vars` (arity, refutability, binding) against a pattern rewritten to
    /// the variant's real, qualified name, since those compare names exactly.
    fn check_trap_arms(&mut self, trap: &TrapDeclaration) -> Result<(), TypeError> {
        let Some(signal_type) = self.sum_types.get(PROCESS_SIGNAL).cloned() else {
            return Err(TypeError::TrapWithoutProcessImport {
                span: trap.span.clone(),
            });
        };
        let Type::Sum { variants, .. } = &signal_type else {
            unreachable!("core.process.Signal is always registered as a sum")
        };

        let mut seen: std::collections::HashMap<String, Span> = std::collections::HashMap::new();

        for arm in &trap.arms {
            let Pattern::Constructor {
                name,
                arguments,
                span,
            } = &arm.pattern
            else {
                return Err(TypeError::TrapArmNotASignalPattern {
                    span: arm.pattern.span().clone(),
                });
            };

            if let Some(first) = seen.insert(name.clone(), span.clone()) {
                return Err(TypeError::DuplicateTrapArm {
                    variant: name.clone(),
                    first,
                    span: span.clone(),
                });
            }

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
                    span: span.clone(),
                });
            };
            let resolved = Pattern::Constructor {
                name: variant.name.clone(),
                arguments: arguments.clone(),
                span: span.clone(),
            };

            self.check_pattern(&resolved, &signal_type)?;

            self.env.push_scope();
            let enclosing_declaration = self.enter_declaration();
            self.bind_pattern_vars(&resolved, &signal_type, &ValueAliasing::default())?;
            // Codegen has no checker `env` to read the payload's type back from; recorded
            // here by the pattern's own span, the way a context-inferred parameter's is.
            if let (Some(Pattern::Identifier { span, .. }), Some(field_type)) =
                (arguments.first(), variant.fields.first())
            {
                self.type_table.insert(span.clone(), field_type.clone());
            }
            self.infer_expression(&arm.body)?;
            self.leave_declaration(enclosing_declaration);
            self.env.pop_scope();
        }

        Ok(())
    }
}
