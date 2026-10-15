//! Deferral analysis — the compiler's view of Quilon's `@` leaf-IO-primitive tier, and the
//! deferred-value taint that makes force-on-use real.
//!
//! Produces the **taint**: which expressions may evaluate to a *deferred* value (a promise,
//! from a value-returning `@` primitive like `@readStdin`), plus the **force-set**
//! (`force_sites`) — the exact spans where the code generator must force one, because a
//! strict primitive is about to read its bytes or it would escape.
//!
//! It reads no types and adds none: a deferred `Text` keeps the ordinary type `Text` — the
//! load-bearing guardrail of the model.
//!
//! Taint is a forward dataflow: a value is deferred iff it flows from `@readStdin` through
//! only *lazy carriers* (a `=` binding, and the arms/result of `?`/ternary/blocks) without
//! crossing a *strict* slot (arithmetic/comparison/logical operands, a match scrutinee,
//! `print`/native/`@` args, indexing, field/array/record construction, interpolation holes,
//! and a function/method/lambda body result). Only tainted spans get forces, so pure code
//! pays nothing. A promise stays inside the function body it was born in — cross-function
//! promise pipelining is a later step.
//!
//! A top-level `:=` binding is the one exception: any function may store a deferred value
//! into it and any other may read it back, so `analyze` also tracks a program-wide
//! `deferred_globals` set, computed in rounds (bounded by the program's number of
//! top-level bindings, so it always terminates) since one global's deferredness may depend
//! on another's already in the set.
//!
//! The same walk enforces that an `@name := …` atomic-binding reassignment's right side may
//! not force a deferred value (see
//! `docs/concurrency/README.md#sharing-state-across-fibers` — forcing parks the fiber
//! mid-statement, risking a stale read from before the park). Which `:=` spans are atomic
//! or top-level reassignments is the type checker's answer
//! (`TypeChecker::take_atomic_reassignments`/`take_top_level_reassignments`), taken
//! ready-made: this pass's own per-call `Scope` does no name resolution of its own.

use crate::ast::{
    Expression, InterpolationPart, Item, MethodDeclaration, Program, Statement,
    VariableDeclaration, at_primitive_name,
};
use crate::lexer::Span;
use std::collections::{HashMap, HashSet};

/// The bare name of the pause primitive (`time.@sleep(seconds)`): effect-only, and the one
/// place calling an `@` primitive waits the instant it is called — no deferred value, so no
/// later force to catch it. Used only by [`Taint::record_transitive_wait`]'s own positive
/// list of primitives that wait on the call itself, rather than trying to exclude every
/// deferred-producing/background-launching primitive by shape (which the options-overload
/// arity on `@tcpServe`/`@serve` already shows is easy to under-cover).
const SLEEP_PRIMITIVE: &str = "sleep";

/// The bare name of the streaming-read primitive (`io.@streamFile(path, chunkSize,
/// onChunk)`): runs strictly on the calling fiber, parking when a read reports not ready —
/// the other primitive [`Taint::record_transitive_wait`] treats as waiting on the call
/// itself.
const STREAM_FILE_PRIMITIVE: &str = "streamFile";

/// The bare name of the value-returning stdin read primitive, reached by an importer as
/// `io.@readStdin()`; the call evaluates to a deferred `Text`.
const READ_PRIMITIVE: &str = "readStdin";

/// The bare name of the request-exchange socket primitive, reached by an importer as
/// `net.@tcpRequest(addr, req)`; the call evaluates to a deferred `Result`
/// (`Ok(responseBytes)` / `NotOk(message)`), read once forced.
const TCP_REQUEST_PRIMITIVE: &str = "tcpRequest";

/// The bare name of `Connection`'s deferred read primitive, reached through a value —
/// `connection.@read()` — rather than a module binding; fused the same way
/// `@readStdin`/`@tcpRequest` are.
const CONNECTION_READ_PRIMITIVE: &str = "read";

/// The bare name of the raw TCP server primitive, reached as `net.@tcpServe(address,
/// handler)`. Its own return value (the `Server` handle) is never deferred, but its accept
/// loop launches in the background all the same — see [`launches_in_background`].
const TCP_SERVE_PRIMITIVE: &str = "tcpServe";

/// The bare name of the HTTP server primitive, reached as `http.@serve(address, handler)`.
/// Its lowering calls the very same runtime entry `@tcpServe` does, so it launches an
/// accept loop in the background exactly the same way — see [`launches_in_background`].
const HTTP_SERVE_PRIMITIVE: &str = "serve";

/// The argument count both `@tcpServe` and `@serve` take (`address`, `handler`) —
/// [`is_serve_call`]'s one arity, shared because the two happen to agree.
const SERVE_ARITY: usize = 2;

/// What the analysis hands to codegen.
#[derive(Debug, Default, Clone)]
pub struct DeferInfo {
    /// The force-set: spans of expressions whose generated value codegen must force in
    /// place, because a deferred value sits where a strict primitive reads its bytes or
    /// would escape. Empty for pure programs — the whole codegen-visible surface of the
    /// taint analysis.
    force_sites: HashSet<Span>,
    /// Spans of `< >` blocks that directly launch at least one value-returning `@`
    /// primitive — the block-scope join's own surface. Codegen opens a launch registry on
    /// entry to such a block and joins it (`allSettled`) before the block's value flows
    /// out; every other block (the overwhelming majority) emits neither call. "Directly"
    /// stops at a nested block or lambda — a launch inside one belongs to THAT scope.
    launch_scopes: HashSet<Span>,
}

impl DeferInfo {
    /// Whether the value produced for the expression at `span` must be forced in place.
    pub fn is_force_site(&self, span: &Span) -> bool {
        self.force_sites.contains(span)
    }

    /// Whether the `< >` block at `span` must open and join a launch registry — it
    /// directly launches at least one value-returning `@` primitive.
    pub fn is_launch_scope(&self, span: &Span) -> bool {
        self.launch_scopes.contains(span)
    }
}

/// A reassignment to an atomic binding whose right side forces a deferred value — the
/// binding-side twin of the locked rule that an atomic type's setter body may not force.
/// `transitive` is set only when the wait is not IN the right side's own expression but
/// reached through a call: `(function, what)` — the first function or method on the call
/// path whose own body waits, and what it waits on (an `@` primitive's qualified name, or a
/// note that the callee is a value the checker could not resolve).
#[derive(Debug)]
pub struct AtomicReassignmentForced {
    pub name: String,
    pub span: Span,
    pub transitive: Option<(String, String)>,
}

