# Quilon — Repo Review Findings

Read-only review of the language design and implementation at commit `b3d20e0` (2026-08-07),
re-audited 2026-08-10 against `fafb6cb`, **re-audited again 2026-08-26 against main `824b51f`
(post-0.9.2 "Hegemon")**. Findings are sorted by the original priority; every finding carries a
status tag. Items marked **[confirmed]** were reproduced live with scratch programs at an audit.

Status legend: **[FIXED]** merged to main · **[TRACKED]** has a GitHub issue · **[PARTIAL]**
meaningfully improved, residue noted · **[OPEN]** unaddressed · **(design)** needs a maintainer
decision before anyone implements.

---

## Re-audit — 2026-08-26: scoreboard and new priorities

Since the 08-10 audit, 0.9.2 shipped ~40 PRs. Newly **FIXED**: 14 (overload members must
annotate their return type — now a documented breaking change), 20 (user `print`/`write`/`now`
definitions are overload members, not silently discarded — #166/#170), 23 (module errors render
against the right file — landed with #152), 26 (top-level non-constant initializers are a clear
error — #139), 50 (dead `if`/`while` tokens removed), 53 (single intrinsic registry — #130),
57 (AOT link is per-platform; macOS works — #185), 60/61 (extension publish fires on
`vscode-v*` tags — 0.9.2 shipped through it; release runs the full gate and auto-generates
notes), 28/64/82 (doc-staleness class — resolved by the CLAUDE.md rewrites and the #165 sweep),
39 (corelib placeholders are provenance-marked and documented — #170), and finding 11's libgc
residual (#49/#181: fully static GC, self-contained binaries).
Newly **PARTIAL**: 17 (interpolation + the render operator make Num→Text routine; Text→Num
parsing still absent), 18 (named-record sum payloads shipped — #156; record-in-record still
deferred), 34 (Map/Set shipped — #160; still no array index-assign/push), 62 (macOS + Arch CI
jobs — #185; `--locked` and the dev-channel LLVM pin remain), 66/67 (shared native harness +
some stdout assertions), 81 (per-file line index killed the O(file) scan — #158).
Reclassified: 63 — released CHANGELOG sections are history by convention; not a defect.

**Update 2026-08-26 (post-#195, merged b7c1639): findings 21, 22, and 31 are [FIXED].**
The fix went further than classification repair — a maintainer design decision (issue #198)
made setters **declared**: `name := (…) => …` marks a mutating record method, and an `=`
method is *verified* non-mutating (compile error pointing at the write, via the shared AST
walk in `src/ast/walk.rs`). `:=` is rejected on sum methods, operator members, and the render
member, with the rule stated in the diagnostic. Finding 31 (setter-ness inferred from the body
= implementation-is-API) is thereby resolved by design. Finding 22's call sites now hold
unannotated method args to the body's `Num` default. Residue tracked: #193 (aliasing escape
routes — deep-immutability decision locked, sub-questions open, do not implement yet) and
#194 (user-typed method params: the named-method call path diverges from the working operator
path — mechanical convergence).

**Every remaining open finding is now [TRACKED] (issues filed 2026-08-26).** The map:
15+41 → #202 · 16 → #203 · 24 (+70) → #204 · 35 → #205 · 45 → #206 · 46 → #207 · 47 → #208 ·
44+49+81 → #209 · 19 → #210 · 25 → #211 · 29 → #212 · 48 → #213 · 30 → #214 · 32 → #215 ·
33 → #216 · 36 → #217 · 37 → #218 · 54 → #219 · 55+59 → #220 · 58 → #221 · 62 → #222 ·
66/67/68/71/72/74 → #223 · 79 → #224 · 80 → #225 · 83 → #226. Already covered elsewhere:
42 + record aliasing → #193 (comment added) · 43 → #191 · 56 → #57 (comment added) ·
Num-default class → #187/#192 · 69 + user-definable iteration → #188/#196 · env → #197.
This file is a historical record; **the tracker is now the source of truth** — GitHub issues,
labeled M4/M7/M8 per the milestone convention.

**Recommended next (in order): #202 (match holes — needs one design answer on
exhaustiveness), then #203 (mechanical), then #204 (canonicalization).**

---

## Re-audit — 2026-08-10: scoreboard and new priorities

**All 11 P0 criticals are fixed and merged**: array-literal lifetime (#66 + #95), per-function
codegen frame (#79), Generic-payload segfault (#53-era concrete payloads), nested literal
patterns (#80), short-circuit `&&`/`||` (#81), `%` (#89), checked indexing (#84), checker
exponential blowup (#86), parser depth (#96), statement fusion incl. `{` (#92), and
distributable `quilon build` (#90). Also landed since the review: self-asserting examples
(#91, closes the old #56), JIT/AOT argv parity (#46), the quilon-rt runtime-library
exception (#94), the watermark (#97), and Text methods / core.cli / core.test.

### Remaining work, re-prioritized (recommendation — design items need your call first)

**Correctness, unfixed and untracked — new P0:**
1. **Finding 12** — codegen re-derives types with a `Num`-defaulting `infer_type` and diverges
   from the checker: internal verifier errors on valid overload calls, and a mis-classified
   tail call that crashed a valid program. The biggest remaining silent-failure class; fix
   direction (consult the oracle first) is mechanical, no design needed.
2. **Finding 14** — overload members' provisional `Num` return: `check` passes, `run` fails,
   definition-order-dependent.
3. **Finding 21** — immutability bypass: mutation inside a lambda in a method body isn't
   detected as a setter; `=`-bound receivers get mutated.
4. **Finding 22** — unannotated method params unchecked at call sites → leaked LLVM dumps.
5. **Finding 26** — top-level non-constant initializers emit invalid IR.
6. **Finding 15** — non-sum match fall-through loads undef. (Emitting an abort on the
   fall-through edge is a safe fix; *requiring* exhaustiveness for non-sum scrutinees is a
   language decision — ask first.)
7. **Finding 41** — unknown constructors / constructor patterns on non-sum scrutinees are
   accepted by the checker, then die at runtime.
8. **Finding 16** — parser lookahead caps (50/80/40 tokens) silently change what a definition
   means at ~17+ typed params.
9. **Findings 23 + 24** — module-system correctness: errors rendered against the wrong file;
   no path canonicalization (diamond imports duplicate definitions, root cycles undetected).
10. **Issue #98** (new, not from this review) — flaky GC SIGABRT under parallel `cargo test`.

**Quick doc fixes (an hour, no decisions):** finding 28 (CLAUDE.md still denies working
Text-in-composites — verified still stale), 64 (LANGUAGE.md still claims `.size` needs a named
receiver), 61 (release workflow still tests nothing and its notes template still advertises the
removed `for` loop), 63 (0.9.0 "Known limitations" block).

**Design queue — maintainer decisions, roughly by leverage:**
finding 17 (Num↔Text conversion — the single biggest usability wall; adjacent #88/#57),
20 (user `print` overloads silently discarded — honor them or reject them?),
43 (empty array literal is hard-`[]Num` — should the annotation seed it?),
19 (`:=` conflates declare/reassign), 25 (exit-code semantics: NaN/±inf UB edges, 8-bit mask),
35 (Text/Bool/negative literals in patterns), 47 (scientific notation), 18 (recursive/composite
payload types), 29–37, 48, 50 (`if`/`while` reserved tokens).

**Already tracked elsewhere — don't duplicate:** findings 27/59 → #83 (uniform Result layout);
53, 73, 75–77 → #87 (structural refactors) + #54 (split quilon-rt); libgc residual of 11 → #49;
finding 60 (VS Code tag trigger) → verify inside open PR #99.

---

## P0 — Critical: silent wrong results, memory corruption, or crashes in valid programs

### 1. Array literals are stack-allocated → silent memory corruption when they escape, and TCO stack overflow **[confirmed]**
**Status: [FIXED] — #66 (heap-allocated array literals) + #95 (literals/ranges inside TCO loops)**
Plain array literals get a raw `build_alloca` at the current insert point (`src/codegen/generator.rs:3014-3019`),
unlike ranges/spreads/records/method results which all GC-allocate via `__alloc`.
Two confirmed failures:
- **Dangling escape:** `Pair = { xs :: []Num }` + `make = () -> Pair => Pair { xs = [10,20,30] }` —
  after another call, `p.xs[0]` returned **77 instead of 10**. Same hazard for arrays captured into escaping closures.

- **Alloca-in-loop:** an array literal inside a tail-call-lowered body re-allocas every iteration —
  a valid self-tail-recursive function **aborted with stack overflow at depth 1,000,000**, breaking the
  constant-stack guarantee `tests/tail_call_test.rs` documents (its cases just happen to contain no array literals).

### 2. Per-function codegen maps are never cleared between functions → later functions silently miscompile **[confirmed]**
**Status: [FIXED] — #79 (per-function FrameState, captures carry metadata)**
`emit_module_function` clears only 4 of the ~10 per-function state maps (`generator.rs:1025-1029`);
`record_types` (:145) and `var_named_types` (:149) accumulate module-wide, `generate_method` (:746-748)
never clears `var_types`, and `emit_lambda_function` (:1908-1912) saves/restores everything except `var_types`.
Confirmed: after one function binds a record to `p`, a later `second = (p :: []Num) -> Num => p.size`
returned **1 instead of 3** — the stale entry diverted `.size` to the record-field path. The same mechanism
can mis-route method dispatch and overload mangling. Suggested shape: one `FunctionFrame` pushed/popped in one place.

### 3. `Generic` sum-payload wildcard causes runtime type confusion → segfault on checked code **[confirmed]**
**Status: [FIXED] — concrete Result payload typing (#53 era); the original repro is rejected at check. Residual composite-layout crash → tracked as #83**
`types_compatible` treats `Type::Generic` as compatible with anything (`src/typechecker/checker.rs:2298-2301`)
and `check_match` upgrades a Generic arm to any sibling arm's concrete type (:2139-2149). A value typed via a
`-> Result` annotation keeps generic payloads, so a `Num` payload can be statically typed `Text`:
`r ? | Ok(x) => "positive" | NotOk(e) => e` then `s + "!"` → **exit 139 (SIGSEGV)**, with Boehm GC trying to
expand the heap by 137 TB (f64 bits read as a Text `{ptr,len}`). Also lets `Ok(<lambda>)`, `Ok([1,2,3])` through.

### 4. Nested literal patterns in constructor arms are silently ignored → wrong arm taken **[confirmed]**
**Status: [FIXED] — #80 (refutable constructor sub-patterns rejected)**
The checker accepts `Ok(1)` and counts it as covering all of `Ok` (`checker.rs:2170-2172, 2219-2222`);
codegen matches on the tag only, discarding the subpattern (`generator.rs:4778`).
Confirmed: `Ok(2) ? | Ok(1) => 10 | Ok(2) => 20 | ...` → **exit 10** (first `Ok` arm always wins). No diagnostic.

### 5. `&&` / `||` do not short-circuit, contradicting LANGUAGE.md **[confirmed]**
**Status: [FIXED] — #81 (branch+phi short-circuit lowering)**
Both operands are generated eagerly, then bitwise `build_and`/`build_or` (`generator.rs:2296-2321` — the code
comments even claim "with short-circuit evaluation"). `LANGUAGE.md:404` promises short-circuit.
Confirmed: `false && f(1)` runs `f`'s side effects. Compounds with #7 below: the guard idiom
`i < a.size && a[i] == k` **always** evaluates the index → guarded access is still an OOB read. No test covers short-circuiting.

### 6. `%` (modulo) is documented ✅ but has no codegen — type-checks, then dies with an internal error **[confirmed]**
**Status: [FIXED] — #89 (frem; fractional + dividend-sign tests)**
`LANGUAGE.md:402,377-378,646` list `%` as working; the checker accepts `Mod` (`checker.rs:526`); `generate_binop`
has **no `Mod` arm** and falls to `Err("Unsupported binary operation")` (`generator.rs:2322`).
Confirmed: `7 % 3` → `❌ Runtime error: Unsupported binary operation: Mod`. Zero tests/examples use `%` —
`examples/tail_recursion.ql:7-8` is explicitly engineered to avoid needing it.

### 7. Raw indexing `arr[i]` is completely unchecked: OOB reads garbage, fractional indices truncate, NaN is LLVM poison **[confirmed]**
**Status: [FIXED] — #84 (bounds/NaN checked before conversion; __index_fail abort; .at poison-free)**
`generate_index` (`generator.rs:4626-4649`): unguarded `fptosi` then raw GEP+load — no bounds/negative/NaN check,
even though the size field is in the same struct. Confirmed: `a[10]` and `a[0-1]` silently return garbage;
`a[1.7]` reads `a[1]`. Even `.at(n)` does its compare **after** an unguarded `fptosi` (:3840-3845), so `at(0/0)`
branches on poison. Everything is masked today only because both JIT and build run at `OptimizationLevel::None`
(`jit.rs:44`, `build.rs:45`) — enabling optimization without guards first turns these into real miscompiles.

### 8. Exponential argument re-inference in `check_call` — the compiler hangs on ~25-stage pipelines **[confirmed]**
**Status: [FIXED] — #86 (first argument inferred exactly once; 60-deep chains check in ms)**
`check_call` infers `args[0]` in the method-dispatch probe (`checker.rs:1877-1880`), then re-infers **all** args in
the overload (:1948-1951) or fallback (:1971-1975) branch → 2^depth work. Measured: 22 nested calls = **6.8 s**;
26 levels **timed out (>2 min)**. `|>` desugars to exactly first-arg-nested calls (and `desugar_pipeline` clones the
whole left subtree per stage, `nodes.rs:328-339`), so an idiomatic long pipeline — in a language whose loop
replacement *is* pipelines — hangs `quilon check`. Fix direction: memoize via the type table (spans are unique keys).

### 9. Parser stack overflow (SIGABRT, core dump) on ~2000 nested parens — no recursion depth limit **[confirmed]**
**Status: [FIXED] — #96 (growable stack + configurable cap, QUILON_MAX_PARSE_DEPTH)**
The ~12-level `parse_expr` → … → `parse_primary` chain (`src/parser/ast_parser.rs:572-1186`) has no depth guard.
A 4 KB file of nested parens aborts the compiler with a core dump instead of a diagnostic. Fix is a simple depth counter.

### 10. No statement separator + greedy postfix parsing silently fuses adjacent statements **[confirmed]**
**Status: [FIXED] — #92 (line-first `(` / `[` / `{` begins a new statement)**
`parse_postfix` treats any following `(` as a call (:955-976) and `[` as an index (:944); blocks have no line-boundary
notion (:534). Confirmed: `x = f()` followed by `(1 + 2) |> print` on the next line parses as `f()(1+2)` — error
reported at the **wrong line**; `b = a` then `[3, 4].each(...)` parses as `a[3, 4]`. If arities/types happen to line up,
this **silently compiles the wrong program** (the JS-ASI hazard class, with no `;` escape hatch).

### 11. Released binaries and `cargo install` builds cannot run `quilon build` at all
**Status: [FIXED] — #90 (gzip-embedded runtime, cache extraction, system-archive preference). libgc residual → #49 [FIXED by #181: fully static, vendored-submodule GC]**
`release.yml:39-45` ships only `target/release/quilon`; `src/build.rs:66-93` finds `libquilon_rt.a` via a path
**baked at CI build time** or next to the binary — both fail on a user's machine, and the error tells them to
`cargo build --release`, impossible from a binary download. Same end state for `cargo install --path .`
(staging target dir is deleted; nothing installs the `.a`). One of the two headline execution paths is dead in
every distributed binary. Compounds open issue **#49** (static libgc) — even shipping the `.a` still needs system libgc.

---

## P1 — High: broken or misleading core behavior, checked-then-crash divergences, major design walls

### 12. Codegen re-derives types with a `Num`-defaulting `infer_type` and diverges from the checker **[confirmed]**
**Status: [FIXED] — symptoms (a) and (b) fixed earlier by the oracle-first lookup in `infer_type` (verified on fafb6cb:
both now exit 2); the live remainder was the oracle's span keying — finding 52, fixed in PR #105 together with the
unchecked `emit_tail_self_call` store (a type-mismatched arg now emits an ordinary call instead of the loop back-edge).
Example coverage: `examples/overload_dispatch.ql`. The premise below (oracle consulted only for `Record`/`Spread`) is
stale as of the oracle-first change.**
`generator.rs:4984-5069` falls back to `Type::Num` for `Index`/`Match`/`Array`/lambdas even though the authoritative
`self.oracle.expr_type` sits on the same struct (consulted only for `Record`/`Spread`). Confirmed symptoms:
(a) `f(a[0])` with `a :: []Text` + `f(Num)/f(Text)` overloads → internal **"Module verification failed"**;
(b) the same misinference inside `is_self_tail_call` (:1184-1193) misclassified a call to a *different* overload
member as a self-call → checker-valid program **crashed with stack overflow** instead of exiting 2.
`emit_tail_self_call` (:1272-1294) also stores args into param slots with no type check (silent stack corruption path).
Consulting the oracle first eliminates the whole class.

### 13. Functions cannot return arrays — return-type lowering disagrees with the value representation **[confirmed]**
**Status: [FIXED] — #66 (bare-array returns lowered via value repr)**
Array params are special-cased to `{ptr,i64}` (`generator.rs:963-972`) but return types go through `type_to_llvm`,
which lowers `Type::Array` to a bare `ptr` (:5126-5131). `make = () -> []Num => [10,20,30]` fails with the misleading
"Can only index into arrays". Closure signatures (:1770-1776) have the same bug for array params/returns.

### 14. Overload members' unannotated return type is provisionally `Num` — `check` passes, `run` fails **[confirmed]**
**Status: [FIXED] — breaking change: every overload member must annotate its return type (shipped in 0.9.2)**
`register_overload_decl` registers return = annotation **or `Num`** (`checker.rs:886-890`), refined only after the body
is checked (:1309). A call checked before the definition resolves against the stale `Num`:
`quilon check` passes, `quilon run` → `❌ Runtime error: Overload not found: g$N`. Definition-order-dependent semantics
and a checker/codegen contract break.

### 15. Match on non-sum scrutinees: no exhaustiveness check, fall-through loads an uninitialized slot **[confirmed]**
**Status: [OPEN] — re-audit priority (abort edge = safe fix; exhaustiveness requirement = design). Re-confirmed live 2026-08-26.**
Exhaustiveness is only checked for `Type::Sum` (`checker.rs:2114-2117`); `5 ? | 0 => 1 | 1 => 2` type-checks.
Codegen's no-match edge branches to a continuation that loads the never-stored `match_result` alloca
(`generator.rs:~4708-4714`) — undef, not a guaranteed value. An `unreachable`/abort on that edge (as
`generate_tail_match` already emits, :1444-1449) would be cheap insurance either way.

### 16. Hard-coded parser lookahead caps (50/80/40 tokens) silently change what a definition means **[confirmed]**
**Status: [OPEN] — re-confirmed 2026-08-26 (`ast_parser/items.rs:206`, `lookahead.rs:107,139`)**
`ast_parser.rs:206` (`idx < 50`), `:1231` (`< 80`), `:1203` (`< 40`), `:150` (`< 10`). A function with 17 typed params
stops being a `FunctionDecl` and becomes a `VarDecl` holding a lambda → its own recursive call fails with
`Undefined variable 'f'`. 30 typed params → `Expected ParenClose, got TypeAnnotation` at the first `::`.
The scans already track paren depth — the caps are unnecessary.

### 17. No Num↔Text conversion anywhere — impossible to build a message containing a number
**Status: [PARTIAL] — string interpolation + the `` ` `` render operator shipped (Num→Text is routine, `print` renders any type). Text→Num parsing still absent (numeric CLI args can't become `Num`); adjacent #88/#57.**
No `show`/`toText`/`format`/`parse` exists; interpolation is ❌ (`LANGUAGE.md:677`); `+` dispatches by exact type
(`(Num,Num)`/`(Text,Text)` only). `"count = " + n` is a compile error with **no workaround**, and numeric CLI args
(`args :: []Text`) can't become `Num` at all. Real programs are impractical. Adjacent to #57 and #50 but covered by neither.

### 18. No recursive or composite-payload data types — no trees, no structured errors
**Status: [PARTIAL] (design) — named records as sum-variant payloads shipped (#156); a record field of a named composite, and recursive types, remain deferred.**
Sum payloads are built-in scalars only (`LANGUAGE.md:158-164`); no records/user types as payloads, no generics,
no self-referencing records. The data-modeling ceiling is flat records of scalars plus arrays: no AST, no JSON-shaped
value, no linked structure — and `NotOk` can't carry a structured error. The single biggest scalability wall in the design.

### 19. `:=` conflates declaration and reassignment — a typo silently creates a fresh binding
**Status: [OPEN] (design)**
`LANGUAGE.md:208-214`. `countr := counter + 1` is not an error. Compounds with closure capture (finding 33):
which `:=` first introduced a name silently determines capture semantics.

### 20. User `print`/`eprint` overloads type-check but are silently discarded
**Status: [FIXED] — #166/#170: built-ins are overload members (BUILTIN_OVERLOADS table); a user definition adds a member, under-annotated definitions are reported, and `write`/`now`/`__color_enabled` had the same hole closed.**
Documented at `LANGUAGE.md:687` vs the advertised "a user definition adds an overload" (:542). Worse than the doc
suggests: `is_inert_io_placeholder` (`src/ast/nodes.rs:104-108`) silently drops **any** user top-level
`print = x => ...` with one unannotated param, even outside `core.io` — the definition just vanishes, no diagnostic.

### 21. Immutability bypass: mutation inside a lambda in a method body is not detected as a setter **[confirmed]**
**Status: [FIXED] — PR #195 (with the #198 design decision): setters are now DECLARED (`name := (…) => …`), and `=` methods are VERIFIED non-mutating via the shared exhaustive AST walk (`src/ast/walk.rs`) — mutation anywhere in the body (lambda, declared function, any nesting) is a compile error pointing at the write. Aliasing routes remain → #193 (deep immutability, decided, not yet implemented).**
`body_mutates_receiver` (`checker.rs:1126-1179`) has no arms for `Lambda`/`Array`/`Record`/`Index`/`FieldAccess`/`Spread`
(fall to `false`). A method whose body does `[1,2,3].each(x => it.v := x)` is callable on an `=`-bound receiver and
**mutates the immutable binding** (confirmed, exit 3). Undermines the language's central immutability promise.

### 22. Method arguments with unannotated params are not checked at the call site **[confirmed]**
**Status: [FIXED] — PR #195: call sites hold arguments to the same `Num` default the body was checked with (consistent with plain functions). User-typed method params' path divergence → #194.**
Call site: "If no type annotation, we can't check" (`checker.rs:1929-1933`) — but the body was checked with those
params defaulted to `Num` (:1047). `t.add("hello")` on `add = (x) => it.v + x` type-checks, then leaks a raw LLVM
verifier dump. The definition-time `Num` default should be enforced at the call site.

### 23. Type errors inside imported modules render against the wrong source file
**Status: [FIXED] — landed with the #152-era SourceMap work; `tests/diagnostics_test.rs` covers a compile error inside an imported module rendering against the module's own file/line/excerpt.**
`driver.rs:59-63` renders post-merge checker errors against the **root** file, but merged items carry spans into the
*module's* source (`src/modules.rs:88-99`). A type error in `mathlib.ql` shows `main.ql:3:7` with a caret under
unrelated text (or the snippet silently dropped when out of range, `diagnostic.rs:42`). Lex/parse errors in modules
fall back to span-less plain strings (`modules.rs:88-91`).

### 24. Module resolution: no path canonicalization, and the root file is never in the visited set
**Status: [OPEN] — re-confirmed 2026-08-26 (no `canonicalize` in modules.rs)**
Dedup key is the literal joined path string, not `fs::canonicalize` (`modules.rs:76-84`); the root is never added to
`visited` (:27-34). Diamond imports via different spellings (or symlinks / macOS case-insensitivity) merge a module
**twice** → bogus duplicate-definition errors; a cycle back to the root duplicates every exported root item with no
cycle diagnostic ever reported.

### 25. `^` exit-code semantics are lossy with UB edges — and the exit code is the language's primary observable
**Status: [PARTIAL] — #91 removed the exit-code testing culture (examples self-assert, exit 0); the fptosi UB edges and 8-bit masking remain (design)**
`generator.rs:610-621`: f64→i32 `fptosi`, so NaN/±inf (`0/0` — `fdiv` has no zero check) are poison → UB. In-range
values are masked to 8 bits by the OS (`^ => 256` exits 0; `-1` exits 255); a non-`Num` body exits 0, so a `Bool`
entry returning `false` "succeeds". The docs/examples/test culture leans on this 8-bit lossy channel as the result
channel (examples hand-tuned to stay under 251). Open issue **#56** (self-asserting examples) mitigates the culture, not the semantics.

### 26. Top-level bindings with non-constant initializers produce invalid IR **[confirmed]**
**Status: [FIXED] — #139: a top-level binding that has to be computed is a clear, located compile error naming the supported forms.**
`generator.rs:885-891` generates the initializer with `current_function == None` and the builder still positioned in
the previously emitted function. `x = f()` at top level (type-checks) → **"Basic Block in function 'f' does not have
terminator"**. Needs a proper diagnostic or an init function. (Note for the fix: JIT'd globals holding GC pointers
would also be un-rooted under Boehm.)

### 27. Heterogeneous `Result` payloads pass the checker, then crash codegen with a PHI verifier error **[confirmed]**
**Status: [FIXED] — uniform/canonical Result layout shipped (a `Result` of any payload flows through a generic `(r :: Result)` param/return).**
Result values are sized per construction site (`generator.rs:313-320, 2801-2829`); `c ? Ok(1) : NotOk("e")` produces
`{i8,double}` vs `{i8,{ptr,i64}}` and `generate_if`'s phi (:2939-2945) dies. The checker accepted it. Interacts with #57.

### 28. CLAUDE.md flatly contradicts LANGUAGE.md and the tests on Text-in-composites
**Status: [FIXED] — CLAUDE.md has been rewritten since (the stale claim is gone).**
`CLAUDE.md:63` says Text in composites "doesn't type-check yet"; `LANGUAGE.md:671` marks it ✅ and
`tests/composite_text_test.rs:33-224` proves it end-to-end (independently re-verified live). Every future agent
reading CLAUDE.md will avoid or "re-implement" a working feature. The *real* remaining hole is finding 3, which is undocumented.

---

## P2 — Medium: design debts, wrong-file/wrong-line diagnostics, silent inconsistencies, infra risk

### Language design
29. [OPEN] (design) **The `>` line-final rule makes trailing comments/whitespace semantically significant** — `> ~ end` is a
    greater-than operator, so you cannot comment a block-close line (`src/lexer/lexer.rs:63-79`; admitted in-code
    at :66-67 but undocumented in `LANGUAGE.md:406-412`). Line-wrapping `a >` silently closes a block; errors then
    surface **lines away** [confirmed]. Two blocks can't close on one line (`> >` → `Gt`; `>>` lexes as `Export`).
    The parser could at least special-case "BlockClose with no open block / Gt with one" to point at the actual `>`.
30. [OPEN] (design) **The symbol namespace is nearly exhausted and already triple-loaded** — `<-` is range AND spread (and was the
    `for` binder within one release cycle); `/` is division vs variant separator disambiguated by identifier
    *capitalization* (`LANGUAGE.md:196-203`); `?` opens both ternary and match; `$` occupies the natural interpolation
    sigil while interpolation is planned; `~`/`<<`/`>>`/`^` block bitwise-not/shifts/xor. Each new feature needs a
    bespoke disambiguation rule (three exist already), and nothing is greppable.
31. [FIXED — PR #195 / #198: setter-ness is now DECLARED with `:=` on the method; the body is verified against the declaration, so the implementation is no longer the API] (design) **Setter-ness is inferred from the method body** (`LANGUAGE.md:147-149`) — adding a memo-cache write to a getter
    silently reclassifies it and breaks every `=`-receiver call site, transitively ("calls another setter"). The
    implementation is part of the API. (And the inference itself has holes — finding 21.)
32. [PARTIAL — #95 fixed the TCO-loop overflow; eager O(n) materialization remains] (design) **Ranges are eagerly materialized arrays with `fptosi`-truncated endpoints** (`generator.rs:3228-3310`) —
    `1 <- 1e8` allocates 800 MB to count to 100M; `1.5 <- 3.9` silently becomes `[1,2,3]`; a NaN endpoint is poison
    feeding a malloc size. With no loop construct, "do N times" has no scalable encoding.
33. [OPEN] (design) **Closure capture mode is decided by the distant binding operator** (`=` snapshot vs `:=` shared cell,
    `LANGUAGE.md:296-307`) — flipping a binding for a local reason retroactively changes every closure over it;
    no per-closure choice, no way to snapshot a `:=` var. Also directly undercuts the README's deep-immutability pillar.
    (The #193 deep-immutability decision will interact with this.)
34. [PARTIAL — built-in Map/Set shipped (#160); no array index-assign/push yet] (design) **Arrays are less mutable than records, and incremental building is O(n²) or impossible** — no index-assign node
    in the AST, no `push`/`set`; a `:=` record mutates in place but a `:=` array can't be written to; collections of
    unknown size have no good encoding.
35. [OPEN — re-confirmed 2026-08-26] (design) **Pattern matching can't match Text/Bool/negative-number literals; no guards or nested destructuring**
    (`nodes.rs:350-367`; `parse_pattern` rejects `-1`/`true`/`"a"` [confirmed]). Dispatching on `args` subcommands —
    the canonical use of the new args feature — can't use the flagship `?`/`|` construct.
36. [OPEN] (design) **No NaN/infinity/div-by-zero story anywhere in the spec** for a language whose only number is f64 — `NaN == NaN`
    is false (OEQ), NaN matches no pattern, and NaN flows to poison via index/range/exit sinks. Integer identity
    above 2^53 (sizes/indices are trusted f64s) also unaddressed.
37. [OPEN] (design) **Variant names are globally unique with no namespacing** (`LANGUAGE.md:166-167`) + flat imports — two libraries
    with a `Pending` variant can never be imported together.
38. [FIXED — concrete payload typing; the stale LANGUAGE.md note is gone] **Generic Result payloads silently dispatch to the Num overload member** (`LANGUAGE.md:685`) — `Ok("hi")` bound
    then printed calls `print(Num)` on a Text. The two flagship features miscompose silently. (Same root as finding 3.)
39. [FIXED — #170: corelib placeholders are provenance-marked (module loader / front end), documented in the file headers, and a user's same-named definition is a real overload member] **corelib/io.ql contradicts LANGUAGE.md and the language's own rules** — `LANGUAGE.md:530` says its "members are
    real functions"; `corelib/io.ql:23` says they're inert placeholders; `>> print = x -> $ => $` violates the
    all-params-annotated overload rule. The one stdlib file couldn't be written by a user. If lowering ever broke,
    placeholder bodies would make prints silent no-ops — and every exit-code test would still pass (nothing asserts output).

### Type checker
40. [OPEN] **Nested named record types are unusable** — field annotations are stored unresolved (`checker.rs:1023-1089`),
    so `P = { q :: Q }` fails to construct with a Debug-dump mismatch [confirmed]. Root inconsistency: overload
    dispatch compares `Named` **by name** (:299-303) while general compatibility compares **structurally** (:2326).
    (Adjacent: #194 — user-typed method params.)
41. [OPEN — with finding 15] **Unknown constructors / constructor patterns on non-sum scrutinees are accepted "for now"** (`checker.rs:2225-2235`)
    → pass `check`, die at JIT with `Unknown constructor` [confirmed]. One hole is *pinned as passing* by the unit test
    `test_sum_type_option` (`checker.rs:2497-2507`).
42. [OPEN] **`FieldAssign` rooted at a call skips the immutability gate** (`checker.rs:1183-1189`, `None` root = allowed) —
    `id(t).v := 5` passes the checker, currently caught by a codegen error; a silent bypass the day codegen learns it.
    (Related: #193's escape/alias analysis.)
43. [OPEN — re-confirmed 2026-08-26] (design-lite: should the annotation seed the element type?) **Empty array literal is hard-typed `[]Num`** (`checker.rs:1598-1601`) — `xs :: []Text = []` is rejected; no way
    to start a non-Num accumulator. The annotation should seed the element type.
44. [OPEN] **First-error-only** — the whole checker is `?`-threaded (`checker.rs:777-832`); one diagnostic per compile.

### Parser / lexer
45. [OPEN — re-confirmed 2026-08-26] **Lowercase binding to `{ … }` can silently become a type declaration** — the type-decl heuristic
    (`ast_parser.rs:141-178`) never checks capitalization (unlike the sum-type path): `x = { f = => 1 }` parses and
    checks as a `TypeDecl` named `x` [confirmed].
46. [OPEN] **Unterminated strings swallow the rest of the file** — the string regex matches newlines (`token.rs:75`), so the
    error lands far away or the diagnostic dumps the entire remaining source as "Invalid token" [confirmed].
47. [OPEN — re-confirmed 2026-08-26] (design) **No scientific notation** — `1e9` lexes as `1` + identifier `e9` → misleading `Undefined variable 'e9'` [confirmed].
48. [OPEN] (design) **Nested match dangling-arm ambiguity** — all following `|` arms bind to the innermost match with no warning;
    an unreachable inner wildcard + a one-arm outer match type-check silently [confirmed]. Parenthesizing works but is undocumented.
49. [OPEN] **Parse errors show Rust Debug token names** (`Expected ParenClose, got TypeAnnotation`) — every error site uses
    `{:?}` while the human-friendly `Display` impl (`token.rs:254-305`) is dead code. Lex+parse are strictly
    bail-on-first-error, no recovery.
50. [FIXED — the dead `if`/`while` tokens were removed in the #87-era cleanup] (design) **`if`/`while` are reserved dead tokens** (`token.rs:85-90`, never matched by the parser) — `if = 5` →
    `Unexpected token: If`, contradicting the locked "no keywords" surface (`for` was deliberately de-reserved).

### Codegen / runtime / build
51. [FIXED — #46 JIT/AOT argv parity] **`quilon run` and `quilon build` disagree about `args`** — the JIT passes the *compiler's* argv
    (`jit.rs:100-105`; `main.rs:27-30` can't forward program args): `args.size` is 3 under `run`, 1 as a binary;
    `args[0]` is the quilon binary, contradicting `LANGUAGE.md:570-572` [confirmed]. Should be settled alongside #50/#60.
52. [FIXED — PR #105, converged with 12] **Oracle span collisions across modules** — `Span` had no file identity;
    imported modules restart offsets at 0, so `TypeTable` entries collided, last-wins. `Span` now carries the id of the
    file it indexes into (lexer stamps tokens, parser stamps composed spans, module loader assigns per-module ids);
    offsets are `u32`. Regression test: `run_test.rs::importer_expression_on_a_modules_byte_range_does_not_retype_it`.
53. [FIXED — #130: one `intrinsic_registry!` table in quilon-rt, `#[used]` retention, parity + link gates] **The intrinsic registry is hand-duplicated in three places** — quilon-rt's `#[used]` table
    (`quilon-rt/src/lib.rs:345-360`), the JIT mapping (`jit.rs:55-87`), and codegen's `get_intrinsic`
    (`generator.rs:2400-2443`). A miss = **null-pointer segfault under `quilon run` with no diagnostic**.
54. [OPEN — design with #60] **`write_to_fd` swallows all write errors and mishandles EINTR** (`quilon-rt/src/lib.rs:163-166` — `n <= 0 → break`,
    no retry): output silently truncated under signals/EAGAIN/EPIPE; `print` intrinsics discard the return entirely.
    Design the errno story together with #60 (input).
55. [OPEN] **Text NUL-termination is an unchecked cross-component invariant** — `__print_text_fd` requires a NUL the `{ptr,len}`
    type doesn't encode; every producer maintains it ad hoc. Any future producer that forgets it
    reads OOB far from the cause. Passing `len` (like `__write_bytes` already does) deletes the invariant. Also:
    `print(t)` lossy-rewrites invalid UTF-8 while `write` passes it through — same bytes, different output.
56. [OPEN — design input for #57] **`.length` is an O(n) grapheme re-walk per call** with a full copy on invalid UTF-8 —
    `.length` in a recursion bound is quadratic. Design input for #57.
57. [FIXED — #185: the AOT link line is per-platform (`-Wl,-force_load` on macOS, the GNU bracket elsewhere; no `-ldl` on macOS), and macOS CI covers it] **AOT link flags are GNU/Linux-only** — `-Wl,--whole-archive` + `-lpthread -ldl` (`src/build.rs:115-121`);
    on macOS the link dies with an opaque ld64 error. Undocumented, and structurally undetectable in CI (single-OS matrix).
58. [OPEN] **`quilon build` stages the object next to the output and deletes it** (`build.rs:98,136`) —
    `-o prog` in a dir with a preexisting `prog.o` silently overwrites **then deletes** the user's file; concurrent
    builds race. Should use a temp dir. (Adjacent: #182 — two producers write the canonical archive path.)
59. [PARTIAL — the Result-layout half is fixed (see 27); binop shape-dispatch remains] **Non-exhaustive-match fall-through defense** — see finding 15; also binop dispatch keys on LLVM value *shape*
    (any struct == struct → `__text_cmp`, `generator.rs:2171-2190`) and sum-payload slots use `type_to_llvm` not the
    value repr (:668-673): both latent type-confusion holes currently shielded by the checker, waiting for #57-era changes.

### CI / release / tracker hygiene
60. [FIXED — the `vscode-v*` tag path shipped the 0.9.1 and 0.9.2 extension releases end to end] **The VS Code publish job likely never fires on tag pushes** — `vscode-extension.yml:20-31` ANDs a `paths` filter
    with the tag trigger; a tag on an existing commit has an empty changed-file set → workflow skipped silently.
61. [FIXED — `scripts/release.sh` runs the full gate before tagging; release notes are auto-generated from merged PRs (no stale template)] **Release workflow runs no tests and its notes template is wrong** — `release.yml:39-40` publishes without testing;
    the body (:55-63) advertises the **removed** `for` loop and calls argv a placeholder though real args/env shipped.
62. [PARTIAL — #185 added macOS + Arch-container CI jobs (non-blocking) alongside the Ubuntu gate; `--locked` and the apt.llvm.org dev-channel pin remain] **CI: single OS, moving-target LLVM channel, no `--locked`** — `ubuntu-latest` only; `llvm.sh` fetched from the
    apt.llvm.org dev channel every run while `LLVM_SYS_221_PREFIX` hard-pins 22.1 (breaks on upstream roll); dead
    `llc` symlink step contradicting CLAUDE.md; lockfile drift invisible.
63. [RECLASSIFIED — released CHANGELOG sections are history by convention; the Unreleased section is maintained. Not a defect.] **CHANGELOG is badly stale** — "Unreleased" omits array methods, args/env, ranges, spread, TCO; the 0.9.0
    "Known limitations" (:63-70) still deny closures/user sum types/`$`/argv — all long since implemented.
64. [FIXED — the #165 docs sweep removed the stale claim] **Stale documented limitation:** `.size` on literal/expression receivers works (verified `[1,2,3].size` → 3) but
    `LANGUAGE.md:686,33-34` says it doesn't — and no test pins the working behavior against regression.
65. [FIXED — #64 closed as duplicate] **Issues #64 and #65 are exact duplicates** (same title and body) — close one.

---

## P3 — Low: coverage gaps, dead code, papercuts

### Test suite
66. [PARTIAL — every example self-asserts in-language (#91); some Rust tests now assert stdout (corelib-overload native test, read_stdin, fail-loud location tests); broad output assertion still thin] **Program output is almost never asserted** — exit codes only; the single stdout assertion in the whole suite is
    `args_native_test.rs:129-133`. Num formatting, Bool→`true`/`false`, trailing newline, and *that eprint goes to
    stderr* are unverified; all I/O could silently break with green CI.
67. [PARTIAL — `tests/common` gained `build_and_run_native`; several suites now exercise native AOT (fail-loud locations, corelib overloads, index checks)] **Feature tests are JIT-only** — native AOT is exercised solely through the examples table + two harness tests;
    any AOT-specific divergence outside example shapes is invisible.
68. [PARTIAL — statement-boundary and depth-cap work added negative parser tests; no fuzzing] **Zero negative tests in lexer/parser** — every test asserts `is_ok()`; nothing pins error messages, spans, the
    `>`-reclassification edges, lookahead caps, or nesting depth. No property/fuzz testing anywhere despite the
    deliberately tricky disambiguation rules.
69. [OPEN] **Documented closure rejections untested** — "rejected at compile time, never miscompiled" (`LANGUAGE.md:689`)
    has no regression net for closure-as-param or closure-return. (PR #196 — first-class higher-order functions — may
    supersede this whole item.)
70. [OPEN] **Module tests cover one fixture, one level** — no module-imports-module, no `a ⇄ b` cycle test (see finding 24),
    no duplicate-import, no exported record/sum/overload-set tests.
71. [OPEN] **`integration_test.rs` names promise runtime checks but stop at `check_program`** (:51-124); a miscompile still
    passes. `sum_codegen_test.rs:144-164` asserts nothing (`let _ = result`) and duplicates the previous test.
72. [OPEN] **Stale lexer tests exercise syntax the language doesn't have** (`Result{T, E}` generics, pre-0.9 forms —
    `lexer_tests.rs:6-58,174-182`); `test_multiline_comment` (:185) tests two single-line comments.
73. [FIXED — shared `tests/common` harness (one `assert_exit`/JIT lock/build-and-run home)] **Harness duplication** — `assert_exit` + `JIT_LOCK` copy-pasted 8×; `assert_rejected` variants differ in
    strictness between files; the JIT lock only serializes within one test binary.
74. [PARTIAL — #89 filled the arithmetic gaps; the rest remains] **Untested working edges** — unary minus, `||`/`!`, `[]`, `""`, negative-span ranges, 3+-member overload sets,
    overload dispatch through `|>`, user overloads of `-`/`*`/`<=`/`>=`/`!=`. The `%` bug (finding 6) shows exactly
    how this class hides real breakage. `quilon compile` has zero coverage; `check` success path untested.

### Code quality / dead code
75. [FIXED — the #87 refactor series removed the dead scaffolding] **`inference.rs` is 310 lines of dead scaffolding** — module-wide `#![allow(dead_code)]`, an HM unifier wired to
    nothing, with its own test suite. Plus dead `TypeError` variants, unused `Symbol.span`, practically unreachable
    `AmbiguousOverload`.
76. [FIXED — #87: generator/checker/parser split into per-area child modules; FrameState (#79) fixed the state root cause] **God-object `CodeGenerator`** — ~10 mutable per-function maps with three inconsistent save/restore flavors
    (the root of finding 2); `with_oracle` re-runs the entire type check; ~450 hand-rolled
    `.map_err(format!)` calls; `build_direct_call` duplicates `call_result_to_basic`.
77. [FIXED — #87 parser split; spread-in-constructor works (LANGUAGE.md documents `Vec {<-p, x = 9}`)] **Parser duplication with user-visible divergence** — param-list and record-field parsing each exist twice;
    the constructor-literal branch doesn't support spread while `parse_record` does.
78. [PARTIAL — deep-recursion aborts fixed by #96; findings 12/14 fixed, shrinking the checker/codegen-divergence panic surface; `Parser::parse` non-Eof panic remains] **Reachable panics on unusual-but-valid shapes** — `Parser::parse` panics on non-Eof-terminated input
    (`ast_parser.rs:1541,1548` — public API, safe only by lexer convention); unwraps/panics in codegen array methods,
    `build_result`, `coerce_payload` fire if checker and codegen ever disagree.
79. [OPEN] **Runtime papercuts** — GC alloc failure unchecked (null `data` with `len>0` → downstream UB — see also the
    `__alloc` null-check note filed with the #158-era findings); size-computation `count * elem_size` can overflow i64
    into GC_malloc's `≤0 → 1 byte` clamp then write past it; JIT argv with interior NUL becomes `""`.
80. [PARTIAL — `--debug`/`-g` shipped on `build` (DWARF); still no `--version`, no `-O` levels (M7 roadmap row), exit-code ambiguity remains] **CLI papercuts** — no `--version` flag (`main.rs:16-18`); exit code 1 is ambiguous between compiler failure,
    runtime error, and a program legitimately returning 1; `check`/`compile` status emoji go to stdout (unpipeable)
    while `run` is silent; no `-O` flag on `build` (always `OptimizationLevel::None`).
81. [PARTIAL — the O(file) `line_col` scan is gone (per-file line index in `source_map.rs`, checked against the naive form); caret misalignment on tabs/double-width chars remains] **Diagnostics papercuts** — caret misaligns on tabs and double-width (CJK/emoji) chars (`diagnostic.rs:53-64`);
    `Span::line_col` is an O(file) scan per diagnostic; import/read errors bypass rendering entirely (span-less
    one-liners); several `TypeError` variants Display raw `{:?}` type dumps; `NonExhaustiveMatch` doesn't say which
    variants are missing.
82. [FIXED — interpolation shipped (making the token.rs claim true), and the docs sweeps removed the stale comments] **Stale doc comments / vestiges** — `nodes.rs:12` claims imports are unimplemented (they work); `token.rs:226`
    claims string interpolation exists (it doesn't; the `\<` escape at :243 is its vestige); "Workstream B1" jargon
    in shipped comments; imprecise spans on `Param` (last token only) and desugared method-callee idents (whole call).
83. [PARTIAL — watermark shipped (#97); `.each` index/`zip` and the rest remain, mostly design] **Language papercuts (documented but worth listing)** — legacy `^ = (argc, argv)` shim keeps a knowingly-wrong
    placeholder argv with no warning (`generator.rs:595-596`); `.each` lost the index when `for (item, index)` was
    removed, with no `zip`/indexed variant; `env` as `[][]Text` can't distinguish unset from empty (PARTIAL: #197
    delivers env as a `[|Text => Text|]` Map — verify on merge); all iteration flows through six built-in methods users
    could never write themselves (PR #196 — first-class higher-order functions — is in flight for exactly this);
    record aliasing under mixed `=`/`:=` bindings is now the subject of the #193 deep-immutability decision.

---

## Open-issue cross-reference summary (as of the original 2026-08-08 review — see the re-audit sections above for current state)

| Issue | Status vs findings |
|---|---|
| #64/#65 assert track-caller | Exact duplicates of each other (finding 65). Related: panic/assert-hygiene themes (73, 78). |
| #60 input reading | Confirmed gap. Design the errno/EINTR story with it (finding 54). |
| #58 VS Code publish | Finding 60 (tag trigger never fires) is a plausible root cause. |
| #57 Text=[]Grapheme | Findings 27, 55, 56, 59 are direct design inputs. |
| #56 self-asserting examples | Confirmed central; findings 25, 66, 67 are the same weakness from other angles. |
| #54 split quilon-rt | Confirmed (one lib.rs); fold finding 53 (shared intrinsic table) into it. |
| #50 core.cli | Blocked harder than stated: findings 17 (no Num↔Text), 35 (no Text patterns), 52 (span collisions) all bite it. |
| #49 static libgc | Confirmed; compounds finding 11 (releases can't build at all). |
| #45 watermark | No overlap. |

**Biggest untracked themes** (candidates for new issues): the P0 codegen/soundness cluster (1–10),
the checker/codegen type-oracle divergence class (12, 14, 42, 59), distribution of a working `quilon build` (11),
module-system correctness (23, 24, 52), and output-asserting tests (66).

---

*Confirmed-repro scratch files (outside the repo):
`/tmp/claude-1000/-home-assaf-code-quilon/ed1b04b0-d6b4-4078-9491-7871ca95ef82/scratchpad/`
(`shortcircuit.ql`, `rec_escape.ql`, `tco_alloca.ql`, `stale_record.ql`, `oob.ql`, `negidx.ql`, `fracidx.ql`,
`overload_idx.ql`, `tco_wrong.ql`, `dangling.ql`, `globalcall.ql`, `hetres.ql`, `args.ql`, `at_nan.ql`, and the
2026-08-26 re-audit probes `f15.qn`, `f21.qn`, `f22.qn`, `f35.qn`, `f43.qn`, `f47.qn`, `nonsum.qn`).*
