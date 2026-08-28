---
title: "Known limitations"
sidebar:
  order: 2
---

# Known limitations

0.9 is a stable **core**, not the whole language. Notably:

- **No generics.** Overloading (ad-hoc, exact-type dispatch) is the only polymorphism; there are no type variables — which is why the [matchers](../corelib/test/README.md#the-matchers) are compiler-provided rather than written in `.qn`. The module system is minimal (`core.io`/`core.test` built-ins + file-path imports).
- **Closures are monomorphic.** Lexical capture works end-to-end (`=` by value / `:=` by reference; see [Closures](../functions/closures.md#closures--capture-by--value-vs--reference)), including recursion of non-capturing nested functions, capture across nesting levels, and capturing-then-calling another closure. A closure can also be passed to a [function-typed parameter](../functions/README.md#function-types--higher-order-functions) and called there. Deferred (each needs the closure's type threaded through inference): capturing a *polymorphic* value, *generic* closures, and **returning** a closure from a function.
- **Overloaded and top-level function names are not first-class values.** A closure is passed as a *lambda literal* or a *named closure binding*; passing a top-level function or an overloaded name as a value is not yet supported.
- **Sum-type payloads mixing types across variants aren't unified yet.** Distinct payload *types* per slot across variants (a position that is `Num` in one variant and `Text` in another) is deferred; the payload set (`Num`/`Text`/`Bool`/`$` and a named record, consistent per position) works.
- **A pattern tests one level.** A constructor pattern's payload sub-pattern must be irrefutable — a binding (`Ok(x)`) or `_` (`Ok(_)`) — because dispatch reads the variant tag alone; compare the bound payload in the arm body instead. There are no `Text` or `Bool` patterns, so a match on either is covered by a catch-all arm.
- **A named-composite sum payload must be a record, and a record field cannot yet be a named composite.** A variant may carry a named **record** (`Post(Body)`), but not another named **sum**; and a record field is still limited to built-in types and arrays (a `{ inner :: Inner }` field of a user type is a deferred follow-up).
- **Ranges are materialized eagerly.** `lo <- hi` builds the whole `[]Num` up front, so it costs 8 bytes per element — `1 <- 100000000` is ~800 MB, not a counter. With no loop construct, "do this N times" over a large N has no scalable encoding yet; a lazy range the [array methods](../collections/arrays.md#array-methods) consume without materializing is the intended fix. (Endpoints themselves are checked: see [ranges](../expressions/ranges-and-spread.md#endpoints-must-be-whole-numbers).)
- **Concurrency is partly built.** The [model](../concurrency/README.md) is locked; the fiber scheduler, reactor, `@sleep`, and the deferred-value primitives (`@readStdin`, `@tcpRequest`) run. Remaining for 1.0: overlap as a showcase, deferred composites, further `@` primitives (file), and multicore M:N.