/// Analyze `program`: the deferred-value taint and force-set — the whole codegen-visible
/// surface of the analysis. `atomic_reassignments` is the type checker's own answer (see
/// [`crate::typechecker::TypeChecker::take_atomic_reassignments`]) — the span of every
/// `:=` statement it resolved as reassigning an atomic binding. `top_level_reassignments`
/// is the checker's parallel answer for the global-tracking rule (see
/// [`crate::typechecker::TypeChecker::take_top_level_reassignments`]) — the span of every
/// `:=` statement it resolved as reassigning a top-level binding, atomic or not. `Err`
/// names the first atomic-reassignment violation, in program order, whose right side
/// forces a deferred value.
///
/// Runs the walk to a fixed point over `deferred_globals` (see the module doc): each round
/// starts from the previous round's set, and the round that adds nothing to it is the
/// answer — its own `force_sites` is what codegen gets, and its own violation (if any) is
/// what `Err` reports. The set only grows and is bounded by the program's number of
/// top-level `:=` bindings, so the loop always terminates.
pub fn analyze(
    program: &Program,
    atomic_reassignments: &HashSet<Span>,
    top_level_reassignments: &HashSet<Span>,
) -> Result<DeferInfo, AtomicReassignmentForced> {
    let known_functions = known_function_names(program);
    let mut deferred_globals: HashSet<String> = HashSet::new();
    loop {
        let mut taint = Taint {
            force_sites: HashSet::new(),
            launch_scopes: HashSet::new(),
            block_stack: Vec::new(),
            atomic_reassignments,
            top_level_reassignments,
            deferred_globals: &deferred_globals,
            newly_deferred_globals: HashSet::new(),
            violation: None,
            known_functions: &known_functions,
            waiting_functions: HashMap::new(),
            current_function: None,
            transitive_waits: Vec::new(),
            producers_seen: HashSet::new(),
            track_unresolved_callees: true,
        };
        for item in &program.items {
            taint.analyze_item(item);
        }

        if taint.newly_deferred_globals.is_empty() {
            return match taint.violation {
                Some(violation) => Err(violation),
                None => Ok(DeferInfo {
                    force_sites: taint.force_sites,
                    launch_scopes: taint.launch_scopes,
                }),
            };
        }
        deferred_globals.extend(taint.newly_deferred_globals);
    }
}

/// Every top-level function name and type-method name in `program` — a NAMED, resolvable
/// callee, as opposed to a call through a function VALUE (a parameter, a captured closure)
/// the checker cannot follow. Over-approximate the same way `ast::reachability` is: an
/// overload set's members all share one name here, and a top-level `=`/`:=` binding whose
/// value is itself a lambda literal counts too (`greet = () => <...>`, not just the
/// `name = (params) -> T => body` sugar). A member call (`.trim()`, a type's own method) is
/// never checked against this set at all — see `Taint::record_transitive_wait`'s own doc.
fn known_function_names(program: &Program) -> HashSet<String> {
    let mut names = HashSet::new();
    for item in &program.items {
        match item {
            Item::FunctionDeclaration(f) => {
                names.insert(f.name.clone());
            }
            Item::VariableDeclaration(v) if matches!(v.value, Expression::Lambda { .. }) => {
                names.insert(v.name.clone());
            }
            Item::TypeDeclaration(t) => {
                for method in t.type_definition.methods() {
                    names.insert(method.name.clone());
                }
            }
            Item::VariableDeclaration(_) | Item::TrapDeclaration(_) => {}
        }
    }
    names
}

/// The deferred-value taint accumulator, for one round of the fixed-point walk.
struct Taint<'a> {
    force_sites: HashSet<Span>,
    launch_scopes: HashSet<Span>,
    /// The spans of `< >` blocks currently being visited, innermost last — a launch found
    /// while this is non-empty belongs to its last entry (see `launch_scopes`'s own doc).
    block_stack: Vec<Span>,
    /// See [`analyze`].
    atomic_reassignments: &'a HashSet<Span>,
    /// See [`analyze`].
    top_level_reassignments: &'a HashSet<Span>,
    /// The top-level `:=` bindings known deferred as of the START of this round (a prior
    /// round's fixed set, read-only here — this round's own findings accumulate
    /// separately in `newly_deferred_globals` so they take effect next round, not
    /// mid-walk).
    deferred_globals: &'a HashSet<String>,
    /// Top-level `:=` bindings this round found stored a deferred value, over and above
    /// `deferred_globals` — folded into the set `analyze` starts the next round with.
    newly_deferred_globals: HashSet<String>,
    /// The first atomic-reassignment violation found, in program order; later ones are not
    /// worth collecting; the whole point is to name one concrete fix.
    violation: Option<AtomicReassignmentForced>,
    /// See [`known_function_names`].
    known_functions: &'a HashSet<String>,
    /// Every function/method already analyzed this round (in program order — a callee is
    /// always declared above its caller, so by the time a call to it is visited its own
    /// answer here is already final) that itself waits, directly or transitively: name ->
    /// (the first function on ITS call path that waits, what it waits on). Read by
    /// [`Self::record_transitive_wait`], written by [`Self::record_waiting_function`].
    waiting_functions: HashMap<String, (String, String)>,
    /// The function or method currently being analyzed, if any — `None` at the top level
    /// (a top-level binding's own value, or a reassignment's right side with no enclosing
    /// named function of its own). Set and restored around each function/method body visit.
    current_function: Option<String>,
    /// Every transitive wait found so far, in visit order — append-only, like `force_sites`:
    /// [`Self::check_atomic_reassignment`] and [`Self::record_waiting_function`] each
    /// snapshot its length before visiting an expression, the same way `force_sites` is
    /// snapshotted, so only entries added DURING that one visit count towards it.
    transitive_waits: Vec<(String, String)>,
    /// Every value-returning `@` primitive's own name seen while visiting the CURRENT
    /// function/method body — a provenance hint only, so `record_waiting_function`'s generic
    /// `force_sites`-growth case (the body forces a deferred value, but not through a call
    /// `record_transitive_wait` already named) can still label it with the actual primitive
    /// (`io.@readStdin`), when there was exactly one, rather than the bare "a deferred
    /// value" — naming the wrong one, with two or more producers in the body, would mislead
    /// more than a generic label does. Saved and restored around each function/method body
    /// visit, like `current_function`.
    producers_seen: HashSet<String>,
    /// Whether the body currently being visited may be flagged for an unresolved (call
    /// through a value) callee — `false` only inside an inert corelib placeholder
    /// (`FunctionDeclaration::is_inert_corelib_placeholder`): that body is never actually
    /// executed (the compiler lowers the call to a runtime intrinsic instead), so an
    /// ordinary higher-order shape in its fake implementation — `@serve`'s own placeholder
    /// invoking its `handler` parameter — must not poison `waiting_functions` for every
    /// real program that calls it. `force_sites`/`@sleep`/`@streamFile`/already-known-waiting
    /// propagation are unaffected; only this one conservative, false-positive-prone branch
    /// is suppressed. `true` everywhere else (including every method body — a method is
    /// never an inert placeholder).
    track_unresolved_callees: bool,
}

