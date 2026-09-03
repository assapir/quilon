---
title: "core.info — Build facts"
sidebar:
  label: "core.info"
  order: 5
---

# `core.info` — Build facts

Import with `<< core.info`. See the [corelib index](README.md) and `examples/info.qn`.

What a program can ask about itself. Each answer is fixed when the program is compiled, and
each closed set of answers is a [sum type](../types/sum-types.md): a match over one is
exhaustive and a misspelt variant is a compile error.

| Function | Result |
|----------|--------|
| `info.platform() -> Platform` | `Aarch64` / `X86_64` / `X86` / `Arm` / `Riscv64` / `Wasm32` / `WtfPlatform(Text)` |
| `info.os() -> Os` | `Linux` / `MacOS` / `Windows` / `FreeBSD` / `OpenBSD` / `NetBSD` / `WtfOs(Text)` |
| `info.pointerWidth() -> PointerWidth` | `Width64` / `Width32` |
| `info.endianness() -> Endianness` | `Little` / `Big` |
| `info.runMode() -> RunMode` | `Aot` / `Jit` — whether this program was built ahead of time or is running through `quilon run` |
| `info.quilonVersion() -> Text` | the compiler that built it, e.g. `"0.9.3"` — an open set, so a `Text` |

Every type renders, so it interpolates with no conversion:

```quilon
<< core.io
<< core.info

^ = () -> Num => <
  io.print("`info.platform()`-`info.os()` `info.pointerWidth()` `info.endianness()`-endian, quilon `info.quilonVersion()`")
  0
>
```

And every type is matchable, which is the point of the sums:

```quilon
<< core.info

^ = () -> Num => <
  info.os() ?
    | info.Linux   => 0
    | info.MacOS   => 0
    | info.Windows => 1
    | _       => 2
>
```

## Methods

| Method | On | Result |
|--------|----|--------|
| `name()` | all five | the spoken name: `"aarch64"`, `"macOS"`, `"64-bit"`, `"little"`, `"aot"` |
| `bits()` | `PointerWidth` | `64` or `32`, as a `Num` |

Every type also defines `` ` ``, delegating to `name()`, so all five interpolate the same way.

## What these mean

**The target.** The answers describe the machine the program **runs** on. For an ordinary
build that is the machine that built it; a cross-compiled binary reports its target.

**`WtfPlatform` and `WtfOs` say which.** They carry the raw text the compiler saw — the
architecture, and the whole target triple — so a target outside the named variants
reports what it is. `name()` returns it.

**Common names.** `Os` has a `MacOS` variant; the target triple's own spelling stays inside
`WtfOs`.

**`PointerWidth` and `Endianness` come from LLVM's data layout** for the target:
`powerpc64le` and `mips64el` report little-endian, and `s390x` reports 64-bit.

**`runMode()` describes the execution.** It reports how the program is being executed:
`Jit` under `quilon run`, `Aot` for a binary from `quilon build`. `quilon compile` emits the
IR an ahead-of-time build would, and reports `Aot` too.

**The import is required.** The types and functions are Quilon declared in this module;
`<< core.info` brings them into scope.
