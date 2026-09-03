---
title: "Modules"
---

# Modules

```quilon ignore
<< core.io                 ~ import the built-in IO module; binds `io`
<< "lib/math.qn"           ~ import a user module by path (/ or \); binds its file stem, `math`

>> add = (a :: Num, b :: Num) => < a + b > ~ `>>` exports an item; unmarked items are file-private

^ = () -> Num => <
  io.print(math.add(2, 3))  ~ exports are reached THROUGH the module's binding
  0
>
```

## Qualified access

An import binds one name in the file — the module's **last path segment** (`<< core.http` binds `http`; a file import binds its file stem), and every
export is reached through that binding: `http.send(...)`, `io.print(...)`,
`http.Request { … }`. Types, their variants, constants, and functions all qualify the
same way, in every position:

```quilon ignore
<< core.http

classify = (m :: http.Method) -> Num => < ~ a qualified type in an annotation
  m ?
    | http.Get     => 1                   ~ qualified variants in patterns
    | http.Post(_) => 2
    | _            => 0
>

request = http.Request { method = http.Get, url = "http://example.com/" }
```

The **full path always works too**: `core.http.send(...)`. It is the escape hatch, not
the everyday form — when two imported modules share a last segment (`core.test` and a
user's `foo.test`), the short name is ambiguous and the compiler asks for the full path.
A module imported by file path has only its stem, so two file imports with the same stem
are rejected at the import — rename the file.

A file import resolves to the module's real path on disk, so the same file reached
through two different spellings (`..`, a symlink, ...) loads once. An import
cycle — a module that imports itself, directly or through others, including one that
leads back to the program's own entry file — is a compile error naming the cycle.

An import **claims its short name** for the whole file: after `<< core.http`, a binding
named `http` (top-level, local, or a parameter) is an error. And an import binds only the
code **below it** — like every other name, since the language has no hoisting.

Two spellings stay bare:

- `@` leaf IO primitives (`@sleep`, `@readStdin`, `@tcpRequest`): importing their module
  is required, and the `@` name is global — the sigil marks it.
- The compiler's own surface — `assert`/`expect` and the matchers — which belongs to no
  module and needs no import.

## Privacy

A module exposes its `>>`-exported items, and its private items **travel with it**:
an exported function may call a private sibling, and the importer reaches the exports
alone — `math.helper` answers ``​`helper` is not exported by `math`​`` for a private
`helper` and for an absent one alike.

An import is whole and named by the module: the binding carries every export under the
module's own name. A module that builds on another holds it and delegates (composition):

```quilon ignore
<< core.http
>> fetch = (url :: Text) -> Result => < http.Request { method = http.Get, url = url }.send() >
```

## Closed overload sets

Qualified access closes a module's overload sets: an imported module's function has the
members the module declares. `core.io.print` takes **any renderable value** — a type
becomes printable by defining its own `` ` `` render member. A program's own bare `print`
(or `now`, or `write`) is an unrelated function.

- The built-in modules are `core.io`, `core.test`, `core.cli`, `core.time`, `core.info`, `core.net`, and `core.http`; their members are real functions. See the [corelib](../corelib/README.md) index for each module's API reference.
- `Text` and the operators are built-ins, available without an import.
- A file's [`test.describe` blocks](../corelib/test/README.md) need `<< core.test`; every command but `quilon test` erases the blocks, and the harness is emitted with the blocks it serves.

(See `examples/qualified_modules.qn` for the access model end to end, and
`examples/use_module.qn`, which imports `examples/mathlib.qn` by file path.)