impl<'a> Taint<'a> {
    /// A fresh scope for a function/method body, seeing this round's `deferred_globals`.
    fn fresh_scope(&self) -> Scope<'a> {
        Scope::new(self.deferred_globals)
    }

    /// Record that `name` (a top-level `:=` binding) was just stored a deferred value,
    /// as `newly_deferred_globals` growth ONLY when `name` is not already in this round's
    /// `deferred_globals` — otherwise a global already known deferred would keep re-adding
    /// itself every round forever, and the fixed point in `analyze` would never see an
    /// empty round to stop on.
    fn mark_global_deferred(&mut self, name: &str) {
        if !self.deferred_globals.contains(name) {
            self.newly_deferred_globals.insert(name.to_string());
        }
    }

    fn analyze_item(&mut self, item: &Item) {
        match item {
            // A function/method body is a strict slot: forcing its result keeps a promise from
            // escaping across the call boundary.
            Item::FunctionDeclaration(f) => {
                self.analyze_function_body(&f.name, &f.body, !f.is_inert_corelib_placeholder());
            }
            // A top-level `:=` binding's own initial value is a STORE into a global, exactly
            // like a later reassignment of it (see the module doc): a deferred value there
            // joins `deferred_globals` rather than being forced here. A top-level `=`
            // binding is never reassigned, so nothing else ever needs to read it unforced —
            // forcing it here, once, is enough to keep it always-ready everywhere.
            Item::VariableDeclaration(v) => {
                let scope = self.fresh_scope();
                let deferred = self.analyze_declaration_value(v, &scope);
                if deferred {
                    if v.mutable {
                        self.mark_global_deferred(&v.name);
                    } else {
                        self.force_sites.insert(v.value.span().clone());
                    }
                }
            }
            Item::TypeDeclaration(t) => {
                for method in t.type_definition.methods() {
                    self.analyze_method(method);
                }
            }
            // The runtime calls an arm directly, so its own launch is joined here too.
            Item::TrapDeclaration(trap) => {
                for arm in &trap.arms {
                    let scope = self.fresh_scope();
                    self.strict(&arm.body, &scope);
                }
            }
        }
    }

    fn analyze_method(&mut self, method: &MethodDeclaration) {
        self.analyze_function_body(&method.name, &method.body, true);
    }

    /// Analyze one function or method's own body as a strict slot, under `name`, then record
    /// whether `name` itself waits — directly (a force grew `force_sites`, or a call reached
    /// an `@` primitive) or transitively (a call reached a name already known to wait) — into
    /// `waiting_functions`, so a LATER caller of `name` (always declared below it — no
    /// hoisting) sees the answer without re-walking this body. A name already present keeps
    /// its first-found answer (an overload set's members share one name here, same as
    /// `known_function_names`'s own over-approximation). `track_unresolved_callees` is
    /// `false` only for an inert corelib placeholder — see that field's own doc.
    fn analyze_function_body(
        &mut self,
        name: &str,
        body: &Expression,
        track_unresolved_callees: bool,
    ) {
        let force_sites_before = self.force_sites.len();
        let transitive_waits_before = self.transitive_waits.len();
        let previous_function = self.current_function.replace(name.to_string());
        let previous_producers = std::mem::take(&mut self.producers_seen);
        let previous_tracking =
            std::mem::replace(&mut self.track_unresolved_callees, track_unresolved_callees);
        let scope = self.fresh_scope();
        self.strict(body, &scope);
        self.current_function = previous_function;
        self.track_unresolved_callees = previous_tracking;
        let producers_during = std::mem::replace(&mut self.producers_seen, previous_producers);
        self.record_waiting_function(
            name,
            force_sites_before,
            transitive_waits_before,
            &producers_during,
        );
    }

    /// See [`Taint::transitive_waits`]'s own doc: fold this body's own visit into
    /// `waiting_functions`, preferring a transitive wait's more specific label (names the
    /// actual `@` primitive or unresolved callee) over the generic "forces a deferred value"
    /// a bare `force_sites` growth gives — both can fire for the SAME body (a direct
    /// `@readStdin` call is both a transitive-wait hit at its own call site and, once its
    /// result is forced two lines later, a `force_sites` hit too), and the specific one is
    /// always at least as informative.
    fn record_waiting_function(
        &mut self,
        name: &str,
        force_sites_before: usize,
        transitive_waits_before: usize,
        producers_seen: &HashSet<String>,
    ) {
        if self.waiting_functions.contains_key(name) {
            return;
        }
        if let Some(found) = self.transitive_waits.get(transitive_waits_before).cloned() {
            self.waiting_functions.insert(name.to_string(), found);
        } else if self.force_sites.len() > force_sites_before {
            let what = match producers_seen.iter().next() {
                Some(producer) if producers_seen.len() == 1 => producer.clone(),
                _ => "a deferred value".to_string(),
            };
            self.waiting_functions
                .insert(name.to_string(), (name.to_string(), what));
        }
    }

    /// Whether this call reaches an `@` primitive or an already-known-waiting function —
    /// pushed onto `transitive_waits` if so.
    ///
    /// Only `@sleep`/`@streamFile` — [`SLEEP_PRIMITIVE`]/[`STREAM_FILE_PRIMITIVE`] — wait the
    /// instant they are called: positively listed, rather than excluding every
    /// deferred-producing (`@readStdin`/`@tcpRequest`/`connection.@read`) or
    /// background-launching (`@tcpServe`/`@serve`) primitive by shape, since an overload
    /// (`@serve`'s own 3-argument `ServerOptions` form) can fall outside the arity
    /// `produces_deferred`/`launches_in_background` check for, and under-covering there
    /// would wrongly reject an ordinary `stand := http.@serve(address, handler, options)`.
    /// Calling a value-returning primitive does not itself wait — only FORCING its result
    /// does, which `record_waiting_function`'s own `force_sites` check already catches
    /// intraprocedurally; counting the bare call here too would wrongly reject
    /// `testimony := io.@readStdin()` itself, the accepted "store it deferred, force it
    /// later elsewhere" shape.
    ///
    /// A MEMBER call (`.trim()`, `.each(...)`, a type's own method) is only ever checked
    /// against `waiting_functions` (keyed by the method's own bare name, exactly as
    /// `known_function_names` collects it) — never flagged as "unresolved": telling a
    /// built-in/corelib composable's qualified name apart from a genuine unresolved method
    /// is exactly the distinction this pass does not make, so a member call simply never
    /// triggers that branch. A NON-member call to a bare name that is neither `@`-marked nor
    /// a known top-level function (`known_functions`) is a call through a function VALUE —
    /// a parameter or a captured closure — which the checker cannot follow; conservatively
    /// treated as a wait too, naming the value instead of a function.
    fn record_transitive_wait(&mut self, function: &Expression, member_call: bool) {
        let Expression::Identifier { name, .. } = function else {
            return;
        };
        // `@tcpServe`/`@serve` return before any wait.
        // Their handler runs on its own fiber later, once a connection arrives.
        if matches!(
            at_primitive_name(name),
            Some(TCP_SERVE_PRIMITIVE) | Some(HTTP_SERVE_PRIMITIVE)
        ) {
            return;
        }
        if matches!(
            at_primitive_name(name),
            Some(SLEEP_PRIMITIVE) | Some(STREAM_FILE_PRIMITIVE)
        ) {
            let culprit = self
                .current_function
                .clone()
                .unwrap_or_else(|| name.clone());
            self.transitive_waits.push((culprit, name.clone()));
            return;
        }
        if let Some(found) = self.waiting_functions.get(name).cloned() {
            self.transitive_waits.push(found);
            return;
        }
        // A capitalized callee (`Ok(x)`, a sum variant's payload constructor) is never a
        // function value — the language's own naming convention (every function name is
        // lowercase; every type/constructor name is capitalized) is enough to tell the two
        // apart without a type-level constructor registry this syntactic pass does not have.
        let is_constructor_shaped = name
            .rsplit('.')
            .next()
            .and_then(|last| last.chars().next())
            .map(|c| c.is_uppercase())
            .unwrap_or(false);
        if self.track_unresolved_callees
            && !member_call
            && !is_constructor_shaped
            && at_primitive_name(name).is_none()
            && !self.known_functions.contains(name)
        {
            self.transitive_waits.push((
                name.clone(),
                "the callee is a value, not a function the checker can follow".to_string(),
            ));
        }
    }

    /// Visit `expression` in a STRICT slot: analyze it, and if its value is deferred, force it here.
    fn strict(&mut self, expression: &Expression, env: &Scope<'a>) {
        if self.visit(expression, env) {
            self.force_sites.insert(expression.span().clone());
        }
    }

    /// Record `v` as the atomic-reassignment violation — once, the first one found — when
    /// its span is one the checker resolved as reassigning an atomic binding, and evaluating
    /// its value either grew `force_sites` past `force_sites_before` (a DIRECT force, right
    /// there in the right side's own expression) or grew `transitive_waits` past
    /// `transitive_waits_before` (reached only through a call — see
    /// [`Self::record_transitive_wait`]). The direct case takes priority when both fire (a
    /// call reaching an `@` primitive whose result is then also forced two steps later would
    /// otherwise report twice): it needs no extra naming, so it is the simpler message.
    fn check_atomic_reassignment(
        &mut self,
        v: &VariableDeclaration,
        force_sites_before: usize,
        transitive_waits_before: usize,
    ) {
        if self.violation.is_some() || !self.atomic_reassignments.contains(&v.span) {
            return;
        }
        if self.force_sites.len() > force_sites_before {
            self.violation = Some(AtomicReassignmentForced {
                name: v.name.clone(),
                span: v.span.clone(),
                transitive: None,
            });
        } else if let Some(found) = self.transitive_waits.get(transitive_waits_before).cloned() {
            self.violation = Some(AtomicReassignmentForced {
                name: v.name.clone(),
                span: v.span.clone(),
                transitive: Some(found),
            });
        }
    }

    /// Analyze a `:=`/`=` declaration or reassignment's value in `env`, checking along the
    /// way for the atomic-reassignment violation, and return whether the value is
    /// delivered to the caller still deferred (a `=`/`:=` binding is itself a lazy carrier
    /// — see [`Self::visit_block`] — so the caller, not this method, decides whether to
    /// force it).
    fn analyze_declaration_value(&mut self, v: &VariableDeclaration, env: &Scope<'a>) -> bool {
        let force_sites_before = self.force_sites.len();
        let transitive_waits_before = self.transitive_waits.len();
        let deferred = self.visit(&v.value, env);
        self.check_atomic_reassignment(v, force_sites_before, transitive_waits_before);
        deferred
    }

    /// Analyze `expression`, recording forces for its own strict children, and return whether its
    /// value is delivered to the parent still deferred (i.e. it reached here through lazy
    /// carriers only). The parent decides whether to force it, via [`Self::strict`].
    fn visit(&mut self, expression: &Expression, env: &Scope<'a>) -> bool {
        match expression {
            Expression::Number { .. }
            | Expression::String { .. }
            | Expression::Bool { .. }
            | Expression::Unit { .. } => false,
            Expression::Identifier { name, .. } => env.is_deferred(name),

            // A value-returning `@` primitive (`@readStdin`, `@tcpRequest`, `Connection`'s
            // `@read`) is the only kind of deferred-producing call; every other call delivers
            // a ready value (its own body forced its result). The callee expression and the
            // arguments are all strict slots (a deferred value used inside `function` — e.g. a
            // called lambda — or passed as an argument is forced there).
            Expression::Call {
                function,
                arguments,
                member_call,
                ..
            } => {
                self.strict(function, env);
                // A handler passed to `@tcpServe`/`@serve` is never invoked synchronously
                // as part of THIS call — it is stored and run later, once per accepted
                // connection, each on its own fresh fiber — so a wait inside it (reading
                // from that connection, say) is not a wait the launch's OWN right side
                // takes on. Discard anything `record_transitive_wait` found while visiting
                // the handler argument's body; the normal taint/force tracking on it
                // (needed for the handler's own analysis, done when IT is itself checked
                // as the atomic-reassignment target of some later statement) is unaffected.
                let is_server_launch = matches!(
                    &**function,
                    Expression::Identifier { name, .. }
                        if matches!(at_primitive_name(name), Some(TCP_SERVE_PRIMITIVE) | Some(HTTP_SERVE_PRIMITIVE))
                );
                let transitive_waits_before_arguments = self.transitive_waits.len();
                for arg in arguments {
                    self.strict(arg, env);
                }
                if is_server_launch {
                    self.transitive_waits
                        .truncate(transitive_waits_before_arguments);
                }
                self.record_transitive_wait(function, *member_call);
                let deferred = produces_deferred(function, arguments, *member_call);
                if deferred && let Expression::Identifier { name, .. } = &**function {
                    self.producers_seen.insert(name.clone());
                }
                if launches_in_background(function, arguments, *member_call)
                    && let Some(scope) = self.block_stack.last()
                {
                    self.launch_scopes.insert(scope.clone());
                }
                deferred
            }

            Expression::BinaryOperator { left, right, .. } => {
                self.strict(left, env);
                self.strict(right, env);
                false
            }
            Expression::UnaryOperator { expression, .. }
            | Expression::Spread { expression, .. } => {
                self.strict(expression, env);
                false
            }
            Expression::FieldAccess { expression, .. } => {
                self.strict(expression, env);
                false
            }
            Expression::FieldAssign { target, value, .. }
            | Expression::IndexAssign { target, value, .. } => {
                self.strict(target, env);
                self.strict(value, env);
                false
            }
            Expression::Index {
                expression, index, ..
            } => {
                self.strict(expression, env);
                self.strict(index, env);
                false
            }
            Expression::Range { start, end, .. } => {
                self.strict(start, env);
                self.strict(end, env);
                false
            }
            Expression::Array { elements, .. } | Expression::SetLiteral { elements, .. } => {
                for element in elements {
                    self.strict(element, env);
                }
                false
            }
            Expression::MapLiteral { entries, .. } => {
                for (key, value) in entries {
                    self.strict(key, env);
                    self.strict(value, env);
                }
                false
            }
            Expression::Record { fields, .. } | Expression::Constructor { fields, .. } => {
                for (_, value) in fields {
                    self.strict(value, env);
                }
                false
            }
            Expression::Interpolation { parts, .. } => {
                for part in parts {
                    if let InterpolationPart::Hole(hole) = part {
                        self.strict(hole, env);
                    }
                }
                false
            }
            // A lambda body result is strict (a closure never returns a promise). The outer
            // scope is visible so a captured deferred value used strictly inside is forced.
            Expression::Lambda { body, .. } => {
                self.strict(body, env);
                false
            }

            // Lazy carriers: the arms/result flow the value through without forcing, so the
            // If/Match/Block delivers deferred iff any branch does — the parent slot forces it.
            Expression::If {
                condition,
                then,
                else_,
                ..
            } => {
                self.strict(condition, env);
                let then_deferred = self.visit(then, env);
                let else_deferred = self.visit(else_, env);
                then_deferred || else_deferred
            }
            Expression::Match {
                expression, arms, ..
            } => {
                self.strict(expression, env);
                let mut any = false;
                for arm in arms {
                    any |= self.visit(&arm.body, env);
                }
                any
            }
            Expression::Block { statements, span } => {
                self.block_stack.push(span.clone());
                let deferred = self.visit_block(statements, env);
                self.block_stack.pop();
                deferred
            }
        }
    }

    /// A block introduces a scope. Bindings carry their value's deferredness (a `=` is lazy);
    /// non-final statement values are discarded (not forced — the launch still runs); the
    /// final expression's value is the block's value, delivered to the block's own slot.
    fn visit_block(&mut self, statements: &[Statement], env: &Scope<'a>) -> bool {
        let mut local = env.child();
        let last = statements.len().saturating_sub(1);
        let mut result_deferred = false;
        for (index, statement) in statements.iter().enumerate() {
            match statement {
                Statement::Item(Item::VariableDeclaration(v)) => {
                    let deferred = self.analyze_declaration_value(v, &local);
                    // A reassignment of a TOP-LEVEL binding is a store into a global (see
                    // the module doc): its deferredness joins `newly_deferred_globals`
                    // rather than only the local scope, so another function's read of it
                    // sees the taint too. Telling a reassignment of a global apart from a
                    // fresh local `:=` of the same name is the checker's job, the same way
                    // it is for `atomic_reassignments` — see `top_level_reassignments`.
                    if deferred && self.top_level_reassignments.contains(&v.span) {
                        self.mark_global_deferred(&v.name);
                    }
                    local.bind(v.name.clone(), deferred);
                }
                Statement::Item(Item::FunctionDeclaration(f)) => {
                    self.analyze_function_body(&f.name, &f.body, true);
                }
                // A block-declared type's methods are analyzed exactly like a
                // top-level type's (see `analyze_item`): each method body is its own
                // strict slot, in a fresh scope.
                Statement::Item(Item::TypeDeclaration(t)) => {
                    for method in t.type_definition.methods() {
                        self.analyze_method(method);
                    }
                }
                // A trap is a top-level-only item; never a block statement.
                Statement::Item(Item::TrapDeclaration(_)) => {
                    unreachable!("a trap is a top-level-only item")
                }
                Statement::Expression(e) => {
                    if index == last {
                        result_deferred = self.visit(e, &local);
                    } else {
                        // Discarded: analyze for its own strict children, but the value itself
                        // need not be forced — its launch already ran (eager launch).
                        let _ = self.visit(e, &local);
                    }
                }
            }
        }
        result_deferred
    }
}

