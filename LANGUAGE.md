# Quilon Language Reference

**Version:** 0.9.0 (stable basics — the core is solid and verified end-to-end, but the language is **not** yet feature-complete; see [Known limitations](#known-limitations)).

Quilon is a statically-typed, **symbol-based** language (no control-flow keywords) that compiles to native code via LLVM. Every example below has a passing end-to-end test (it compiles, runs, and produces the documented exit code / output).

---

## Symbols

| Symbol | Meaning | Example |
|--------|---------|---------|
| `=` | Immutable binding | `x = 42` |
| `:=` | Mutable bind / reassign / in-place field write | `counter := 0`, `obj.field := v` |
| `::` | Type annotation | `x :: Num` |
| `=>` | Function body / match arm | `f = x => x + 1` |
| `->` | Return type | `f = x -> Num => x` |
| `< >` | Block delimiters | `< a b a + b >` |
| `^` | Entry point (main) | `^ = () -> Num => 0` |
| `$` | Unit type **and** its sole value | `f = () -> $ => $` |
| `<<` | Import a module | `<< core.io` |
| `>>` | Export an item from a module | `>> add = (a, b) => a + b` |
| `\|>` | Pipe (first-arg injection) | `x \|> f(a)` ≡ `f(x, a)` |
| `for n <- xs => body` | Loop over a collection | `for n <- [1,2,3] => print(n)` |
| `?` `\|` `_` | Pattern match | `v ? \| 0 => "zero" \| _ => "other"` |
| `? :` | Ternary | `x < 0 ? -x : x` |
| `~` | Comment (to end of line) | `~ a note` |

There are **no keywords**: `if`/`while`/`for`/`return` etc. are all expressed with symbols.

---

## Types

### `Num`
All numbers — integers and floats are one unified type, represented as `f64`.
```quilon
x = 42
y = 3.14
z = x + y          ~ mixed arithmetic
```

### `Text`
UTF-8 text. A **built-in** type (like `Num`/`Bool`/arrays) — **no import needed**. Represented internally as `{ ptr, byte_len }`.
```quilon
greeting = "héllo" + " 🌍"   ~ + concatenates (GC-allocated)
b = greeting.size            ~ byte length      → 11
c = greeting.length          ~ grapheme count   → 7
```
- `.size` = byte length.
- `.length` = grapheme-cluster count (user-perceived characters, full UTF-8).
- `+` = concatenation.

(See `examples/text.ql`.)

### `Bool`
`true` / `false`.

### `Unit` — `$`
The **unit type**, written `$`. It has exactly one value, also written `$` — so `$` is
both the type (in type position, e.g. `-> $`) and its sole value (in value position),
analogous to `()` in Rust/ML. Use it for side-effecting expressions and functions whose
result is meaningless. `print` and `eprint` return `$`. `$` is compatible only with `$`.
```quilon
log = (m :: Text) -> $ => print(m)   ~ a function whose result is meaningless
^ = () -> $ => log("started")        ~ a `$` body exits 0 (it is not a Num)
```

### Arrays — `[]T`
```quilon
nums  = [1, 2, 3, 4, 5]
count = nums.size      ~ → 5
first = nums[0]        ~ → 1
```
Arrays are `{ ptr, size }` internally. (See `examples/arrays.ql`.)

### Records
Anonymous structs with named fields:
```quilon
user = { name = "Alice", age = 30 }
n    = user.name
```
(See `examples/records.ql`.)

### Named record types with methods
Methods take an implicit `it` (the receiver):
```quilon
User = {
  name :: Text,
  age  :: Num,
  greet   = => "Hello, " + it.name,
  olderBy = years => it.age + years
}

u = User { name = "Alice", age = 30 }
g = u.greet()          ~ "Hello, Alice"
a = u.olderBy(5)       ~ 35
```
(See `examples/methods.ql`.)

A method is a **setter** (mutating) iff its body writes `it.field := …` (or calls
another setter on `it`); there is no marker — the visible `:=` *is* the signal.
Calling a setter requires a mutable (`:=`) receiver (see [Mutation](#mutation-in-place-field-writes--setters)).

### Sum types — `Ok` / `NotOk`
The built-in result type for success/failure. Construct with `Ok(value)` / `NotOk(value)`, consume with pattern matching:
```quilon
classify = v => v ?
  | Ok(x)    => x * 2
  | NotOk(e) => 0
```
Numeric payloads work end-to-end. (See `examples/result.ql`. Non-numeric payloads: see [Known limitations](#known-limitations).)

---

## Variables

Immutable by default (`=`); use `:=` to declare a mutable binding **and** to reassign it.
```quilon
x = 42                  ~ immutable bind (rebinding x with = is an error)
counter := 0            ~ mutable bind
counter := counter + 1  ~ reassign (also :=)
```
Reassigning requires the binding to be mutable: `x := 5` on an immutable `x` is an error.
Types are inferred but can be annotated: `x :: Num = 42`.

---

## Mutation: in-place field writes & setters

Mutability is **Rust-like**, decided by the binding operator — it governs not just
reassignment but in-place mutation of records:

- An `=`-bound instance is **immutable** (frozen): no field writes, and calling a
  setter (mutating) method on it is a compile error.
- A `:=`-bound instance is **mutable**: both forms of in-place mutation are allowed —
  a direct field write `obj.field := value` (mutates the existing record, no
  re-allocation), and any **setter** method.

```quilon
Counter = {
  value :: Num,
  bump = (by :: Num) => it.value := it.value + by   ~ setter: writes `it.value := …`
}

c := Counter { value = 30 }   ~ `:=` -> mutable
c.bump(5)                      ~ setter mutates in place -> value = 35
c.value := c.value + 7         ~ direct field write    -> value = 42
```

A method is a **setter** iff its body writes `it.field := …` (or calls another setter
on `it`) — there is **no marker/annotation**; the `:=` in the body is the signal.
A setter call requires a `:=` receiver:

```quilon
c = Counter { value = 30 }   ~ `=` -> immutable
c.value := 99                 ~ error: cannot write a field of immutable `c`
c.bump(5)                     ~ error: cannot call mutating method `bump` on immutable `c`
```

Non-mutating (getter) methods carry no `it.field := …` and so are callable on `=`
instances too. (See `examples/mutation.ql`.)

---

## Functions

```quilon
greet  = => "Hello!"                       ~ no params
double = x => x * 2                        ~ one param, no parens
add    = (a, b) => a + b                   ~ multiple params
typed  = (a :: Num, b :: Num) -> Num => a + b
```
Multi-statement bodies use `< >` blocks (the last expression is the value):
```quilon
compute = x => <
  doubled = x * 2
  doubled * doubled
>
```
Functions may recurse; a recursive function needs a `-> Type` annotation:
```quilon
factorial = n -> Num => n == 0 ? 1 : n * factorial(n - 1)
```
(See `examples/factorial.ql`, `examples/fibonacci.ql`.)

---

## Expressions

- **Arithmetic:** `+ - * / %` (and `-x`). `+` is overloaded for `Text` concatenation.
- **Comparison:** `== != < <= > >=`.
- **Logical:** `&& || !` (short-circuit).
- **Ternary:** `cond ? then : else`.
- **Blocks:** `< stmt… last >` are expressions that evaluate to their last expression — usable anywhere a value is, not just as a function body:
```quilon
result = <
  x = 10
  y = 20
  x + y          ~ result is 30
>
```

### Pipe — `|>`
`|>` feeds its left operand in as the **first argument** of the right-hand call:
```quilon
x |> f          ~ ≡ f(x)
x |> f(a)       ~ ≡ f(x, a)
10 |> double |> addFive   ~ ≡ addFive(double(10))
```
(See `examples/pipeline.ql`.)

### Loops — `for n <- collection => body`
Iterate a collection for side effects (returns `Num` 0):
```quilon
for n <- [1, 2, 3] => print(n)
for (val, i) <- xs => print(i)   ~ with index
```
The body may be a single expression or a `< >` block. (See `examples/for_loop.ql`.)

---

## Pattern matching

```quilon
result = value ?
  | 0        => "zero"
  | 1        => "one"
  | Ok(x)    => x
  | NotOk(e) => 0
  | _        => "other"      ~ wildcard
```
The type checker verifies matches are exhaustive (use `_` to cover the rest). (See `examples/pattern_match.ql`.)

---

## Modules

```quilon
<< core.io                 ~ import the built-in IO module
<< "lib/math.ql"           ~ import a user module by path (/ or \)

>> add = (a, b) => a + b   ~ `>>` exports an item; unmarked items are file-private
```
- `core.io` is the one built-in module (its members are real functions).
- `Text` and the operators are built-ins and need **no** import.
- A module exposes only its `>>`-exported items.

(See `examples/use_module.ql`, which imports `examples/mathlib.ql`.)

---

## I/O — `<< core.io`

| Function | Effect |
|----------|--------|
| `print(x) -> $` | Write `x` to stdout, **with a trailing newline**. Polymorphic over `Num`/`Text`/`Bool` (`Bool` prints `true`/`false`). Returns `$` (Unit). |
| `eprint(x) -> $` | Same, to stderr. Returns `$` (Unit). |
| `write(content :: Text, fd :: Num) -> Num` | Write raw bytes (no newline) to a file descriptor; returns bytes written. |
| `stdout`, `stderr` | The standard file descriptors. |

```quilon
<< core.io
^ = () -> Num => <
  print("hello")            ~ stdout: hello\n
  "raw" |> write(stdout)    ~ stdout: raw   (no newline)
  eprint("oops")            ~ stderr: oops\n
  0
>
```
There is no `println` — `print` owns the newline; `write` is the raw form. (See `examples/io.ql`.)

---

## Entry point

Every executable defines `^` (main); the compiler generates a C-compatible `main()` wrapper (and initializes the GC).
```quilon
^ = () -> Num => 42                              ~ exit 42
^ = (argc :: Num, argv :: Num) -> Num => argc    ~ argv is a placeholder for now
```
**Exit code:** if `^`'s body evaluates to a `Num`, that value is the exit code. If the body is **not** a `Num` (e.g. a side-effecting block), the program exits **0** — so an effect-only `main` needs no trailing `0`. (This implicit-0 applies only to `^`; ordinary functions always return their last expression's value.)

(See `examples/hello_world.ql`.)

---

## Memory

Quilon uses a **conservative garbage collector** (Boehm). Heap values (`Text`, etc.) are GC-managed — there is no manual free. In 0.9 this is the system's **dynamic `libgc`** (a documented build- and run-time dependency); a statically-linked / vendored GC is a post-0.9 goal.

---

## Compiling & running

```bash
quilon check   program.ql   # front-end only (lex + parse + resolve imports + typecheck)
quilon run     program.ql   # front-end, then JIT-execute in-process (exit code = ^'s result)
quilon build   program.ql   # produce a native executable
quilon compile program.ql   # emit LLVM IR → program.ll (for inspection)
```

`quilon build` emits an object file in-process and links it (with the Quilon runtime `libquilon_rt` and the GC `libgc`) into a native executable:
```bash
quilon build program.ql -o program       # default linker: clang
quilon build program.ql --linker gcc      # gcc also supported (CI checks both)
./program; echo "exit: $?"
```
(During development, prefix any command with `cargo run --`, e.g. `cargo run -- run program.ql`.)

### Error messages

Compile errors — from the lexer, parser, and type checker — are reported in a
rustc-style format: a `path:line:col: error: <message>` header, followed by the
offending source line and a caret (`^`) underline beneath the exact span. Line
and column are **1-based**, and the column counts characters (not bytes), so it
is correct in the presence of multi-byte characters. For example, the program

```
add = (a :: Num) -> Num => a + true
```

reports:

```
program.ql:1:28: error: Type mismatch: expected Num, got Bool
  |
1 | add = (a :: Num) -> Num => a + true
  |                            ^^^^^^^^
```

A span covering multiple lines underlines its first line. Failures with no
source location (a missing file, an unresolved import) print a plain one-line
message instead. Any compile error exits with status 1.

---

## Feature matrix

✅ = works end-to-end with a passing run test · 🚧 = partial · ❌ = not yet

| Feature | Status |
|---|---|
| `^` entry point, native compile + JIT `run` | ✅ |
| `Num`, arithmetic, comparison, logical, ternary | ✅ |
| `Text` built-in: literals, `+`, `.size`, `.length` | ✅ |
| `Bool` | ✅ |
| `Unit` type / value (`$`) | ✅ |
| Arrays: literals, `.size`, `[index]` | ✅ |
| Records + field access | ✅ |
| Named record types + methods (`it`) | ✅ |
| In-place mutation of `:=` records: field writes (`obj.f := v`) + setter methods | ✅ |
| Functions, recursion, blocks, type inference | ✅ |
| Pipe `\|>` (first-arg injection) | ✅ |
| `for n <- collection => body` loops | ✅ |
| Pattern matching (numbers, wildcard, identifiers, `Ok`/`NotOk`) | ✅ |
| `Ok`/`NotOk` with **numeric** payloads | ✅ |
| Modules: `<< core.io`, file-path imports, `>>` exports | ✅ |
| I/O: `print` / `eprint` / `write` | ✅ |
| Conservative GC (Boehm) | ✅ |
| `Text` in records/arrays, or as a sum-type payload (`Ok(text)`) | 🚧 |
| Command-line `argv` (argc works; argv is a placeholder) | 🚧 |
| Generics, closures, `while` loops, custom sum types | ❌ |
| Array methods (`map`/`filter`/`reduce`), string interpolation | ❌ |

---

## Known limitations

0.9 is a stable **core**, not the whole language. Notably:

- **Non-numeric data in composites isn't sound yet.** `Text` inside a record or an array, and non-numeric sum-type payloads such as `Ok("x")` / `NotOk("error")`, do not type-check correctly in 0.9 — numeric payloads and numeric records/arrays work. Planned for a later release.
- **Array `.size` works only on a named receiver** (`xs.size`), not on a literal/expression (`[1,2,3].size`).
- A user-defined `print`/`eprint` is honored by the type checker but the code generator still lowers the built-in — overriding the runtime body is a follow-up.
- **No generics, closures, `while` loops, or user-defined (custom) sum types.** The module system is minimal (`core.io` built-in + file-path imports).
- `argv` is a placeholder (0); full `[]Text` conversion is planned.

---

## Compiler architecture

A classic multi-pass pipeline (each stage a module under `src/`); `src/driver.rs` runs the shared front-end (read → lex → parse → resolve imports → typecheck) for all CLI commands and renders any failure through `src/diagnostic.rs` (the rustc-style `path:line:col` reporter described under [Error messages](#error-messages)).

1. **Lexer** — `src/lexer/` (`logos`), `Lexer::tokenize(&str)`.
2. **Parser** — `src/parser/ast_parser.rs`, hand-written recursive descent, `parse(&tokens)`.
3. **AST** — `src/ast/nodes.rs`.
4. **Type checker** — `src/typechecker/` (`checker.rs` + `inference.rs`).
5. **Code generator** — `src/codegen/generator.rs` (`inkwell`, LLVM 22) → LLVM IR.
6. **Runtime intrinsics** — `src/runtime/` (`__write_bytes`, grapheme counting, GC glue), packaged as `libquilon_rt`.
7. **LLVM** — `quilon build` emits an object in-process and links `libquilon_rt` + `libgc` into a native binary; `quilon run` uses an in-process JIT.

See `CLAUDE.md` for contributor guidance.
