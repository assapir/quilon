---
title: "Call-site locations — Site"
sidebar:
  label: "Call-site locations"
---

# Call-site locations — `Site`

A function whose **last** parameter is a `Site` receives the location of the call — and a
call that leaves that argument off has it **filled in by the compiler**:

```quilon
<< core.io

whereAmI = (site :: Site) -> Text => < "`site.file`:`site.line`:`site.column`" >

^ = () -> $ => <
  io.print(whereAmI())        ~ prints e.g. demo.qn:4:9 — the location of THIS call
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

`line`, `column`, and `width` are at least 1. `Site` is a built-in type name, reserved like
`Result`. A program may *build* one
(`Site { file = "…", line = 1, column = 1, excerpt = "…", width = 1 }`) and pass it on;
`failAt` reports wherever it says.

**A `Site` is read-only.** Writing one of its fields (`site.line := 9`) is a compile error
however the value was reached, a `:=` rebinding included.

**Passing one explicitly forwards it.** That is the whole propagation rule: a chain of
wrappers that forwards its `Site` reports the *user's* call.

```quilon
inner = (site :: Site) -> Num => < site.line >
outer = (site :: Site) -> Num => < inner(site) > ~ forwards: reports where `outer` was called
plain = (site :: Site) -> Num => < inner() >   ~ fills in: reports THIS line
```

A **top-level function's last** parameter is the position the compiler fills in. A `Site`
in any other position — before another parameter, on a lambda or nested declaration, or on a
record method — is a compile error. The arity a caller sees excludes it — `whereAmI()`
above takes no arguments at the call site.

Filling one in **costs nothing at run time**: the location is known at compile time, and
the call allocates nothing. `quilon run` and a native build report identically. A site
occupies binary size.
[`failAt`](../corelib/test/README.md#building-a-check-of-your-own) is built on this
mechanism. See `examples/call_site.qn`.