/// A lexical scope mapping in-scope names to whether they hold a deferred value. Names
/// absent from the map (parameters, pattern bindings from a forced scrutinee) are ready,
/// UNLESS they name a top-level `:=` binding in `deferred_globals` (see the module doc) —
/// every such name reads as deferred everywhere, not only in the scope it was stored from.
#[derive(Clone)]
struct Scope<'a> {
    deferred_names: HashMap<String, bool>,
    deferred_globals: &'a HashSet<String>,
}

impl<'a> Scope<'a> {
    fn new(deferred_globals: &'a HashSet<String>) -> Self {
        Scope {
            deferred_names: HashMap::new(),
            deferred_globals,
        }
    }

    fn child(&self) -> Self {
        self.clone()
    }

    fn bind(&mut self, name: String, deferred: bool) {
        self.deferred_names.insert(name, deferred);
    }

    fn is_deferred(&self, name: &str) -> bool {
        self.deferred_names
            .get(name)
            .copied()
            .unwrap_or_else(|| self.deferred_globals.contains(name))
    }
}

/// Whether `function`/`arguments` is a call to the `@readStdin` primitive (`@readStdin()`, no
/// arguments) — qualified (`io.@readStdin`) or, as inside `core.io` itself, still bare.
fn is_read_call(function: &Expression, arguments: &[Expression]) -> bool {
    matches!(function, Expression::Identifier { name, .. }
        if at_primitive_name(name) == Some(READ_PRIMITIVE))
        && arguments.is_empty()
}

