---
title: "core.info — Build facts"
sidebar:
  label: "core.info"
  order: 5
---

# `core.info` — Build facts

Import with `<< core.info`. See the [corelib index](README.md) and `examples/info.qn`.

What a program can ask about itself. Each answer is fixed when the program is compiled, and
each closed set of answers is a [sum type](../types/sum-types.md) rather than a `Text`, so a
match over one is exhaustive and a typo is a compile error.

| Function | Result |
|----------|--------|
| `info.platform() -> Platform` | `Aarch64` / `X86_64` / `X86` / `Arm` / `Riscv64` / `Wasm32` / `WtfPlatform(Text)` |
| `info.os() -> Os` | `Linux` / `MacOS` / `Windows` / `FreeBSD` / `OpenBSD` / `NetBSD` / `WtfOs(Text)` |
| `info.pointerWidth() -> PointerWidth` | `Width64` / `Width32` |
| `info.endianness() -> Endianness` | `Little` / `Big` |
| `info.runMode() -> RunMode` | `Aot` / `Jit` — whether this program was built ahead of time or is running through `quilon run` |
| `info.quilonVersion() -> Text` | the compiler that built it, e.g. `"0.10.0"` — an open set, so a `Text` |

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

**Target, not host.** The answers describe the machine the program will **run** on. For an
ordinary build that is the machine that built it, but a cross-compiled binary reports its
target — the useful answer, since the program is the thing asking.

**`WtfPlatform` and `WtfOs` say which.** They carry the raw text the compiler saw — the
architecture, and the whole target triple — so a target with no variant of its own still
reports what it is rather than collapsing to a shrug. `name()` returns it.

**Names people use, not triple spellings.** `Os` has a `MacOS` variant, never a `Darwin` one.
A target triple is a build-system detail.

**`PointerWidth` and `Endianness` come from LLVM's data layout**, not from the architecture's
name — which is a poor guide, since `powerpc64le` and `mips64el` are little-endian despite
their spelling, and `s390x` is 64-bit without saying so.

**`runMode()` is the one that is not about the target.** It reports how the program is being
executed: `Jit` under `quilon run`, `Aot` for a binary from `quilon build`. `quilon compile`
emits the IR an ahead-of-time build would, so it reports `Aot` too.

**The import is required**, unlike `core.time`'s. The types and functions are real Quilon
declared in this module, so `<< core.info` is what brings them into scope.
