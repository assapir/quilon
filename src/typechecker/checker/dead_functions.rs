//! Dead top-level functions are compile errors, not warnings.
//!
//! A non-exported top-level function that nothing reachable from `^` calls is never used
//! and never can be from outside its own file: `>>` is a module's only public surface, so
//! an unexported name nothing reaches is dead by construction — `NeverReachable` (QN352).
//! In a module with no `^` (compiled on its own, or reached only through `<<`), the same
//! question is asked against the module's own exports instead: a private helper none of
//! them calls is exactly as dead. Corelib functions are exempt, and a module with no `^`
//! and no export at all is not checked — `ast::reachability::reachable_functions` returns
//! `None` there, the same signal codegen's own tree-shaker reads as "keep everything, a
//! later program may still call any of it".
//!
//! `run`/`build`/`check` erase a program's `test.describe` blocks before compiling it (only
//! `quilon test`'s synthesized `^` runs them — see `driver.rs`). A function this check
//! would otherwise call dead, but that an erased block still mentions, looked alive on the
//! page for a reason: `ReachableOnlyFromTests` (QN353) says so, instead of reporting it as
//! ordinary dead code. The names an erased block would have kept alive are read straight
//! off `program.test_blocks` with the same over-approximate "names mentioned" walk
//! `reachable_functions` itself uses (`ast::reachability::names_mentioned`) — under
//! `quilon test` those blocks are drained into the synthesized `^` before the checker ever
//! runs, so their mentions are ordinary reachable code by then and nothing here fires.

use super::*;

impl TypeChecker {
    /// The first non-exported, non-corelib top-level function `program` declares that
    /// nothing reachable from its roots calls — reported as `NeverReachable`, or
    /// `ReachableOnlyFromTests` when only an erased test block kept it looking alive.
    /// Reports at most one: a run that fixes it and checks again is how the rest surface,
    /// matching every other single-diagnostic-at-a-time pass in this checker.
    pub(super) fn check_dead_functions(&self, program: &Program) -> Result<(), TypeError> {
        let Some(reachable) = crate::ast::reachability::reachable_functions(program) else {
            return Ok(());
        };
        let mentioned_by_tests = crate::ast::reachability::names_mentioned(&program.test_blocks);

        for item in &program.items {
            let Item::FunctionDeclaration(declaration) = item else {
                continue;
            };
            if declaration.exported
                || declaration.from_corelib
                || reachable.contains(declaration.name.as_str())
            {
                continue;
            }
            if mentioned_by_tests.contains(declaration.name.as_str()) {
                return Err(TypeError::ReachableOnlyFromTests {
                    name: declaration.name.clone(),
                    span: declaration.span.clone(),
                });
            }
            return Err(TypeError::NeverReachable {
                name: declaration.name.clone(),
                span: declaration.span.clone(),
            });
        }
        Ok(())
    }
}