/// Whether `function`/`arguments` is a call to the `@tcpRequest` primitive —
/// `@tcpRequest(address, requestBytes)` or `@tcpRequest(address, requestBytes, options)` —
/// qualified or bare, the same as [`is_read_call`].
fn is_tcp_request_call(function: &Expression, arguments: &[Expression]) -> bool {
    matches!(function, Expression::Identifier { name, .. }
        if at_primitive_name(name) == Some(TCP_REQUEST_PRIMITIVE))
        && matches!(arguments.len(), 2 | 3)
}

/// Whether `function`/`arguments` is a call to `Connection`'s `@read` primitive — the
/// member-call form, either `connection.@read()` (just the receiver) or
/// `connection.@read(seconds)` (the receiver plus the timeout overload's one explicit
/// argument) — both deferred the same way, so both need the taint and the block-scope join.
fn is_connection_read_call(
    function: &Expression,
    arguments: &[Expression],
    member_call: bool,
) -> bool {
    member_call
        && (arguments.len() == 1 || arguments.len() == 2)
        && matches!(function, Expression::Identifier { name, .. }
            if at_primitive_name(name) == Some(CONNECTION_READ_PRIMITIVE))
}

/// Whether `function`/`arguments` is a call to a value-returning `@` primitive — one that hands
/// back a DEFERRED value the taint must track: `@readStdin()` (a deferred `Text` line),
/// `@tcpRequest(addr, req)` (a deferred `Result`), or `connection.@read()` (a deferred `Text`).
/// Matched on name AND arity, so a call that does not fit the primitive's signature is not
/// treated as deferred. Effect-only primitives like `@sleep` (which yields `$`) are never
/// deferred and so never appear here.
fn produces_deferred(function: &Expression, arguments: &[Expression], member_call: bool) -> bool {
    is_read_call(function, arguments)
        || is_tcp_request_call(function, arguments)
        || is_connection_read_call(function, arguments, member_call)
}

/// Whether `function`/`arguments` is a call to the named `@`-marked server primitive
/// (`primitive_name`, one of [`TCP_SERVE_PRIMITIVE`]/[`HTTP_SERVE_PRIMITIVE`]) at its own
/// [`SERVE_ARITY`] — the one shape both `net.@tcpServe(address, handler)` and
/// `http.@serve(address, handler)` share, so one function answers for either.
fn is_serve_call(function: &Expression, arguments: &[Expression], primitive_name: &str) -> bool {
    matches!(function, Expression::Identifier { name, .. }
        if at_primitive_name(name) == Some(primitive_name))
        && arguments.len() == SERVE_ARITY
}

