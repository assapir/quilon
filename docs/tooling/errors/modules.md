---
title: "Module errors"
sidebar:
  order: 2
---

# Module errors

QN2xx: import resolution and linking errors.

## Imports

### QN200 — `@` primitive declared outside the corelib

A user source declares a name starting with `@`. The `@` marks a built-in IO primitive,
which only the corelib defines; user code calls one.

```quilon ignore
@sleep = (seconds :: Num) -> $ => < $ >
```

Declare an ordinary function, and call the corelib's primitive where a primitive is meant.

### QN201 — missing module

A `<<` import names a built-in module the compiler lacks, a file that is missing or
unreadable, or `core.text`, which the compiler merges on its own.

```quilon ignore
<< core.magic
```

Import one of the built-in modules (`core.io`, `core.test`, `core.cli`, `core.time`,
`core.net`, `core.http`, `core.info`) or an existing `.qn` file by path.

### QN202 — private member reached through its module

A qualified reference names a member the module keeps private, or one it lacks.

```quilon ignore
<< core.io
^ = () -> Num => < io.secret() >
```

Reach an exported member, or mark the member `>>` in its module.

### QN203 — name claimed by an import

A binding, parameter, or pattern name is the short name an import binds (`io` for
`<< core.io`).

```quilon ignore
<< core.io
io = 1
```

Rename the binding; the import keeps its name.

### QN204 — import cycle

A module imports itself, directly or through other modules.

```quilon ignore
<< "cycle_lib.qn"
^ = () -> Num => < 0 >
```

```quilon title="cycle_lib.qn"
<< "root.qn"
```

Move the shared definitions into a third module both import.

### QN205 — two modules with one name

Two imported modules bind the same short name — two files named `util.qn` in different
directories, or a file stem that is unusable as a binding.

```quilon ignore
<< "lib/util.qn"
<< "vendor/util.qn"
```

```quilon title="lib/util.qn"
answer = 1
```

```quilon title="vendor/util.qn"
answer = 2
```

Rename one of the files, or drop one of the imports.

### QN206 — module binding used as a value

An import's binding name appears where a value is required.

```quilon ignore
<< core.io
^ = () -> Num => < x = io >
```

Reach the module's exports through the binding: `io.print(…)`.

### QN207 — ambiguous module prefix

A short prefix is bound by more than one imported module.

```quilon ignore
<< core.http
<< "vendor/http.qn"
^ = () -> Num => < http.send(request) >
```

Write the full path: `core.http.send(…)`.

### QN208 — test blocks without `core.test`

A file has top-level `test.describe` blocks and `quilon test` finds the harness's summary
function out of scope. Recognizing a call as a test block already requires `<< core.test`
above it — an unimported `test.describe` reads as an ordinary qualified reference and is
[QN106](syntax.md#qn106--qualified-name-through-a-missing-import) instead — so this is the
backstop for `core.test` itself failing to define its summary function.

```text
error[QN208]: no test harness in scope: `core.test.reportSummary` is undefined
  help: add `<< core.test` above this block
```
