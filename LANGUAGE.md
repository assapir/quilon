# Quilon Language Reference

**Version:** 0.9.0 (stable basics — the core is solid and verified end-to-end, but the language is **not** yet feature-complete; see [Known limitations](#known-limitations)).

Quilon is a statically-typed, **symbol-based** language (no control-flow keywords) that compiles to native code via LLVM. Every example below has a passing end-to-end test: each `examples/*.ql` program is **self-asserting** — it verifies its own results in-language with `<< core.test` and exits 0 (a failing assertion aborts with exit 101), under both the JIT (`quilon run`) and native AOT.

---

## Design principles

Quilon's identity, and the rules that guide its design:

- **No keywords.** Every construct is punctuation, not words — *nothing was removed from the language; the words were.* Branching is `?` / `|`, the entry point is `^`, import/export are `<<` / `>>`, mutability is `:=`, sum-type alternatives are `/`. (`for` is the lone surviving word — a known wart, slated for removal.)
- **Symbols mirror notation that already exists.** A symbol should reuse a notation the world already has rather than invent one: `/` separates sum-type alternatives the way you already write "red / green / blue". The symbol is both the shorthand and its own justification.
- **The playful choice wins.** When a design decision is a genuine toss-up, the more delightful option is picked — characterful, memorable symbols (`^` for the entry point, `$` for Unit) over bland ones. Syntax is allowed to have a sense of humor.
- **Deliberate simplicity — reject complexity.** The smallest system that works: no generics (ad-hoc overloading is the only polymorphism), no `while`, no interfaces, a single `Num` type. Features are omitted on purpose.
- **Fail loud, never silent.** Invalid inputs and meaningless operations must *fail* — never silently no-op, clamp, or return a magic sentinel. If the compiler can determine the problem (a literal / statically-known value) it is a **compile error**; otherwise a **clear runtime error** (stderr, non-zero exit). Silent behavior is undebuggable, so Quilon refuses it. (Hence `Text.indexOf → Ok(Num)/NotOk` rather than a `-1` sentinel, and `Text.replace`'s count/empty-argument checks failing rather than clamping.)
- **No magic.** Behavior is explicit and visible — no hidden coercions, no implicit dispatch. Overloads are exact-typed; operators mean what they say.
- **Immutable by default.** `=` binds immutably, `:=` binds mutably; because `:=` is visible wherever mutation happens, a method is a setter exactly when its body writes `it.field := …`.
- **Errors are values.** Fallible operations return `Ok` / `NotOk` (a normal sum type) — no exceptions, no sentinels.
- **Library APIs hide internals.** A library never makes the caller do its own conversion/desugaring (`print(x)`, never `print(show(x))`).

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
| `<-` (infix) | Inclusive range → `[]Num` | `1 <- 4` ≡ `[1,2,3,4]` · `4 <- 1` ≡ `[4,3,2,1]` |
| `<-` (prefix) | Spread inside a `[ ]` / `{ }` literal ([rule](#spread-in-literals)) | `[<-xs, 4]` · `{<-p, x = 9}` |
| `?` `\|` `_` | Pattern match | `v ? \| 0 => "zero" \| _ => "other"` |
| `/` | Division **or** sum-type variant separator | `a / b` · `Color = Red / Green` |
| `? :` | Ternary | `x < 0 ? -x : x` |
| `~` | Comment (to end of line) | `~ a note` |

There are **no keywords**: `if`/`return` etc. are all expressed with symbols, and there
are no loop constructs at all — iteration is via [array methods and recursion](#iteration--array-methods--recursion).

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

#### Text methods

`Text` carries a set of **built-in, compiler-provided methods**, called with method
syntax (`text.method(...)`) and freely chainable, each backed by a UTF-8-correct runtime
intrinsic. Where an index or length is user-visible they are **grapheme-based** (matching
`.length`), not byte-based.

| Method | Result | Notes |
|--------|--------|-------|
| `split(sep :: Text)` | `[]Text` | split on `sep`; consecutive separators keep empty pieces (`"a,,b".split(",")` → `["a","","b"]`), an empty haystack yields `[""]`, and an **empty** `sep` splits into individual graphemes (`"abc".split("")` → `["a","b","c"]`) |
| `trim()` | `Text` | strip leading **and** trailing whitespace |
| `trimStart()` / `trimEnd()` | `Text` | strip leading-only / trailing-only whitespace |
| `replaceAll(from :: Text, to :: Text)` | `Text` | replace **every** occurrence of `from` with `to` |
| `replace(from :: Text, to :: Text, count :: Num)` | `Text` | replace **exactly** the first `count` occurrences (left→right); `count` truncates toward zero |
| `contains(sub :: Text)` | `Bool` | whether `sub` occurs in the text |
| `indexOf(sub :: Text)` | `Ok(Num)` / `NotOk` | grapheme index of the first occurrence (`Ok`), or `NotOk` if absent — **no `-1` sentinel** |
| `slice(start :: Num, end :: Num)` | `Text` | substring over grapheme indices `[start, end)`; out-of-range indices **clamp** to bounds (never an error), and `end ≤ start` yields `""` |
| `toUpper()` / `toLower()` | `Text` | Unicode-aware case mapping |

```quilon
"a,b,c".split(",")                       ~ ["a", "b", "c"]
"  hi  ".trim()                          ~ "hi"
"  hi  ".trimStart()                     ~ "hi  "
"  hi  ".trimEnd()                       ~ "  hi"
"a-a-a".replaceAll("a", "x")             ~ "x-x-x"   (every occurrence)
"a-a-a".replace("a", "x", 1)             ~ "x-a-a"   (exactly the first)
"Hello".contains("ell")                  ~ true
"héllo".indexOf("llo") ?                 ~ Ok(2)  (grapheme index)
  | Ok(i)    => i
  | NotOk(_) => 0 - 1
"Hello".slice(1, 4)                      ~ "ell"
"Hello".slice(-5, 100)                   ~ "Hello"  (clamped)
"héllo".toUpper()                        ~ "HÉLLO"
```

Like the [array methods](#array-methods), these are **reserved on `Text`**: a user may
define a same-named function/overload on another type, but on a `Text` receiver the
built-in always wins. `split`'s result is a **plain generic array** — `[]Text` is just
`[]T` with `T = Text` (like `[]Num`), so it composes with `.size`, indexing `[i]`, the
[array methods](#array-methods), and array `+` concatenation. There is **no `join`** —
collapse a `[]Text` with `reduce` + `+`.

`replace`/`replaceAll` **fail loudly** — an invalid request is never a silent no-op. An
empty `from` (for either method), and a `replace` `count` that is `<= 0` or greater than
the number of occurrences actually present, are rejected: at **compile time** when the
values are literals (e.g. `"a".replace("a", "b", 0)` or `"aa".replace("a", "b", 5)`), and
otherwise at **run time** — the program prints a diagnostic to stderr and exits `101`. Use
`replaceAll` for "replace everything"; `replace(count)` is a precise "replace exactly this
many" contract.

(See `examples/text.ql` and `examples/text_methods.ql`.)

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

Indexing is **checked** (fail loud, never silent): an out-of-bounds, negative, or NaN
index is a clear runtime error — `runtime error: array index 10 out of bounds (size 3)`
to stderr, exit status 1 — never a raw memory read. A **fractional** in-range index
truncates toward zero (`nums[1.7]` reads `nums[1]`) — with one unified `Num`, index
arithmetic like `size / 2` legitimately produces fractions. Use [`at(n)`](#array-methods)
for the non-aborting form (`Ok`/`NotOk`).

#### Array methods

Arrays carry a set of **built-in, compiler-provided methods**, called with method
syntax (`arr.method(...)`) and freely chainable. The higher-order ones take a **lambda**
(`x => …`, `(a, b) => …`) — an anonymous function literal valid **only** as a direct
argument to one of these methods. The compiler **inlines** the lambda body per element
rather than passing it as a function value (a deliberate specialization — Quilon's
closures are not accepted as higher-order arguments here).

| Method | Result | Notes |
|--------|--------|-------|
| `map(f)` | new `[]R` | element type `R` is `f`'s return type (so `map` may change the element type, e.g. `[]Num → []Text`) |
| `filter(pred)` | new `[]elem` | keeps the elements where `pred` returns `Bool` `true`, in order; `pred` **must** return `Bool` |
| `reduce(init, (acc, x) => …)` | the accumulator | fold-left from `init`; the reducer's result type must match `init`'s type |
| `each(f)` | **the receiver array** | runs `f` for side effects, then returns the array itself, so it chains |
| `find(pred)` | `Ok(elem)` / `NotOk` | the first element satisfying `pred`, absent-safe; `pred` returns `Bool` |
| `at(n :: Num)` | `Ok(elem)` / `NotOk` | non-aborting index — `Ok` in bounds, `NotOk` otherwise (incl. NaN); raw `arr[n]` aborts with a runtime error instead |

```quilon
nums = [1, 2, 3, 4, 5, 6]

total = nums
  .map(x => x * 2)              ~ [2, 4, 6, 8, 10, 12]
  .filter(x => x > 4)           ~ [6, 8, 10, 12]
  .reduce(0, (acc, x) => acc + x)   ~ 36

first = nums.find(x => x > 3) ?  ~ Ok(4)
  | Ok(v)    => v
  | NotOk(_) => 0

third = nums.at(2) ?             ~ Ok(3)
  | Ok(v)    => v
  | NotOk(_) => 0
```

These methods are **reserved on arrays**: a user can define a same-named function/overload
(e.g. a `map` on a `Num`), but on an *array receiver* the built-in always wins — it is
resolved ahead of the overload set. `map`/`reduce`/`find` work over any element type
(e.g. `[]Text`), not just `[]Num`. (See `examples/array_methods.ql`.)

#### Array concatenation — `+`

`+` on arrays builds a **new** array (it never mutates an operand), in three forms — each
selected by the **exact** operand types, so there is never any ambiguity:

```quilon
~ concat:  []T + []T -> []T
[1, 2] + [3, 4]          ~ [1, 2, 3, 4]
["a"] + ["b", "c"]       ~ ["a", "b", "c"]

~ append:  []T + T   -> []T   (add one element at the end)
[1, 2] + 3               ~ [1, 2, 3]
["a"] + "b"              ~ ["a", "b"]

~ prepend: T   + []T -> []T   (add one element at the front)
0 + [1, 2]               ~ [0, 1, 2]
```

Both sides must agree on the element type — `[]Num + []Text` (or `[]Num + Text`) is a
type error. The forms are mutually exclusive (an array `[]T` can never equal its own
element `T`), so even nested arrays disambiguate cleanly: `[][]Num + []Num` is an
**append** (the `[]Num` is a single new row → `[][]Num`), while `[][]Num + [][]Num` is a
**concat**. `[]T + []T` is the same as the spread `[<-a, <-b]` and shares its element-copy
lowering, so it is element-repr-correct for `[]Num`, `[]Text`, and nested arrays alike.
(See `examples/array_concat.ql`.)

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
`NotOk("error")`), and a **pattern-bound payload carries its concrete type**, so it is
*usable* at the match site — `Ok("x") ? Ok(s) => s.size` binds `s : Text`, and passing
`s` to an [overload set](#overloading) dispatches to the `Text` member (not a generic
fallback). This holds across a function boundary too: a function returning `Ok("x")`
(whether its return type is inferred or annotated `-> Result`) hands the caller a usable
`Text` payload, and a `-> Result` whose branches are `Ok(Text)` / `NotOk(Text)` — the
`getEnv`/`getOpt` shape — carries **both** arms' payloads. (See `examples/result.ql` and
`examples/result_payload.ql`.)

A constructor pattern's argument must be **irrefutable** — a binding (`Ok(x)`) or the
wildcard (`Ok(_)`). A literal or nested constructor there (`Ok(1)`, `Ok(Ok(x))`) is a
compile error: match dispatch tests the constructor tag only, so such a pattern would
silently match *any* payload of the variant. Bind the payload and compare it in the arm
body instead (`Ok(n) => n == 1 ? … : …`).

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

### Tail self-recursion is optimized to a loop (guaranteed)

When a function returns a call **to itself in tail position** — i.e. the self-call is
the function's whole result, with nothing left to do to it — the compiler **guarantees**
it is lowered to a loop (the parameters become loop-carried slots and the call becomes a
back-edge jump) instead of a stack-pushing call. So a tail-recursive function runs in
**constant stack** and will not overflow, however deep the recursion:
```quilon
count = (n :: Num, acc :: Num) -> Num =>
  n == 0 ? acc : count(n - 1, acc + n)   ~ the self-call IS the `:` branch → tail position
```
Tail position flows through the constructs that yield a value directly: `?`/`|` match
arms, `if`/ternary branches, the tail of a `< >` block, and a `|>` pipeline. A self-call
**not** in tail position (e.g. `n * fact(n - 1)`, whose result is multiplied first) stays
ordinary recursion, as does a tail call to a *different* function (general/mutual tail
calls are a later follow-up). This is codegen-only — there is no surface syntax for it.
(See `examples/tail_recursion.ql`, which recurses 1,000,000 deep.)

### Closures — capture by `=` (value) vs `:=` (reference)

A function written **inside** another function's body is a **closure**: it can read the
enclosing locals it refers to. How each captured name is captured is decided **by the
operator that bound it** — there is no capture list and no marker, mirroring the
mutability rule for [variables](#variables) and [records](#mutation-in-place-field-writes--setters):

- a name bound with **`=`** is captured **by value** — a frozen, read-only snapshot taken
  when the closure is created;
- a name bound with **`:=`** is captured **by reference** — a single shared, mutable cell.
  Writes through it (from inside the closure or from the enclosing code) are visible to
  everyone sharing it, and the cell survives even if the closure outlives the frame that
  created it.

```quilon
^ = () -> Num => <
  total := 0                 ~ `:=` -> captured BY REFERENCE
  bump = n => <
    total := total + n       ~ writes the SHARED cell; the effect persists across calls
    total
  >
  bump(10)                   ~ total -> 10
  bump(20)                   ~ total -> 30  (same cell)

  base = 7                   ~ `=`  -> captured BY VALUE (a frozen copy)
  addBase = x => x + base

  total + addBase(5)         ~ 30 + 12 = 42
>
```

A non-capturing nested function may **recurse** (`fact = n => … fact(n-1) …`); nested
closures may capture from any enclosing frame (the shared `:=` cell is threaded through
every level), and a closure value may itself be captured by another closure and called.

Closures are **monomorphic** in this milestone: parameters and captured values are
concrete-typed (the capture rule needs no type variables). Capturing a polymorphic value,
generic closures, passing a closure as a function **parameter**, and returning a closure
from a function (higher-order across frames) are deferred — see
[Known limitations](#known-limitations). (See `examples/closures.ql`.)

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

- **Arithmetic:** `+ - * / %` (and `-x`). `+` is an [overload set](#overloading): `Num + Num` adds, `Text + Text` concatenates, and on arrays it concatenates / appends / prepends (`[]T + []T`, `[]T + T`, `T + []T`, all yielding a new `[]T` — see [Array concatenation](#array-concatenation--)). `%` is the f64 remainder and works on fractional operands too (`7.5 % 2` → `1.5`); the result takes the **dividend's** sign (`-7 % 3` → `-1`, `7 % -3` → `1`), like C `fmod` / Rust `%`.
- **Comparison:** `== != < <= > >=`. Over `Num` and (lexicographically) `Text`; all return `Bool`. Each is a [user-overloadable operator](#operator-overloading).
- **Logical:** `&& || !` (short-circuit).

> **`<` and `>` vs. `< >` blocks.** `<` and `>` double as the block delimiters. A `<`
> after a complete operand is always less-than (a block can't start mid-expression). A
> `>` is the **block close** only when it is the **last token on its line** (`>`
> followed by only spaces/tabs then a newline or end-of-file); any other `>` — one with
> more on the same line, like `a > b` — is the greater-than operator. So `a > b` works
> everywhere; the only rule is *don't end a line with a comparison `>`* (write the right
> operand on the same line). `<=`/`>=`/`>>` are distinct tokens and unaffected.

> **Statement boundaries — line-first `(` / `[` / `{`.** Quilon has no statement
> separator, and the grammar is newline-insensitive except for **two** line-aware rules:
> the line-final `>` above, and this one — a `(`, `[`, or `{` that is the **first token
> on its line** never continues the previous expression as a call, index, or record
> constructor; it begins a **new statement**. Call arguments, index brackets, and
> constructor braces must open on the **same line** as the expression they apply to. A
> continuation line may still start with `.`, `|>`, or an operator, and an argument list
> (or a constructor body) opened on its expression's line may span lines.
> ```quilon
> ~ (statements inside a `< >` block / `^` body)
> ~ OK — these all continue the expression:
> sum = add(40,
>   2)                                  ~ `(` opened on add's line; args may span lines
> total = nums.map(n => n * 2)
>   .reduce(0, (acc, n) => acc + n)     ~ `.`-led line chains
> p = Point {
>   x = 1, y = 2 }                      ~ `{` opened on Point's line; body may span lines
>
> ~ OK — a line-first `(`, `[`, or `{` is a NEW statement:
> x = f()
> (1 + 2) |> print                      ~ not the call `f()(1 + 2)`
> b = a
> [3, 4].each(n => print(n))            ~ not the index `a[3, 4]`
> e = origin
> { x = 9, y = 9 }                      ~ not the constructor `origin { x = 9, y = 9 }`
>
> ~ DON'T — a call may not open its argument list on the next line:
> x = f
> (10)                                  ~ NOT the call `f(10)`: `(10)` is a new statement
> ```
> (See `examples/statements.ql`.)
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

### Iteration — array methods + recursion
Quilon has **no `for`/`while` loop**. A collection is iterated with the built-in
[array methods](#array-methods): `.each` runs a body for its side effects (the direct
replacement for a side-effecting loop), and `.map`/`.filter`/`.reduce` transform or fold
without any mutable accumulator. Each takes a lambda the compiler inlines per element:
```quilon
nums = [1, 2, 3]
nums.each(n => print(n))              ~ side effects; returns the receiver (chainable)

sum = nums
  .map(n => n * 2)                    ~ [2, 4, 6]
  .reduce(0, (acc, n) => acc + n)     ~ 12
```
When iteration doesn't fit a method, use **recursion**: a self-tail-call is
[guaranteed to be lowered to a loop](#tail-self-recursion-is-optimized-to-a-loop-guaranteed),
so even deep recursion runs in constant stack. (See `examples/iteration.ql`.)

### Ranges — infix `lo <- hi`
The infix `<-` operator builds an **inclusive** `[]Num`:
```quilon
1 <- 4          ~ [1, 2, 3, 4]
4 <- 1          ~ [4, 3, 2, 1]   (descends when the left end is larger)
5 <- 5          ~ [5]            (single point)
```
It is pure **array sugar** — there is no distinct `Range` type; the result *is* a
`[]Num`, so it composes with `.size`, indexing `[i]`, and the [array methods](#array-methods):
```quilon
r = 2 <- 5      ~ [2, 3, 4, 5]
n = r.size      ~ 4   (inclusive count = |hi - lo| + 1)
first = r[0]    ~ 2
r.each(x => print(x))   ~ a range iterates with `.each` like any array
```
Both ends are full `Num` expressions (they may be dynamic, not just literals); the
direction (ascending vs descending) is decided at runtime. (See `examples/ranges.ql`.)

### Spread in literals
The **prefix** `<-` splices a source's contents into an array or record literal:

- **Array spread** `[<-xs, 4, 5]` builds a new array = every element of `xs`, then
  `4, 5`. Multiple spreads are allowed and applied left-to-right: `[<-a, <-b]`, or
  `[0, <-a, <-b, 9]`. The spread source must be an array with the same element type as
  the rest of the literal; `[]Text`/`[]Num`/nested-array elements all splice correctly.
  `[<-xs]` on its own is a copy of `xs`.
- **Record functional-update** `{<-p, x = 9}` builds a new record copying every field of
  `p`, then applying the overrides. Later entries override earlier ones (left-to-right),
  and an entry naming a field not in `p` **adds** it. If `p` is a **named** record and the
  result reproduces that type's fields exactly (only overriding existing fields, adding
  nothing), the result keeps the **named type and its methods**; otherwise it is an
  anonymous record.

```quilon
xs = [1, 2, 3]
ys = [<-xs, 4, 5]        ~ [1, 2, 3, 4, 5]
zs = [0, <-xs, <-ys]     ~ [0, 1,2,3, 1,2,3,4,5]

Vec = { x :: Num, y :: Num, sum = => it.x + it.y }
a = Vec { x = 10, y = 20 }
b = { <-a, x = 5 }       ~ still a Vec: b.sum() → 25
```

**Range vs. spread — the disambiguation rule.** `<-` is now BOTH the infix inclusive
range (`lo <- hi`, between two complete expressions) AND the prefix spread. They are told
apart purely by **position**: a `<-` that is the **first token of a `[ ]` element or a
`{ }` field** is a spread; a `<-` that follows a complete expression is the range
operator. So:

- `[1 <- 4]` is a **one-element** array whose sole element is the range `[1,2,3,4]`
  (the `<-` follows the complete expression `1`).
- `[<-xs, 4]` **spreads** `xs` (the `<-` begins the element).
- Inside a spread the source is a full expression, so `[<-1 <- 4]` spreads the range
  `1 <- 4` — i.e. `[1, 2, 3, 4]`.

(See `examples/spread.ql`.)

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
- The built-in modules are `core.io` (I/O), `core.test` (assertions), and `core.cli` (argv/env helpers); their members are real functions.
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

## Assertions — `<< core.test`

In-language assertions for **self-verifying programs and examples**. A holding
assertion does nothing; a **failing** one prints a message to **stderr** and exits
the process with code **101** (the Rust-panic convention, distinct from the 0 a
passing program exits with), so a broken program fails loudly in CI. Every example
in `examples/` is written this way: it asserts each result it demonstrates and exits
0 on success — the examples gate runs them all under the JIT and native AOT.

| Function | Effect |
|----------|--------|
| `assert(cond :: Bool) -> $` | The primitive. If `cond` is false, print `assertion failed` to stderr and exit `101`; otherwise do nothing. Returns `$` (Unit). |
| `assert(cond :: Bool, opts :: AssertOpts) -> $` | Same, but on failure print `opts.message` instead of the default. An [overload](#overloading) of `assert`. |
| `AssertOpts` | Options record for `assert`: `{ message :: Text }`. The extensible knob (more options may be added later). Records are nominal, so construct it by name: `AssertOpts { message = "..." }`. |
| `assertEq(actual, expected) -> $` | Assert `actual == expected`; on failure prints **expected** then **actual** to stderr before failing. An [overload set](#overloading) over `Num`/`Text`/`Bool`. |
| `assertNotEq(a, b) -> $` | Assert `a != b`; prints the (equal) value on failure. Overloaded over `Num`/`Text`/`Bool`. |
| `assertOk(r :: Result) -> $` | Assert `r` is `Ok`; fail on `NotOk`. |
| `assertNotOk(r :: Result) -> $` | Assert `r` is `NotOk`; fail on `Ok`. |

```quilon
<< core.test
^ = () -> $ => <
  assert(1 + 1 == 2)
  assert(1 + 1 == 2, AssertOpts { message = "math is broken" })
  assertEq(6 * 7, 42)
  assertNotEq("a", "b")
  assertOk([10, 20].at(0))       ~ Ok in bounds
  assertNotOk([10, 20].at(9))    ~ NotOk out of bounds
>
```

`assertEq`/`assertNotEq` render values with `eprint`, so their failure messages are
precise for `Num`/`Text`/`Bool`; other types (records, arrays, sum payloads) get only
the generic `assertion failed` line until a `toText` exists. The whole module is
pure Quilon (`corelib/test.ql`) built on `assert`, `==`/`!=`, and pattern-matching —
its only native primitive is a process-exit intrinsic. (See `examples/assert_demo.ql`.)

---

## Entry point

Every executable defines `^` (main); the compiler generates a C-compatible `main()` wrapper (and initializes the GC).
```quilon
^ = () -> Num => 42                              ~ no args/env
^ = (args :: []Text) -> Num => args.size         ~ command-line arguments
^ = (args :: []Text, env :: [][]Text) -> Num => args.size   ~ args + environment
```
**Arguments & environment.** `^` may declare, in order, two typed parameters that the
generated `main()` wrapper fills from the C `argc`/`argv`/`envp`:
- `args :: []Text` — the command-line arguments (argv), **including** `argv[0]` (the
  program name), so `args.size` is always at least 1, and `args[i]` is the *i*-th
  argument as a `Text`.
- `env :: [][]Text` — the environment, as an array of `[key, value]` pairs. Each inner
  array holds exactly two `Text`s: an entry `KEY=val` is split on its **first** `=`
  (so `KEY=a=b` becomes `[KEY, a=b]`); an entry with no `=` becomes `[entry, ""]`.

Both are real Quilon arrays — `.size`, `[index]`, and the array methods work on them.
`quilon run <file> [args...]` and a native build agree on `args`: under `run`, the
program sees `argv = [<file>, <args...>]` (the `quilon`/`run` CLI prefix is stripped and
the `.ql` path becomes `argv[0]`), so `quilon run f.ql a b c` gives the same `args.size`
and trailing arguments as a native `./f a b c` — `argv[0]` is the `.ql` path rather than
the compiled binary's path, but everything the program indexes past it matches. (The
legacy `^ = (argc :: Num, argv :: Num)` form, where `argv` was a placeholder `0`, still
compiles for backward compatibility but is superseded by `args :: []Text`.) Any other
`^` signature (e.g. a non-`Text` array element, or an unexpected parameter) is a
compile-time error, reported by `check` as well as `run`/`build`.

**Exit code:** if `^`'s body evaluates to a `Num`, that value is the exit code. If the body is **not** a `Num` (e.g. a side-effecting block), the program exits **0** — so an effect-only `main` needs no trailing `0`. (This implicit-0 applies only to `^`; ordinary functions always return their last expression's value.)

(See `examples/hello_world.ql` and `examples/args.ql`.)

---

## CLI helpers — `<< core.cli`

Thin, pipe-friendly helpers over the entry point's `args :: []Text` and
`env :: [][]Text`. The data is always the **first** parameter, so
`env |> getEnv("PATH")` and `args |> hasFlag("-v")` read naturally.

| Function | Result |
|----------|--------|
| `getEnv(env :: [][]Text, key :: Text) -> Result` | Find the pair whose `[0]` equals `key`; `Ok(value)` (its `[1]`) if present, else `NotOk`. |
| `hasFlag(args :: []Text, flag :: Text) -> Bool` | `true` when the bare flag appears in `args`. The name works **with or without** a leading `--` (so `"verbose"` and `"--verbose"` both match an arg `"--verbose"`). |
| `getOpt(args :: []Text, name :: Text) -> Result` | Collect the option's values (argv[0] skipped), recognising both `--name value` and `--name=value`; the name works with or without `--`. Returns `Ok([]Text)` of the values in argv order (an option may repeat), or `NotOk(name)` when no value is found — the name never appears, or appears only as a trailing `--name` with nothing after it. (The `--name=value` form always supplies a value, even the empty one in `--name=`.) |

```quilon
<< core.cli
^ = (args :: []Text, env :: [][]Text) -> Num => <
  home :: Text = env |> getEnv("HOME") ? | Ok(v) => v | NotOk(_) => "?"
  verbose :: Bool = args |> hasFlag("-v")
  outputs :: []Text = args |> getOpt("--out") ? | Ok(vs) => vs | NotOk(_) => args.filter(x => false)
  verbose ? 0 : outputs.size
>
```

The whole module is pure Quilon (`corelib/cli.ql`) — built only from the array
methods (`.find`/`.filter`/`.reduce`), array indexing, ranges (`<-`), and the `Text`
methods (`.slice`/`.indexOf`/`.contains`/`==`/`+`); it adds no compiler intrinsics.
(See `examples/cli.ql`.)

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

Add `--debug` (or `-g`) to emit **DWARF line-number debug info** for source-level
debugging — a debugger (`gdb`/`lldb`) can then set breakpoints, step, and show
backtraces in terms of `.ql` lines:
```bash
quilon build program.ql --debug -o program
llvm-dwarfdump --debug-line program        # lists the .ql file + its line table
gdb ./program                              # break/step by .ql line
```
Builds are already unoptimized, so `--debug` only *adds* the debug info; without
it the binary carries none. This first phase covers line tables and per-function
scopes only — local-variable and full-type debug info is planned for a later
phase. Debug info is attributed to the program's own source file; functions
pulled in from imported modules (`<<`) currently carry no line info, because a
`Span` records only a byte offset and not which module file it came from (the
same limitation noted for the type oracle). Multi-file line info is a follow-up.

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

To stay robust on hostile or machine-generated input, the parser also caps how
deeply expressions may nest: nesting more than **128 levels** of parentheses,
array/record literals, block statements, `[]T` element types, constructor
patterns, or chained prefix operators is a parse error
(`expression nesting too deep …`) rather than a crash.
Ordinary code nests only a handful of levels, so this limit is reached only by
pathological input.

---

## Feature matrix

✅ = works end-to-end with a passing run test · 🚧 = partial · ❌ = not yet

| Feature | Status |
|---|---|
| `^` entry point, native compile + JIT `run` | ✅ |
| `Num`, arithmetic, comparison, logical, ternary | ✅ |
| `Text` built-in: literals, `+`, `.size`, `.length` | ✅ |
| `Text` comparison: `==`/`!=` (equality), `<`/`<=`/`>`/`>=` (lexicographic) | ✅ |
| `Text` methods: `split`/`trim`/`trimStart`/`trimEnd`/`replaceAll`/`replace`/`contains`/`indexOf`/`slice`/`toUpper`/`toLower` (chainable; grapheme-based) | ✅ |
| Ad-hoc overloading: same-named typed defs, exact-type dispatch | ✅ |
| Operator overloading (`+`, comparisons, … on user types); built-ins as overloads | ✅ |
| `Bool` | ✅ |
| `Unit` type / value (`$`) | ✅ |
| Arrays: literals, `.size`, `[index]` | ✅ |
| Array methods: `map`/`filter`/`reduce`/`each`/`find`/`at` (chainable; lambda args inlined) | ✅ |
| Array `+`: concat `[]T + []T`, append `[]T + T`, prepend `T + []T` → new `[]T` (non-mutating) | ✅ |
| Records + field access | ✅ |
| Named record types + methods (`it`) | ✅ |
| In-place mutation of `:=` records: field writes (`obj.f := v`) + setter methods | ✅ |
| Functions, recursion, blocks, type inference | ✅ |
| Guaranteed self-tail-call optimization (tail self-recursion → loop, constant stack) | ✅ |
| Closures: lexical capture (`=` by value / `:=` by reference), monomorphic | ✅ |
| Pipe `\|>` (first-arg injection) | ✅ |
| Ranges: infix `lo <- hi` → inclusive `[]Num` (descends when `lo > hi`) | ✅ |
| Spread: prefix `<-` in literals — array splice `[<-xs, 4]`, record update `{<-p, x = 9}` | ✅ |
| Pattern matching (numbers, wildcard, identifiers, sum-type variants) | ✅ |
| User-defined sum types (`/` separator), exhaustive matching, payload binding | ✅ |
| `Result` as a normal predefined sum type (`Ok`/`NotOk`) | ✅ |
| Sum-type payloads: `Num` / `Bool` / `Text` | ✅ |
| Concrete `Result` payloads: a bound `Ok`/`NotOk` payload is usable at its real type (overload dispatch, across `-> Result` fn boundaries) | ✅ |
| Modules: `<< core.io`, `<< core.test`, `<< core.cli`, file-path imports, `>>` exports | ✅ |
| I/O: `print` / `eprint` / `write` | ✅ |
| Assertions: `<< core.test` (`assert` (+ `AssertOpts` message) / `assertEq` / `assertNotEq` / `assertOk` / `assertNotOk`; fail → exit 101) | ✅ |
| CLI helpers: `<< core.cli` (`getEnv` / `hasFlag` / `getOpt`; both `--name value` and `--name=value`; flag names with or without `--`) | ✅ |
| Conservative GC (Boehm) | ✅ |
| `Text` (and nested arrays) in records/arrays, or as a sum-type payload (`Ok(text)`) | ✅ |
| `^` receives `args :: []Text` (argv) and `env :: [][]Text` (environment pairs) | ✅ |
| Lambdas (`x => …`) as array-method arguments (inlined per element) | ✅ |
| Generics / type variables (overloading is the only polymorphism) | ❌ |
| Overloaded name passed as a value, or a closure as a param / return (higher-order) | ❌ |
| Generic / polymorphic-capturing closures | ❌ |
| String interpolation | ❌ |

---

## Known limitations

0.9 is a stable **core**, not the whole language. Notably:

- **No generics.** Overloading (ad-hoc, exact-type dispatch) is the only polymorphism; there are no type variables. The module system is minimal (`core.io`/`core.test` built-ins + file-path imports).
- **Closures are monomorphic.** Lexical capture works end-to-end (`=` by value / `:=` by reference; see [Closures](#closures--capture-by--value-vs--reference)), including recursion of non-capturing nested functions, capture across multiple nesting levels, and capturing-then-calling another closure. Deferred to a later milestone (they need the closure's type threaded through inference / defunctionalization): capturing a *polymorphic* value, *generic* closures, passing a closure **as a function parameter**, and **returning a closure from a function**. A closure used in an unsupported position is rejected at compile time (e.g. an unannotated function parameter that is called reports `Not a function`), never miscompiled.
- **Overloads (and closures) resolve at direct call sites only.** Passing an overloaded name as a value (higher-order use) is not yet supported.
- **Sum-type payloads mixing types across variants behind one value aren't unified yet.** Each variant's payload slots have a fixed representation sized to the widest variant; a single value carries one variant's payload. Distinct payload *types* per slot across variants (e.g. a position that is `Num` in one variant and `Text` in another) is a deferred follow-up — the built-in payload set (`Num`/`Text`/`Bool`, consistent per position) works.
- A `Text` value bound from an `args`/`env` element supports the full `Text` API
  (`.size`/`.length`/`+`/comparison), and — like a bound `Result` payload — dispatches an
  [overload set](#overloading) by its concrete `Text` type.

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