/// Whether `function`/`arguments` launches work that keeps running in the background after
/// the call itself returns, so the enclosing `< >` block must join it before its own value
/// flows out — every deferred-producing call (its producer fiber), plus `@tcpServe`/`@serve`
/// (their accept loop): neither call's own return value, the `Server` handle, is ever
/// deferred, but the accept loop each starts keeps running after the call returns, so it
/// registers with the block's launch scope the same way a value-returning launch's producer
/// does (`crate::launch_scope::register`, called directly from the runtime intrinsic here
/// rather than through the deferred-value taint this pass otherwise tracks).
fn launches_in_background(
    function: &Expression,
    arguments: &[Expression],
    member_call: bool,
) -> bool {
    produces_deferred(function, arguments, member_call)
        || is_serve_call(function, arguments, TCP_SERVE_PRIMITIVE)
        || is_serve_call(function, arguments, HTTP_SERVE_PRIMITIVE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser;

    /// Every test below except the atomic-reassignment ones is about plain taint/force-set
    /// behavior, unaffected by which reassignments (if any) are atomic — an empty set is
    /// exactly equivalent to "none are". None of these needs the checker at all, matching
    /// how they worked before the rule existed.
    fn info(src: &str) -> DeferInfo {
        let tokens = Lexer::tokenize(src).expect("lex");
        let program = parser::parse(&tokens).expect("parse");
        analyze(&program, &HashSet::new(), &HashSet::new())
            .expect("expected no atomic-reassignment violation")
    }

    /// The number of force sites in the program — the size of the force-set.
    fn force_count(src: &str) -> usize {
        info(src).force_sites.len()
    }

    /// Lex, parse, link, and check `src`, then run `analyze` with the checker's own
    /// atomic-reassignment and top-level-reassignment spans — what a global-tracking or
    /// atomic-reassignment test needs, since (unlike plain force-counting) telling which
    /// reassignment is atomic or top-level is the checker's job, not this pass's. `src`
    /// must check cleanly: every `@` primitive is reached through its module binding
    /// (`io.@readStdin()`), matching what user code actually writes.
    fn checked(src: &str) -> Result<DeferInfo, AtomicReassignmentForced> {
        let tokens = Lexer::tokenize(src).expect("lex");
        let program = parser::parse(&tokens).expect("parse");
        let (program, _sources) = crate::modules::link(program, std::path::Path::new("."), None)
            .expect("import linking failed");
        let mut checker = crate::typechecker::TypeChecker::new();
        checker
            .check_program(&program)
            .expect("type checking failed");
        let atomic_reassignments = checker.take_atomic_reassignments();
        let top_level_reassignments = checker.take_top_level_reassignments();
        analyze(&program, &atomic_reassignments, &top_level_reassignments)
    }

    /// The number of force sites in a checked program expected to raise no violation.
    fn checked_force_count(src: &str) -> usize {
        checked(src)
            .expect("expected no atomic-reassignment violation")
            .force_sites
            .len()
    }

    /// The atomic-reassignment violation a checked program is expected to raise.
    fn atomic_violation(src: &str) -> AtomicReassignmentForced {
        checked(src).expect_err("expected an atomic-reassignment violation")
    }

    #[test]
    fn pure_program_has_no_force_sites() {
        let i = info("^ = () -> Num => < 1 + 2 * 3 >");
        assert_eq!(i.force_sites.len(), 0);
    }

    #[test]
    fn effect_only_sleep_is_never_a_deferred_value() {
        // `@sleep` returns `$`, not a value: it is never deferred and never forced.
        let i = info("<< core.time\n^ = () -> $ => <\n  time.@sleep(1)\n  $\n>");
        assert_eq!(i.force_sites.len(), 0);
    }

    #[test]
    fn bound_read_is_deferred_and_forced_at_a_strict_use() {
        // `x = io.@readStdin()` binds a deferred Text (lazy); the comparison forces it once.
        let src = "<< core.io\n^ = () -> Num => <\n  x = io.@readStdin()\n  x == \"hi\" ? 0 : 1\n>";
        // Exactly one force: the `x` read inside the comparison. The binding stays lazy.
        assert_eq!(force_count(src), 1);
    }

    #[test]
    fn read_directly_in_a_strict_slot_forces_at_the_call() {
        // No binding: the `io.@readStdin()` value is consumed strictly (compared) right away.
        let src = "<< core.io\n^ = () -> Num => < io.@readStdin() == \"hi\" ? 0 : 1 >";
        assert_eq!(force_count(src), 1);
    }

    #[test]
    fn read_passed_to_a_call_forces_at_the_argument() {
        let src = "<< core.io\n^ = () -> Num => <\n  x = io.@readStdin()\n  print(x)\n  0\n>";
        // The `print(x)` argument is a strict slot: one force.
        assert_eq!(force_count(src), 1);
    }

    #[test]
    fn a_bound_but_unused_read_is_not_forced() {
        // Launched (eager) but never read strictly: no force site. The launch still runs.
        let src = "<< core.io\n^ = () -> Num => <\n  x = io.@readStdin()\n  0\n>";
        assert_eq!(force_count(src), 0);
    }

    #[test]
    fn read_flows_lazily_through_a_second_binding() {
        let src = "<< core.io\n^ = () -> Num => <\n  x = io.@readStdin()\n  y = x\n  y == \"hi\" ? 0 : 1\n>";
        // Two lazy bindings, forced once at the comparison.
        assert_eq!(force_count(src), 1);
    }

    #[test]
    fn a_bare_read_at_the_root_is_still_deferred() {
        // A root program's own names stay bare — qualify renames nothing there — mirroring
        // `corelib/io.qn`'s own body, which calls `@readStdin` bare. `at_primitive_name` must
        // recognize the bare spelling too, or a root-level (or corelib-internal) call would
        // silently stop being tracked as a deferred producer while codegen still launches it.
        let src = "^ = () -> Num => < @readStdin() == \"hi\" ? 0 : 1 >";
        assert_eq!(force_count(src), 1);
    }

    #[test]
    fn a_qualified_read_call_is_recognized_as_a_launch() {
        // `io.@readStdin()` is the call form every importer now writes — `is_force_site`/
        // `is_launch_scope` key off `at_primitive_name`, not the literal spelling, so the
        // qualified form must be tracked exactly like the bare one: force-set membership
        // AND the enclosing block's own launch-scope membership, or the scope join would
        // silently stop being emitted for every ordinary (imported) call site.
        let src = "<< core.io\n^ = () -> Num => <\n  x = io.@readStdin()\n  x == \"hi\" ? 0 : 1\n>";
        let i = info(src);
        assert_eq!(
            i.force_sites.len(),
            1,
            "the qualified read must still be forced at the comparison"
        );
        assert_eq!(
            i.launch_scopes.len(),
            1,
            "the qualified read must still mark its enclosing block as a launch scope"
        );
    }

    #[test]
    fn bound_tcp_request_is_deferred_and_forced_at_a_strict_use() {
        // `r = net.@tcpRequest(...)` binds a deferred Result (lazy); the match forces it
        // once — the same shape as a bound `io.@readStdin`, proving the taint tracks both
        // producers.
        let src = "<< core.net\n^ = () -> Num => <\n  r = net.@tcpRequest(\"a:1\", \"b\")\n  r ? | Ok(_) => 0 | NotOk(_) => 1\n>";
        assert_eq!(force_count(src), 1);
    }

    #[test]
    fn tcp_request_with_wrong_arity_is_not_deferred() {
        // A `@tcpRequest` reference that does not fit the primitive's two-argument signature is
        // not treated as a deferred producer: no value flows out deferred, so nothing is forced.
        let src = "<< core.net\n^ = () -> Num => <\n  r = net.@tcpRequest(\"a:1\")\n  0\n>";
        assert_eq!(force_count(src), 0);
    }

    #[test]
    fn bound_tcp_request_with_options_is_deferred_and_forced_at_a_strict_use() {
        // The three-argument `ConnectOptions` overload is the same producer under the taint:
        // still deferred, still forced only where a strict use reads it.
        let src = "<< core.net\n^ = () -> Num => <\n  o = net.ConnectOptions.default()\n  r = net.@tcpRequest(\"a:1\", \"b\", o)\n  r ? | Ok(_) => 0 | NotOk(_) => 1\n>";
        assert_eq!(force_count(src), 1);
    }

    #[test]
    fn bound_connection_read_is_deferred_and_forced_at_a_strict_use() {
        // `line = connection.@read()` binds a deferred Text (lazy) through the member-call
        // form, exactly like the module-qualified primitives above; the comparison forces it.
        let src = "^ = () -> Num => <\n  line = c.@read()\n  line == \"\" ? 0 : 1\n>";
        assert_eq!(force_count(src), 1);
    }

    #[test]
    fn bound_connection_read_with_a_timeout_is_deferred_and_forced_at_a_strict_use() {
        // `connection.@read(seconds)` — the timeout overload — is deferred exactly like the
        // zero-argument form: the block launching it still needs its own launch scope, or
        // it returns (and the runtime auto-closes the connection) before the background
        // read ever runs.
        let src = "^ = () -> Num => <\n  line = c.@read(5)\n  line == \"\" ? 0 : 1\n>";
        assert_eq!(force_count(src), 1);
    }

    #[test]
    fn a_bare_at_read_with_no_receiver_is_not_a_connection_read() {
        // `@read` reached bare (no `.` receiver, not a member call) fits no known primitive's
        // shape — a value that happens to be named `@read` is not this pass's concern, since
        // only the corelib may declare an `@` name in the first place.
        let src = "^ = () -> Num => <\n  line = @read()\n  0\n>";
        assert_eq!(force_count(src), 0);
    }

    #[test]
    fn a_block_that_calls_tcp_serve_is_a_launch_scope_though_its_return_is_not_deferred() {
        // `net.@tcpServe`'s own return value (the `Server` handle) is a ready value, never
        // forced — but the accept loop it starts keeps running after the call returns, so the
        // enclosing block still opens and joins a launch scope for it.
        let src =
            "<< core.net\n^ = () -> Num => <\n  server = net.@tcpServe(\"127.0.0.1:0\", h)\n  0\n>";
        let i = info(src);
        assert_eq!(i.launch_scopes.len(), 1);
        assert_eq!(i.force_sites.len(), 0);
    }

    #[test]
    fn a_block_that_calls_http_serve_is_a_launch_scope_though_its_return_is_not_deferred() {
        // `http.@serve` lowers to the same runtime entry `net.@tcpServe` does, so it must be
        // recognized as a background launch the same way, though nothing here actually
        // imports `core.http` (this pass reads no types, so the bare primitive name alone
        // is what it keys on).
        let src = "^ = () -> Num => <\n  server = @serve(\"127.0.0.1:0\", h)\n  0\n>";
        let i = info(src);
        assert_eq!(i.launch_scopes.len(), 1);
        assert_eq!(i.force_sites.len(), 0);
    }

    #[test]
    fn read_through_a_ternary_arm_forces_at_the_result_use() {
        // Ternary arms are lazy carriers: the deferred value survives the `?` and is forced
        // where the ternary's result is used strictly (the outer comparison).
        let src = "<< core.io\n^ = () -> Num => <\n  x = io.@readStdin()\n  chosen = true ? x : \"z\"\n  chosen == \"hi\" ? 0 : 1\n>";
        assert_eq!(force_count(src), 1);
    }

    #[test]
    fn a_block_that_directly_launches_is_a_launch_scope() {
        let src = "<< core.io\n^ = () -> Num => <\n  x = io.@readStdin()\n  0\n>";
        assert_eq!(info(src).launch_scopes.len(), 1);
    }

    #[test]
    fn a_server_address_call_is_not_a_launch_scope() {
        // `Server.address()` is a plain, synchronous member — reading where a server bound,
        // not launching anything — so it must not be mistaken for `@tcpServe`/`@serve`'s
        // own launch registration, nor be treated as a deferred-value producer.
        let src = "^ = () -> Num => <\n  a = server.address()\n  0\n>";
        let i = info(src);
        assert!(i.launch_scopes.is_empty());
        assert_eq!(i.force_sites.len(), 0);
    }

    #[test]
    fn a_pure_block_is_not_a_launch_scope() {
        let i = info("^ = () -> Num => < 1 + 2 * 3 >");
        assert!(i.launch_scopes.is_empty());
    }

    #[test]
    fn a_trap_arms_own_deferred_body_is_forced() {
        let src = "!> | Interrupt(s) => @readStdin()\n^ = () -> Num => < 0 >";
        assert_eq!(info(src).force_sites.len(), 1);
    }

    #[test]
    fn a_trap_arm_that_never_forces_has_no_force_site() {
        let src = "!> | Interrupt(s) => 1\n^ = () -> Num => < 0 >";
        assert_eq!(info(src).force_sites.len(), 0);
    }

    #[test]
    fn a_launch_inside_a_nested_block_scopes_to_that_block_only() {
        // The outer block launches nothing directly; the launch belongs to the inner block.
        let src = "<< core.io\n^ = () -> Num => <\n  helper = () => <\n    io.@readStdin()\n    0\n  >\n  helper()\n>";
        let i = info(src);
        assert_eq!(i.launch_scopes.len(), 1);
        // The outer function body itself must not be the marked scope.
        let program = {
            let tokens = Lexer::tokenize(src).expect("lex");
            parser::parse(&tokens).expect("parse")
        };
        let Item::FunctionDeclaration(entry) = &program.items[0] else {
            panic!("expected the entry function");
        };
        assert!(
            !i.is_launch_scope(entry.body.span()),
            "the outer block launches nothing directly"
        );
    }

    #[test]
    fn stream_file_result_is_never_a_deferred_value() {
        // `@streamFile` runs on the calling fiber: it is not in `produces_deferred`, so binding
        // its result produces no force site of its own — the match on it needs no force,
        // because it was never lazy to begin with.
        let src = "<< core.io\n^ = () -> Num => <\n  r = io.@streamFile(\"f\", 10, chunk => true)\n  r ? | Ok(_) => 0 | NotOk(_) => 1\n>";
        assert_eq!(force_count(src), 0);
    }

    #[test]
    fn a_deferred_argument_passed_to_stream_file_is_forced_at_the_argument() {
        // `@streamFile` is an ordinary call as far as its own arguments go: a deferred `Text`
        // flowing into its `path` argument is forced there, the same as any other call's
        // strict argument slot.
        let src = "<< core.io\n^ = () -> Num => <\n  p = io.@readStdin()\n  io.@streamFile(p, 10, chunk => true)\n  0\n>";
        assert_eq!(force_count(src), 1);
    }

    #[test]
    fn atomic_reassignment_forcing_a_deferred_value_is_rejected() {
        // The maintainer's own example: a top-level atomic global, forced on the right side
        // of its reassignment from a separate function.
        let src = "<< core.io\n@hits := 0\n>> bump = () -> $ => < hits := hits + io.@readStdin().length >";
        assert_eq!(atomic_violation(src).name, "hits");
    }

    #[test]
    fn atomic_reassignment_directly_forcing_a_primitive_call_is_rejected() {
        let src = "<< core.io\n@hits := 0\n>> bump = () -> $ => < hits := io.@readStdin().length >";
        assert_eq!(atomic_violation(src).name, "hits");
    }

    #[test]
    fn a_top_level_atomic_reassignment_forcing_a_deferred_value_is_rejected() {
        // The same rule at the top level, not from inside a separate function.
        let src = "<< core.io\n@hits := 0\nhits := hits + io.@readStdin().length";
        assert_eq!(atomic_violation(src).name, "hits");
    }

    #[test]
    fn atomic_reassignment_reading_an_already_forced_binding_is_accepted() {
        // The accepted rewrite: force into a plain binding first, then reassign — the
        // reassignment's own right side reads an already-ready value.
        let src = "<< core.io\n@hits := 0\n>> bump = () -> $ => <\n  extra = io.@readStdin().length\n  hits := hits + extra\n>";
        assert_eq!(checked_force_count(src), 1);
    }

    #[test]
    fn a_plain_mutable_reassignment_may_force_a_deferred_value() {
        // The rule is atomic-binding-specific: an ordinary `:=` global forcing a deferred
        // value on its reassignment's right side is untouched.
        let src = "<< core.io\ncounter := 0\n>> bump = () -> $ => < counter := counter + io.@readStdin().length >";
        assert_eq!(checked_force_count(src), 1);
    }

    #[test]
    fn a_reassignment_after_a_plain_reassignment_of_the_same_atomic_binding_is_rejected() {
        // A `:=` declaration and a reassignment are the same AST node; only the checker
        // tells them apart (by whether the name is already bound), so a bare, non-forcing
        // reassignment earlier in the function must not make the checker (or this pass)
        // forget the binding stays atomic for the reassignment after it.
        let src = "<< core.io\n@hits := 0\n>> bump = () -> $ => <\n  hits := hits + 1\n  hits := hits + io.@readStdin().length\n>";
        assert_eq!(atomic_violation(src).name, "hits");
    }

    #[test]
    fn a_reassignment_of_an_atomic_global_by_a_same_named_local_looking_declaration_is_rejected() {
        // `counter := 0` inside `tally` reads as a fresh local at a glance, but Quilon has
        // no shadowing for a mutable name already in scope: it reassigns the top-level
        // `@counter` like any other `:=` on an existing name, so the forcing reassignment
        // after it is rejected the same as any other atomic reassignment.
        let src = "<< core.io\n@counter := 0\n>> tally = () -> Num => <\n  counter := 0\n  counter := counter + io.@readStdin().length\n  counter\n>";
        assert_eq!(atomic_violation(src).name, "counter");
    }

    #[test]
    fn an_atomic_reassignment_inside_a_lambdas_block_is_rejected() {
        // The checker resolves `hits` from inside the `.each` callback's block the same
        // way it would from any other nested scope, so the rule reaches a reassignment
        // however deeply it sits inside a lambda, not only a named function's own body.
        let src = "<< core.io\n@hits := 0\n>> bump = () -> $ => <\n  (1 <- 3).each(n => < hits := hits + io.@readStdin().length >)\n  $\n>";
        assert_eq!(atomic_violation(src).name, "hits");
    }

    #[test]
    fn a_global_stored_from_one_function_is_forced_at_a_read_in_another() {
        // The issue's own reproduction: `testimony` is stored a deferred value inside
        // `interrogate`, and read from a completely separate function, `^`. Without
        // cross-function tracking the read sees an unforced sentinel; with it, the read
        // itself is the one force site.
        let src = "<< core.io\ntestimony := \"\"\ninterrogate = () -> $ => < testimony := io.@readStdin() >\n^ = () -> Num => <\n  interrogate()\n  testimony == \"fact\" ? 0 : 1\n>";
        assert_eq!(checked_force_count(src), 1);
    }

    #[test]
    fn a_global_never_stored_a_deferred_value_has_no_force_sites_on_its_reads() {
        // `label` only ever holds a ready value; every read of it stays cost-free, exactly
        // as it did before this binding was tracked at all.
        let src = "<< core.io\nlabel := \"steady\"\nshout = () -> $ => < label := label + \"!\" >\n^ = () -> Num => <\n  shout()\n  label == \"steady!\" ? 0 : 1\n>";
        assert_eq!(checked_force_count(src), 0);
    }

    #[test]
    fn a_globals_deferredness_propagating_through_another_global_still_converges() {
        // `copy`'s own deferredness depends on `testimony`'s already being known
        // deferred: one round discovers `testimony`, a second discovers that `relay`'s
        // store into `copy` is therefore deferred too, and only then does a read of
        // `copy` see it. The fixed point still lands on exactly one force site — at the
        // read in `^`.
        let src = "<< core.io\ntestimony := io.@readStdin()\ncopy := \"\"\nrelay = () -> $ => < copy := testimony >\n^ = () -> Num => <\n  relay()\n  copy == \"fact\" ? 0 : 1\n>";
        assert_eq!(checked_force_count(src), 1);
    }

    #[test]
    fn an_atomic_bindings_own_deferred_initial_value_is_accepted_inside_a_function() {
        // The atomic-reassignment rule only ever applies to a REASSIGNMENT (a name
        // already bound): an atomic binding's own declaring occurrence is never one, so a
        // deferred right side is accepted — the maintainer's own top-level example, here
        // nested inside a function body, and a store, so it forces nothing either way.
        let src = "<< core.io\nlisten = () -> $ => < @testimony := io.@readStdin() >\n^ = () -> Num => <\n  listen()\n  0\n>";
        assert_eq!(checked_force_count(src), 0);
    }

    #[test]
    fn an_atomic_globals_reassignment_from_a_function_is_accepted_and_forces_at_the_read() {
        // The exact shape the `block_scope_join` example uses: `testimony` is DECLARED
        // atomic at the top level, then reassigned bare from `interrogate` with a
        // deferred value (accepted — the store's own right side forces nothing, so the
        // atomic-reassignment rule has nothing to reject) and read from `^`, a separate
        // function, which is where the one force site lands.
        let src = "<< core.io\n@testimony := \"\"\ninterrogate = () -> $ => < testimony := io.@readStdin() >\n^ = () -> Num => <\n  interrogate()\n  testimony == \"fact\" ? 0 : 1\n>";
        assert_eq!(checked_force_count(src), 1);
    }

    #[test]
    fn a_reassignment_calling_a_function_that_waits_is_rejected_transitively() {
        // The maintainer's own probe: `f` reads stdin and forces it (its own body's
        // `line.length`); `^`'s atomic reassignment never mentions `io.@readStdin` itself,
        // only calls `f()` — the wait is reached through a call, one hop away.
        let src = "<< core.io\n@a := 0\nf = () -> Num => <\n  line = io.@readStdin()\n  line.length\n>\n^ = () -> Num => <\n  a := a + f()\n  a\n>";
        let violation = atomic_violation(src);
        assert_eq!(violation.name, "a");
        assert_eq!(
            violation.transitive,
            Some(("f".to_string(), "core.io.@readStdin".to_string()))
        );
    }

    #[test]
    fn a_reassignment_calling_a_function_two_hops_from_the_wait_names_the_waiting_one() {
        // `^` calls `g`, `g` calls `f`, `f` reads stdin — the violation names `f`, the
        // function that ACTUALLY waits, not `g`, which merely forwards the call.
        let src = "<< core.io\n@a := 0\nf = () -> Num => <\n  line = io.@readStdin()\n  line.length\n>\ng = () -> Num => < f() >\n^ = () -> Num => <\n  a := a + g()\n  a\n>";
        let violation = atomic_violation(src);
        assert_eq!(violation.name, "a");
        assert_eq!(
            violation.transitive,
            Some(("f".to_string(), "core.io.@readStdin".to_string()))
        );
    }

    #[test]
    fn a_reassignment_calling_a_pure_function_is_accepted() {
        // A right side calling an ordinary function that itself waits on nothing — the
        // transitive rule has nothing to reject here.
        let src = "@a := 0\npure = (n :: Num) -> Num => < n + 1 >\n^ = () -> Num => <\n  a := a + pure(1)\n  a\n>";
        assert_eq!(checked_force_count(src), 0);
    }
}
