---
title: "Quilon Language Reference"
---

# Quilon Language Reference

**Version:** 0.10.0 — "Demosthenes" (stable basics — the core is verified end-to-end; the deferred parts are listed under [Known limitations](status/limitations.md)).

Quilon is a statically-typed, **symbol-based** language (no control-flow keywords) that compiles to native code via LLVM. Every example in this reference has a passing end-to-end test. Each `examples/*.qn` program is **self-asserting**: it verifies its own results in-language with `assert(value, matcher)` and exits 0 (a failing assertion aborts with exit 101), under both the JIT (`quilon run`) and native AOT.

## Design principles

Quilon's identity, and the rules that guide its design:

- **No keywords.** Every construct is punctuation. Branching is `?` / `|`, the entry point is `^`, import/export are `<<` / `>>`, mutability is `:=`, sum-type alternatives are `/`. Every word is an ordinary identifier: `if`, `while`, `for` and the rest may be bound.
- **Symbols mirror notation that already exists.** A symbol reuses a notation the world already has: `/` separates sum-type alternatives the way "red / green / blue" is written.
- **The playful choice wins.** On a toss-up, the more delightful option is picked — `^` for the entry point, `$` for Unit.
- **Deliberate simplicity.** The smallest system that works: ad-hoc overloading is the polymorphism, iteration is array methods and recursion, and `Num` is the one number type.
- **Fail loud.** An invalid input or a meaningless operation *fails*. A statically-determinable problem is a **compile error**; anything else is a runtime error on stderr with a non-zero exit, saying [where it happened](tooling/errors.md). `Text.indexOf` yields `Ok(Num)`/`NotOk`, and `Text.replace`'s count and empty-argument checks fail.
- **No magic.** Overloads are exact-typed and operators mean what they say; a value converts through an explicit operation.
- **Immutable by default.** `=` binds immutably, `:=` binds mutably — for variables, for record bindings, and for methods: a method declared `name := …` may mutate its receiver, and one declared `name = …` is verified non-mutating.
- **Errors are values.** A fallible operation returns `Ok` / `NotOk`, a normal sum type.
- **Library APIs hide internals.** A library performs its own conversion and desugaring (`print(x)`).

## Symbols

