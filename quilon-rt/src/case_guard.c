/* SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0 */

/* A case's bail-out point. `ql_case_run` calls a case's body with a nonlocal exit target
 * recorded first, so `ql_case_abort` -- called from wherever a failing `expect` is reached,
 * however deeply nested inside the body's own call tree -- can resume right after that call
 * instead of letting the rest of the body run. setjmp/longjmp is the only tool that can
 * unwind an LLVM-compiled call stack: it carries no Rust unwind tables to walk, and a flag
 * checked between statements cannot reach into a lambda the body has already called into
 * (`.each`, and the like). Real C, not attempted from Rust, because the compiler needs to
 * know a setjmp call may return twice to compile the surrounding code correctly -- a
 * guarantee only a C compiler gives its own setjmp calls.
 *
 * The jump target is thread-local: `quilon test` runs one suite per process, but a case
 * never assumes it is the only thread that could ever run one (the runtime's own unit tests
 * exercise several in sequence on one thread, and a future multithreaded run should not have
 * to revisit this file to stay correct).
 */

#include <setjmp.h>
#include <stddef.h>

typedef unsigned char (*ql_case_body)(void *env);

static __thread jmp_buf *ql_current_case_jmp = NULL;

void ql_case_run(void *body, void *env) {
    ql_case_body run = (ql_case_body)body;
    jmp_buf here;
    jmp_buf *previous = ql_current_case_jmp;
    ql_current_case_jmp = &here;
    if (setjmp(here) == 0) {
        run(env);
    }
    ql_current_case_jmp = previous;
}

/* A no-op if no case is running: `expect` only ever reaches this from inside one (the type
 * checker enforces it), so this is a defensive fallback rather than a path taken in practice.
 */
void ql_case_abort(void) {
    if (ql_current_case_jmp != NULL) {
        longjmp(*ql_current_case_jmp, 1);
    }
}
