---
title: "Build errors"
sidebar:
  order: 4
---

# Build errors

QN4xx: code generation and native build failures.

## Code generation and build

### QN400 — code generation failed

The checked program reached a case the code generator lacks. The message names the case.

```text
error[QN400]: unknown array method `frobnicate`
```

File the message and the program as an issue; the checker and the generator agree on every
construct the language reference documents.

### QN401 — native build failed

The linker is missing, or the link or the write of the output failed. The message carries
the linker's own words.

```text
error[QN401]: linker `clang` not found on PATH. Install it, or pass `--linker <name>` (e.g. `--linker gcc`).
```

Install `clang`, or pass `--linker gcc`.
