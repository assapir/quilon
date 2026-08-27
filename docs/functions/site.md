---
title: "Call-site locations — Site"
---

# Call-site locations — `Site`

A function whose **last** parameter is a `Site` receives the location of the call — and a
call that leaves that argument off has it **filled in by the compiler**:

```quilon
whereAmI = (site :: Site) -> Text => "`site.file`:`site.line`:`site.column`"

^ = () -> $ => <
  print(whereAmI())        ~ prints e.g. demo.qn:4:9 — the location of THIS call
>
```

`Site` is a built-in record type, nameable in any signature with **no import**:

| Field | Type | Is |
|---|---|---|
| `file` | `Text` | the call's file, as the compiler resolved it |
| `line` | `Num` | 1-based line of the call |
| `column` | `Num` | 1-based column, in characters |
| `excerpt` | `Text` | the text of that line, without its newline |
| `width` | `Num` | how many characters of the line the call spans |

`line`, `column`, and `width` are always at least 1. `Site` is a built-in type name, so a
program cannot declare its own (as with `Result`). A program may still *build* one
(`Site { file = "…", line = 1, column = 1, excerpt = "…", width = 1 }`) and pass it on;
`failAt` will report wherever it says.

**A `Site` is read-only.** A location is a value, not a variable. Writing one of its fields
(`site.line := 9`) is a compile error however the value was reached: records alias, so a
write through a `:=` rebinding would write the same thing.

**Passing one explicitly forwards it.** That is the whole propagation rule. It makes a
chain of wrappers report the *user's* call rather than the innermost hop — Rust's
`#[track_caller]`, as an ordinary argument:

```quilon
inner = (site :: Site) -> Num => site.line
outer = (site :: Site) -> Num => inner(site)   ~ forwards: reports where `outer` was called
plain = (site :: Site) -> Num => inner()       ~ does not: reports THIS line
```

Only a **top-level function's last** parameter can be filled in. A `Site` anywhere else is a
compile error rather than an argument nothing supplies: not before another parameter, not on
a lambda or nested declaration (called through a value, not by name), and not on a record
method (dispatched by receiver type). The arity a caller sees never counts it — `whereAmI()`
above takes no arguments at the call site.

Filling one in **costs nothing at run time**: the location is known at compile time, so the
call allocates nothing. `quilon run` and a native build report identically. Assert as often
as you like, in the hottest loop you have. (A site does cost a little binary size.)
[`failAt`](../corelib/test/README.md#building-a-check-of-your-own) is built on this; nothing
about it is specific to them. See `examples/call_site.qn`.
