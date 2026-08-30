---
title: "core.info — Build facts"
sidebar:
  label: "core.info"
  order: 5
---

# `core.info` — Build facts

Import with `<< core.info`. See the [corelib index](README.md) and `examples/info.qn`.

Facts about the build a program came from. Every member is a **compile-time constant** the
compiler bakes into the binary, so a call costs nothing at run time, reads no machine state,
and makes no syscall.

| Function | Result |
|----------|--------|
| `platform() -> Text` | The CPU architecture the program was built for: `"aarch64"`, `"x86_64"`. |
| `os() -> Text` | The operating system it was built for: `"linux"`, `"macOS"`, `"windows"`, `"FreeBSD"`, `"OpenBSD"`, `"NetBSD"`, else `"unknown"`. |
| `bits() -> Num` | Pointer width: `64` or `32`. |
| `endianness() -> Text` | Byte order: `"little"` or `"big"`. |
| `quilonVersion() -> Text` | The version of the compiler that built it, e.g. `"0.9.3"`. |

```quilon
<< core.io
<< core.info

^ = () -> Num => <
  print("`platform()`-`os()` `bits()`-bit `endianness()`-endian, quilon `quilonVersion()`")
  0
>
```

## What these mean

**Target, not host.** The architecture and OS describe the machine the program will **run**
on. For an ordinary build those are the same machine, but a cross-compiled binary reports its
target — which is the useful answer, since the program is the thing doing the asking.

**Names people use, not triple spellings.** `os()` answers `"macOS"`, never `"darwin"`. A
target triple is a build-system detail, and a program printing its own environment should
print something a reader recognises.

**`bits()` and `endianness()` come from LLVM's data layout**, not from the architecture's
name — which is a poor guide, since `powerpc64le` and `mips64el` are little-endian despite
their spelling, and `s390x` is 64-bit without saying so.

**Constants, not queries.** These are resolved while the program is compiled, so they cannot
change while it runs and cost nothing to read. `quilon compile` shows it — the calls do not
appear in the emitted IR at all, only the strings they became. That also makes them safe
anywhere, including inside a hot loop.

They are [overload sets](../functions/overloading.md) like `now`, so defining `platform` with
a different signature of your own adds a member rather than shadowing the built-in.

Like `core.time`, the import is documentation-only: the members are compiler-provided, so they
resolve with or without it. Writing `<< core.info` states the intent.
