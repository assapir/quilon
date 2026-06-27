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
| `< >` | Block delimiters · also `<`/`>` comparison ([rule](#expressions)) | `< a b a + b >` · `a < b` · `a > b` |
| `^` | Entry point (main) | `^ = () -> Num => 0` |
| `$` | Unit type **and** its sole value | `f = () -> $ => $` |
| `<<` | Import a module | `<< core.io` |
| `>>` | Export an item from a module | `>> add = (a, b) => a + b` |
| `\|>` | Pipe (first-arg injection) | `x \|> f(a)` ≡ `f(x, a)` |
| `for n <- xs => body` | Loop over a collection | `for n <- [1,2,3] => print(n)` |
| `<-` (infix) | Inclusive range → `[]Num` | `1 <- 4` ≡ `[1,2,3,4]` · `4 <- 1` ≡ `[4,3,2,1]` |
| `?` `\|` `_` | Pattern match | `v ? \| 0 => "zero" \| _ => "other"` |
| `/` | Division **or** sum-type variant separator | `a / b` · `Color = Red / Green` |
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
Fields may hold any type — `Text`, arrays, nested arrays, etc. — and read back at
their real type (no numeric-only restriction). (See `examples/records.ql` and
`examples/composites.ql`, which exercises a `Text` record field, an array of `Text`,
and a nested array together.)

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

### Sum types — `/`
A sum type (tagged union / enum) is a set of named **variants**, declared with `/`
as the separator. Variants may be **nullary** or carry a payload:
```quilon
Color = Red / Green / Blue                 ~ three nullary variants
Shape = Circle(Num) / Rect(Num, Num)       ~ variants with payloads
```
- **Payloads are built-in types only** — `Num`, `Text`, `Bool`, or `$` (Unit). There are
  no type variables (no generics), but a variant may take several payload fields
  (e.g. `Rect(Num, Num)`). A `$` payload carries no value — it's the "this variant has
  no data" case (see `Ok($)` below).
- At a given payload position, every variant with a concrete (non-`$`) field there must
  agree on its type; `$` may coexist with a concrete type at the same position
  (`Done($) / Pending(Num)` is fine, `A(Num) / B(Text)` is rejected).
- **Variant (constructor) names are unique per scope** — two sum types can't share a
  variant name.

**Construct** a value by naming the variant (with payload arguments if it has any), and
**consume** it with `?`/`|` pattern matching, which binds the payload:
```quilon
area = (s :: Shape) -> Num => s ?
  | Circle(r)  => 3 * r * r
  | Rect(w, h) => w * h          ~ binds both payload fields
```
A match over a sum type **must be exhaustive**: cover every variant, or end with a `_`
(or a lowercase binding) wildcard. (See `examples/sum_types.ql`.)

#### `Result` is a normal sum type
`Result` is just a predefined sum type — there is no special case:
```quilon
Result = Ok(...) / NotOk(...)    ~ predefined; `Ok` = success, `NotOk` = failure
```
Use it exactly like any other sum type:
```quilon
classify = v => v ?
  | Ok(x)    => x * 2
  | NotOk(e) => 0
```
Payloads work end-to-end for `Num`, `Bool`, and `Text` (e.g. `Ok("done")` /
`NotOk("error")`). (See `examples/result.ql` and `examples/composites.ql`.)

#### `/` — sum-type separator vs. division
`/` is the division operator **and** the sum-type variant separator. They are told apart
by Quilon's **Capitalized-type / lowercase-value** convention: `/` is a variant separator
**only** in a type-declaration context — i.e. when the binding name and every operand are
Capitalized type/constructor names:
```quilon
Color = Red / Green / Blue       ~ sum type: name + operands are Capitalized
half  = a / b                    ~ division: lowercase operands are values
```
A single bare Capitalized name with no `/` (e.g. `x = Red`) is an ordinary value binding
(here, of an existing nullary variant), not a one-variant sum-type declaration.

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

## Overloading

Quilon has **explicit ad-hoc overloading** — the *only* form of polymorphism (there
are no generics / type variables). Multiple top-level definitions that **share a name
and each carry full parameter type annotations** simply *are* an overload set — there is
no marker symbol or keyword:

```quilon
score = (n :: Num)  -> Num => n + 1       ~ the Num member
score = (s :: Text) -> Num => s.size      ~ the Text member

a = score(41)       ~ 42  — picks the Num member
b = score("abcd")   ~ 4   — picks the Text member
```

**Dispatch is by exact static argument type, with NO implicit coercion.** At each call
site the compiler picks the member whose parameter types match exactly. If none matches,
or (with exact matching) two members share a parameter-type list, it is a clear compile
error that lists the candidates:

```
error: No overload of 'score' matches argument types (Bool). Candidates: (Num), (Text)
```

- Every member of an overload set must annotate **all** its parameters (exact dispatch
  can't choose between unannotated members).
- A single, ordinary `name = …` definition is **not** an overload set — it keeps full
  type inference (unannotated params default to `Num`, the return type is inferred).
- Dispatch is resolved at **direct call sites** by static argument types. Passing an
  overloaded name as a value (higher-order use) is not yet supported.

### Operator overloading

Operators are user-overloadable — `+ - * / %`, `== != < <= > >=` — because **an operator
is just a named overload set** under the hood. The standard operators are *visible*
overloads (e.g. `+` on `Num` and `+` on `Text`), not compiler magic, and a user
definition adds a member for a user type. Define one by naming it with the operator
symbol:

```quilon
Vec = { x :: Num, y :: Num }
+ = (a :: Vec, b :: Vec) -> Vec => Vec { x = a.x + b.x, y = a.y + b.y }

v = Vec { x = 1, y = 2 } + Vec { x = 3, y = 4 }   ~ resolves to the user `+`
```

A user operator overload is resolved exactly like a function overload (by argument
types) and lowers to a direct call. `==` over `Text` (equality) and `<`/`>`/`<=`/`>=`
over `Text` (lexicographic order) are built-in overloads, so text comparisons work out
of the box: `"abc" < "abd"`, `"hi" == "hi"`. (Defining `<`/`>` is reserved — a top-level
`<`/`>` would read as a block; overload the others, or use `<=`/`>=`.)

A **comparison/equality** operator overload (`== != < <= > >=`) **must return `Bool`** —
these are predicates that feed `?`/`|` matching and conditionals; a non-`Bool` return is
a compile error. **Arithmetic** operators (`+ - * / %`) are unconstrained: an overload
returns whatever it declares (so `Vec + Vec -> Vec`, `Vec * Num -> Vec`, or a `Vec * Vec
-> Num` dot product are all legal).

(See `examples/overloading.ql`.)

---

## Expressions

- **Arithmetic:** `+ - * / %` (and `-x`). `+` is an [overload set](#overloading): `Num + Num` adds, `Text + Text` concatenates.
- **Comparison:** `== != < <= > >=`. Over `Num` and (lexicographically) `Text`; all return `Bool`. Each is a [user-overloadable operator](#operator-overloading).
- **Logical:** `&& || !` (short-circuit).

> **`<` and `>` vs. `< >` blocks.** `<` and `>` double as the block delimiters. A `<`
> after a complete operand is always less-than (a block can't start mid-expression). A
> `>` is the **block close** only when it is the **last token on its line** (`>`
> followed by only spaces/tabs then a newline or end-of-file); any other `>` — one with
> more on the same line, like `a > b` — is the greater-than operator. So `a > b` works
> everywhere; the only rule is *don't end a line with a comparison `>`* (write the right
> operand on the same line). `<=`/`>=`/`>>` are distinct tokens and unaffected.
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

### Ranges — infix `lo <- hi`
The infix `<-` operator builds an **inclusive** `[]Num`:
```quilon
1 <- 4          ~ [1, 2, 3, 4]
4 <- 1          ~ [4, 3, 2, 1]   (descends when the left end is larger)
5 <- 5          ~ [5]            (single point)
```
It is pure **array sugar** — there is no distinct `Range` type; the result *is* a
`[]Num`, so it composes with `.size`, indexing `[i]`, and `for` loops:
```quilon
r = 2 <- 5      ~ [2, 3, 4, 5]
n = r.size      ~ 4   (inclusive count = |hi - lo| + 1)
first = r[0]    ~ 2
for x <- 1 <- 3 => print(x)   ~ a range drives a loop like any array
```
Both ends are full `Num` expressions (they may be dynamic, not just literals); the
direction (ascending vs descending) is decided at runtime. (See `examples/ranges.ql`.)

> Note: the infix range `<-` is distinct from the `for` header's `<-`
> (`for n <- collection => …`). The `for` form is the loop binder; the infix form,
> *between two value expressions*, is the range constructor.

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
| `print(x) -> $` | Write `x` to stdout, **with a trailing newline**. An [overload set](#overloading) over `Num`/`Text`/`Bool` (`Bool` prints `true`/`false`). Returns `$` (Unit). A user `print` definition *adds* an overload. |
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

reports (since `+` is an [overload set](#overloading), a `Num + Bool` matches no member):

```
program.ql:1:28: error: No overload of '+' matches argument types (Num, Bool). Candidates: (Num, Num), (Text, Text)
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
| `Text` comparison: `==`/`!=` (equality), `<`/`<=`/`>`/`>=` (lexicographic) | ✅ |
| Ad-hoc overloading: same-named typed defs, exact-type dispatch | ✅ |
| Operator overloading (`+`, comparisons, … on user types); built-ins as overloads | ✅ |
| `Bool` | ✅ |
| `Unit` type / value (`$`) | ✅ |
| Arrays: literals, `.size`, `[index]` | ✅ |
| Records + field access | ✅ |
| Named record types + methods (`it`) | ✅ |
| In-place mutation of `:=` records: field writes (`obj.f := v`) + setter methods | ✅ |
| Functions, recursion, blocks, type inference | ✅ |
| Pipe `\|>` (first-arg injection) | ✅ |
| `for n <- collection => body` loops | ✅ |
| Ranges: infix `lo <- hi` → inclusive `[]Num` (descends when `lo > hi`) | ✅ |
| Pattern matching (numbers, wildcard, identifiers, sum-type variants) | ✅ |
| User-defined sum types (`/` separator), exhaustive matching, payload binding | ✅ |
| `Result` as a normal predefined sum type (`Ok`/`NotOk`) | ✅ |
| Sum-type payloads: `Num` / `Bool` / `Text` | ✅ |
| Modules: `<< core.io`, file-path imports, `>>` exports | ✅ |
| I/O: `print` / `eprint` / `write` | ✅ |
| Conservative GC (Boehm) | ✅ |
| `Text` (and nested arrays) in records/arrays, or as a sum-type payload (`Ok(text)`) | ✅ |
| Command-line `argv` (argc works; argv is a placeholder) | 🚧 |
| Generics / type variables (overloading is the only polymorphism), closures, `while` loops | ❌ |
| Overloaded name passed as a value (higher-order); only direct call sites resolve | ❌ |
| Array methods (`map`/`filter`/`reduce`), string interpolation | ❌ |

---

## Known limitations

0.9 is a stable **core**, not the whole language. Notably:

- **A generic `Result` payload routed through an overload set resolves to the `Num` member.** `Text`/array fields and `Ok("x")`/`NotOk("e")` payloads now type-check and round-trip end-to-end (see [records](#records), [`Result`](#result-is-a-normal-sum-type)). But a `Result` payload is *generic*, so binding it (`Ok(x) => …`) and passing `x` to an [overload set](#overloading) still resolves to the **`Num`** member; a user sum type's payloads are concrete (`Circle(Num)`, `On(Bool)`), so they dispatch overloads correctly by their declared type.
- **Array `.size` works only on a named receiver** (`xs.size`), not on a literal/expression (`[1,2,3].size`).
- **No generics, closures, or `while` loops.** Overloading (ad-hoc, exact-type
  dispatch) is the only polymorphism; there are no type variables. The module system is
  minimal (`core.io` built-in + file-path imports).
- **Overloads resolve at direct call sites only.** Passing an overloaded name as a value
  (higher-order use) is not yet supported.
- **Sum-type payloads mixing types across variants behind one value aren't unified yet.** Each variant's payload slots have a fixed representation sized to the widest variant; a single value carries one variant's payload. Distinct payload *types* per slot across variants (e.g. a position that is `Num` in one variant and `Text` in another) is a deferred follow-up — the built-in payload set (`Num`/`Text`/`Bool`, consistent per position) works.
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