| Symbol | Meaning | Example |
|--------|---------|---------|
| `=` | Immutable binding | `x = 42` |
| `:=` | Mutable bind / reassign / in-place field write | `counter := 0`, `obj.field := v` |
| `::` | Type annotation | `x :: Num` |
| `=>` | Function body / match arm | `f = (x :: Num) => x + 1` |
| `->` | Return type; also a [function type](functions/README.md#function-types--higher-order-functions) | `f = (x :: Num) -> Num => x` · `(Num) -> Bool` |
| `+` `-` `*` `/` `%` | [Arithmetic](expressions/README.md) (`-x` negates) | `a + b` · `x % 2` |
| `==` `!=` `<` `<=` `>` `>=` | [Comparison](expressions/README.md) → `Bool` · `==`/`!=` over `Num`/`Text`/`Bool`, ordering over `Num`/`Text` | `a == b` · `x <= 3` |
| `&&` `\|\|` `!` | Logical and / or / not (short-circuit) | `a && !b` |
| `< >` | Block delimiters · every function and method body is one · also `<`/`>` comparison ([rule](expressions/README.md)) | `< a b a + b >` · `a < b` · `a > b` |
| `^` | Entry point (main) | `^ = () -> Num => < 0 >` |
| `$` | Unit type **and** its sole value | `f = () -> $ => < $ >` |
| `<<` | Import a module | `<< core.io` |
| `>>` | Export an item from a module | `>> add = (a :: Num, b :: Num) => a + b` |
| `<-` (infix) | Inclusive range → `[]Num` | `1 <- 4` ≡ `[1,2,3,4]` · `4 <- 1` ≡ `[4,3,2,1]` |
| `<-` (prefix) | Spread inside a `[ ]` / `{ }` literal ([rule](expressions/ranges-and-spread.md#spread-in-literals)) | `[<-xs, 4]` · `{<-p, x = 9}` · `Vec {<-p, x = 9}` |
| `?` `\|` `_` | Pattern match | `v ? \| 0 => "zero" \| _ => "other"` |
| `/` | Division **or** sum-type variant separator | `a / b` · `Color = Red / Green` |
| `[\| \|]` | [Map](collections/README.md#maps) / [Set](collections/README.md#sets) pipe fence (`=>` = "maps to") | `[\|"a" => 1\|]` (map) · `[\|1, 2\|]` (set) |
| `+-` `-+` | [Set intersection](collections/README.md#sets) (one symmetric operator) | `a +- b` ≡ `a -+ b` |
| `` ` `` (in a string) | [Interpolation](types/text.md#string-interpolation-and-the-render-operator-) hole · `` `` `` = one literal backtick | `` "hi `user.name`" `` |
| `` ` `` (as a name) | The overloadable **render** operator — a type's `Text` rendering | `` ` = () -> Text => "..." `` |
| `? :` | Ternary | `x < 0 ? -x : x` |
| `@` (name prefix) | A [leaf IO primitive](concurrency/README.md), declared in corelib and called from user code | `@sleep(1)` |
| `~` | Comment (to end of line) | `~ a note` |

There are **no keywords**: `if`/`return` etc. are expressed with symbols, and iteration is
[array methods and recursion](expressions/iteration.md). Every word is free to bind:
`if = 5` and a function named `while` are legal.

## Contents

- Types: [overview](types/README.md) (`Num`, `Bool`, `$`) · [Text](types/text.md) · [records](types/records.md) · [sum types](types/sum-types.md)
- [Collections](collections/README.md): [arrays](collections/arrays.md), maps, and sets
- [Variables](variables.md) · [Mutation](mutation.md)
- Functions: [basics](functions/README.md) · [closures and tail recursion](functions/closures.md) · [overloading](functions/overloading.md) · [call-site locations](functions/site.md)
- Expressions: [operators and blocks](expressions/README.md) · [iteration](expressions/iteration.md) · [ranges and spread](expressions/ranges-and-spread.md) · [pattern matching](expressions/pattern-matching.md)
- Modules: [imports and exports](modules/README.md) · [entry point](modules/entry-point.md)
- [Corelib](corelib/README.md): the standard library, module by module
- [Concurrency](concurrency/README.md) · [its runtime](concurrency/runtime.md)
- [Memory](memory.md)
- Tooling: [compiling & running](tooling/compiling.md) · [error messages](tooling/errors.md) · [language server](tooling/language-server.md)
- Status: [feature matrix](status/feature-matrix.md) · [known limitations](status/limitations.md) · [compiler architecture](status/architecture.md) · [ABI and calling convention](status/abi.md)

## Writing style

This reference states what Quilon **is** — positive, normative facts about current
behavior. No rationale for a design (`because…`, `the point is…`), no comparison
with other languages (`unlike Rust…`, `rather than a for loop…`), no history
(`used to…`, `no longer…`, `still…`), and no hedging (`just`, `simply`, `note
that…`). A statement of what the language does not do belongs only where it
defines behavior (e.g. "there are no keywords").

[Vale](https://vale.sh) enforces this over `docs/**/*.md` and
`editors/vscode/README.md` (`docs/ORCHESTRATION.md`, `docs/ROADMAP.md`, and
`docs/status/**` are planning/status pages and carry decisions and gaps by
nature, so they're exempt) — run it locally with:

```bash
vale docs
```

The rules live in `.vale/styles/Quilon/`, one file per category. A term that's
a legitimate exception (a fixed piece of vocabulary, not a violation) goes in
`.vale/styles/config/vocabularies/Quilon/accept.txt`; a one-off exception for a
single sentence uses Vale's inline markup instead (`<!-- vale Quilon.Hedging =
NO -->` … `<!-- vale Quilon.Hedging = YES -->`) — see the comment at the top of
`accept.txt` for both.
