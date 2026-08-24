# Quilon Language Reference

**Version:** 0.9.1 (stable basics — the core is solid and verified end-to-end, but the language is **not** yet feature-complete; see [Known limitations](#known-limitations)).

Quilon is a statically-typed, **symbol-based** language (no control-flow keywords) that compiles to native code via LLVM. Every example below has a passing end-to-end test: each `examples/*.ql` program is **self-asserting** — it verifies its own results in-language with `<< core.test` and exits 0 (a failing assertion aborts with exit 101), under both the JIT (`quilon run`) and native AOT.

---

## Design principles

Quilon's identity, and the rules that guide its design:

- **No keywords.** Every construct is punctuation, not words — *nothing was removed from the language; the words were.* Branching is `?` / `|`, the entry point is `^`, import/export are `<<` / `>>`, mutability is `:=`, sum-type alternatives are `/`. Not one word is reserved: `if`, `while`, `for` and the rest are ordinary identifiers you may bind.
- **Symbols mirror notation that already exists.** A symbol reuses a notation the world already has rather than inventing one: `/` separates sum-type alternatives the way you already write "red / green / blue".
- **The playful choice wins.** On a genuine toss-up, the more delightful option is picked — `^` for the entry point, `$` for Unit. Syntax is allowed a sense of humor.
- **Deliberate simplicity.** The smallest system that works: no generics (ad-hoc overloading is the only polymorphism), no `while`, no interfaces, a single `Num` type. Features are omitted on purpose.
- **Fail loud, never silent.** Invalid inputs and meaningless operations must *fail* — never silently no-op, clamp, or return a magic sentinel. A statically-determinable problem is a **compile error**; anything else is a runtime error on stderr with a non-zero exit, saying [where it happened](#error-messages). (Hence `Text.indexOf → Ok(Num)/NotOk` rather than a `-1` sentinel, and `Text.replace`'s count/empty-argument checks failing rather than clamping.)
- **No magic.** No hidden coercions, no implicit dispatch. Overloads are exact-typed; operators mean what they say.
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
| `<-` (prefix) | Spread inside a `[ ]` / `{ }` literal ([rule](#spread-in-literals)) | `[<-xs, 4]` · `{<-p, x = 9}` · `Vec {<-p, x = 9}` |
| `?` `\|` `_` | Pattern match | `v ? \| 0 => "zero" \| _ => "other"` |
| `/` | Division **or** sum-type variant separator | `a / b` · `Color = Red / Green` |
| `[\| \|]` | [Map](#maps) / [Set](#sets) pipe fence (`=>` = "maps to") | `[\|"a" => 1\|]` (map) · `[\|1, 2\|]` (set) |
| `+-` `-+` | [Set intersection](#sets) (one symmetric operator) | `a +- b` ≡ `a -+ b` |
| `` ` `` (in a string) | [Interpolation](#string-interpolation-and-the-render-operator) hole · `` `` `` = one literal backtick | `` "hi `user.name`" `` |
| `` ` `` (as a name) | The overloadable **render** operator — a type's `Text` rendering | `` ` = () -> Text => "..." `` |
| `? :` | Ternary | `x < 0 ? -x : x` |
| `@` (name prefix) | A [leaf IO primitive](#concurrency--colorless-implicit-futures--in-progress) (corelib-only; user code calls, never declares) | `@sleep(1)` |
| `~` | Comment (to end of line) | `~ a note` |

There are **no keywords**: `if`/`return` etc. are all expressed with symbols, and there
are no loop constructs at all — iteration is via [array methods and recursion](#iteration--array-methods--recursion).
No word is reserved either, so `if = 5` or a function named `while` is perfectly legal.

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

Escapes inside a literal: `\n`, `\r`, `\t`, `\"`, `\\`, `\<` (a literal `<`, which would
otherwise open a block), and `\e` — the ESC byte that leads an ANSI terminal sequence
(`"\e[1m" + text + "\e[0m"`). Any other escape is a lex error.

#### Text methods

`Text` carries **built-in, compiler-provided methods**, called as `text.method(...)` and
freely chainable. User-visible indices and lengths are **grapheme-based** (matching
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
| `repeat(count :: Num)` | `Text` | `count` copies back to back (`"^".repeat(3)` → `"^^^"`); `0` yields `""` |

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

Like the [array methods](#array-methods), these are **reserved on `Text`**: a same-named
user overload on another type is fine, but on a `Text` receiver the built-in wins. `split`
yields a plain `[]Text`, so it composes with `.size`, `[i]`, the
[array methods](#array-methods), and array `+`. There is **no `join`** — collapse a `[]Text`
with `reduce` + `+`.

`replace`/`replaceAll`/`repeat` **fail loudly**, never silently no-op or clamp. Rejected: an
empty `from`; a `replace` `count` that is `<= 0` or exceeds the occurrences present; a
negative or fractional `repeat` count. Literal cases are compile errors (`"a".replace("a",
"b", 0)`, `"aa".replace("a", "b", 5)`); computed ones are a [located
diagnostic](#error-messages) at run time, exit `101`. Use `replaceAll` for "replace
everything"; `replace(count)` means exactly that many.

(See `examples/text.ql` and `examples/text_methods.ql`.)

#### String interpolation and the render operator (`` ` ``)

A string literal may contain **interpolation holes** — an expression wrapped in
backticks — which are rendered to `Text` and spliced in:

```quilon
"hi `user.name`"      ~ splices the rendered value of user.name
"sum: `a + b`"        ~ any expression, not just a variable
"port `getPort()`"    ~ a call
```

A hole can be **any expression**, and its value can be of **any type** — every type is
renderable. To write a **literal backtick**, double it: `` `` `` yields one `` ` `` (never
starts a hole). A plain string with no holes is an ordinary `Text` literal.

**One render path.** Both interpolation and `print`/`eprint` render a value by invoking
its `` ` `` (backtick) operator. Every built-in type has a **default** `` ` ``; **any**
user type may **override** its rendering by defining its own `` ` `` operator — declared
method-style (like other [record methods](#named-record-types-with-methods)), with `it`
bound to the instance, returning `Text`, and free to use interpolation itself:

```quilon
User = {
  name :: Text,
  age  :: Num,
  ` = () -> Text => "User(`it.name`, `it.age`)"   ~ override: `it` is the instance
}
~ Now both `print(u)` and `"`u`"` render as  User(Ada, 36)
```

So `print(u)` and `` "`u`" `` take the same path through `u`'s `` ` `` — the override when
present, the built-in default otherwise. (A `` ` `` that renders `it` *wholesale* falls
back to the default rather than recursing forever.)

**Default rendering** (the built-in `` ` `` per type):

| Type | Renders as | Example |
|------|-----------|---------|
| `Num` | integer-valued → no decimals; else shortest round-trip | `5`, `5.5`, `0.5` |
| `Bool` | `True` / `False` — **capitalized** (deliberately unlike the `true`/`false` literals) | `True` |
| `Text` | itself | `hi` |
| record | the **type name** (unless overridden) | `Point` |
| sum type | the **variant/constructor name** (unless overridden) | `Green`, `Ok` |
| array | length **≤ 10** → full `[a, b, c]` (each element via its own `` ` ``); length **> 10** → truncated `[first <- last]` | `[1, 2, 3]`, `[1 <- 100]` |

There are **no format specifiers** (width/precision/etc.). (See `examples/interpolation.ql`.)

### `Bool`
`true` / `false` (the literals are lowercase; note that a `Bool` *renders* as capitalized
`True`/`False` — see [interpolation](#string-interpolation-and-the-render-operator)).

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

Indexing is **checked** (fail loud, never silent): an out-of-bounds, negative, or NaN index
is a runtime error naming the read that failed ([shape](#error-messages)), exit status 1 —
never a raw memory read. A **fractional** in-range index truncates toward zero (`nums[1.7]`
reads `nums[1]`); with one unified `Num`, index arithmetic like `size / 2` legitimately
produces fractions. Use [`at(n)`](#array-methods) for the non-aborting `Ok`/`NotOk` form when
an index might be out of range — see the computed-index case at the end of
`examples/array_methods.ql`.

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

### Maps

A `Map` is a **built-in parametric collection** — like `[]T`, not a user-defined generic —
written with a **pipe fence** `[|K => V|]` (`=>` reads "maps to"). It is immutable, keyed by
`Num`/`Text`/`Bool`, and read through `.get` (which returns a `Result` — there is no bracket
indexing on a map). Full reference: [`docs/collections/map.md`](collections/map.md) (and `examples/maps.ql`).

### Sets

A `Set` is a **built-in parametric collection** — like `[]T`, not a user-defined generic —
written with the same **pipe fence** `[|T|]` (which keeps a set literal distinct from an array).
It is immutable, holds unique `Num`/`Text`/`Bool` elements, and supports set algebra
(`+` union, `-` difference, `+-`/`-+` intersection). Full reference:
[`docs/collections/set.md`](collections/set.md) (and `examples/sets.ql`).

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
- **Payloads are built-in scalars or a named record** — `Num`, `Text`, `Bool`, `$`
  (Unit), or a previously-declared **record** type. There are no type variables (no
  generics), but a variant may take several payload fields (e.g. `Rect(Num, Num)`). A `$`
  payload carries no value — it's the "this variant has no data" case (see `Ok($)` below).
- A **named record** payload lets a sum carry structured data — `Method = Get / Post(Body)`
  where `Body` is a record. The record must be declared **above** the sum (no hoisting),
  and a match arm binds it at its full type, so `Post(b) => b.payload` reads its fields
  and calls its methods. (Nesting another **sum** as a payload is not yet supported; see
  `examples/nested_composites.ql`.)
- At a given payload position, every variant with a concrete (non-`$`) field there must
  agree on its type — including the named-record case; `$` may coexist with a concrete
  type at the same position (`Done($) / Pending(Num)` is fine, `A(Num) / B(Text)` and
  `Wrap(Body) / Plain(Num)` are rejected).
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

Every `Result` shares **one uniform layout** regardless of its payload, so a `Result`
carrying *any* payload — `Num`, `Text`, `[]Text`, a composite — passes through a generic
`(r :: Result)` parameter or return. This is what lets `assertOk` / `assertNotOk`
([`core.test`](#coretest--assertions)) accept a `Result` of any shape, including the
composite-payload results of `getEnv` / `getOpt` (see `examples/cli.ql`). Extracting a
payload still needs its concrete type in scope at the match site (there are no generics),
but *matching by variant* (`Ok` vs `NotOk`) works on any `Result` anywhere.

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

### A top-level binding must be a constant or a function

A binding written outside any function is a **global**, and a global's initializer has to
be a constant already: no Quilon code runs before `^`, so there is nowhere to compute one.
The value may be a `Num`, `Bool` or `$` literal, or a function — mutable (`:=`) globals
included, and a `:=` global is writable from inside a function like any other.

```quilon
limit = 10              ~ fine
enabled = true          ~ fine
scale = n => n * 3      ~ fine — a function value
counter := 4            ~ fine — and writable from a function

doubled = limit * 2     ~ error: has to be computed
greeting = "hi"         ~ error: Text is a { pointer, length } pair, built at runtime
sizes = [1, 2]          ~ error: an array is built at runtime
origin = { x = 0 }      ~ error: so is a record
```

A rejected binding reports what it is and how to fix it — move the work into the function
that uses it. Anything computed is perfectly ordinary *inside* a function; the restriction
is only about globals. (See `examples/globals.ql` and `examples/global_computed.ql`.)

---

## Mutation: in-place field writes & setters

Mutability is decided by the binding operator, and governs in-place mutation as well as
reassignment:

- An `=`-bound instance is **immutable**: no field writes, and calling a setter method on it
  is a compile error.
- A `:=`-bound instance is **mutable**: a direct field write `obj.field := value` (in place,
  no re-allocation) and any **setter** method.
- One exception, by type rather than by binding: a [`Site`](#call-site-locations--site) is
  read-only — a location is a value, not a variable — so writing one of its fields is an
  error even through a `:=` binding.

```quilon
Counter = {
  value :: Num,
  bump = (by :: Num) => it.value := it.value + by   ~ setter: writes `it.value := …`
}

c := Counter { value = 30 }   ~ `:=` -> mutable
c.bump(5)                      ~ setter mutates in place -> value = 35
c.value := c.value + 7         ~ direct field write    -> value = 42
```

A method is a **setter** iff its body writes `it.field := …` (or calls another setter on
`it`) — no marker; the `:=` is the signal. A setter call requires a `:=` receiver:

```quilon
c = Counter { value = 30 }   ~ `=` -> immutable
c.value := 99                 ~ error: cannot write a field of immutable `c`
c.bump(5)                     ~ error: cannot call mutating method `bump` on immutable `c`
```

Getter methods carry no `it.field := …`, so they are callable on `=` instances too. (See
`examples/mutation.ql`.)

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

### Names resolve top to bottom

A call may only name something **already defined above it** — there is no hoisting. A
definition is in scope for its own body (so a function may recurse) and for everything
that follows it, but not for anything before it:
```quilon
^ = () -> Num => later()   ~ error: Undefined variable 'later'
later = () -> Num => 7
```
This holds for overload-set members too, which report the situation by name:
```quilon
h = () -> Text => g(1)     ~ error: cannot call 'g' before its definition
g = (n :: Num) -> Text => "a"
g = (t :: Text) -> Text => "b"
```
So **mutual recursion between top-level functions is not expressible**: whichever of the
pair comes first would have to call the other before it exists. Self-recursion (including
a recursive overload member calling itself) is unaffected — restructure a mutual pair into
one self-recursive function.

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
enclosing locals it refers to. How each name is captured is decided by **the operator that
bound it** — no capture list, no marker, mirroring the mutability rule for
[variables](#variables) and [records](#mutation-in-place-field-writes--setters):

- **`=`** captures **by value** — a frozen snapshot taken when the closure is created;
- **`:=`** captures **by reference** — one shared mutable cell. Writes through it, from
  inside the closure or outside, are visible to everyone sharing it, and the cell survives
  the frame that created it.

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

Closures are **monomorphic**: parameters and captured values are concrete-typed. Capturing a
polymorphic value, generic closures, and passing or returning a closure across frames are
deferred — see [Known limitations](#known-limitations). (See `examples/closures.ql`.)

---

## Overloading

Quilon has **explicit ad-hoc overloading** — the only polymorphism, since there are no
generics. Top-level definitions that share a name and each annotate their parameters *are*
an overload set; there is no marker:

```quilon
score = (n :: Num)  -> Num => n + 1       ~ the Num member
score = (s :: Text) -> Num => s.size      ~ the Text member

a = score(41)       ~ 42  — picks the Num member
b = score("abcd")   ~ 4   — picks the Text member
```

**Dispatch is by exact static argument type, with NO implicit coercion.** No match, or two
members sharing a parameter-type list, is a compile error listing the candidates:

```
error: No overload of 'score' matches argument types (Bool). Candidates: (Num), (Text)
```

- Every member must annotate **all** its parameters **and its return type** — the signature
  is what dispatch selects on, and a call has to know what it produces:
  ```quilon
  g = (n :: Num) => 1        ~ error: overload member 'g' (Num) has no return type
  g = (t :: Text) -> Num => 2
  ```
- A single ordinary `name = …` definition is **not** an overload set: it keeps full
  inference (unannotated params default to `Num`, return type inferred).
- A member joins its set where it is written, so a call resolves only against the members
  above it ([names resolve top to bottom](#names-resolve-top-to-bottom)).
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

(See `examples/overloading.ql`, and `examples/overload_dispatch.ql` for dispatch on
argument types that come out of an array element, a match, a call, or a lambda.)

---

## Expressions

- **Arithmetic:** `+ - * / %` (and `-x`). `+` is an [overload set](#overloading): `Num + Num` adds, `Text + Text` concatenates, and on arrays it concatenates / appends / prepends (`[]T + []T`, `[]T + T`, `T + []T`, all yielding a new `[]T` — see [Array concatenation](#array-concatenation--)). `%` is the f64 remainder and works on fractional operands too (`7.5 % 2` → `1.5`); the result takes the **dividend's** sign (`-7 % 3` → `-1`, `7 % -3` → `1`), like C `fmod` / Rust `%`.
- **Comparison:** `== != < <= > >=`. Over `Num` and (lexicographically) `Text`; all return `Bool`. Each is a [user-overloadable operator](#operator-overloading).
- **Logical:** `&& || !` (short-circuit).

> **`<` and `>` vs. `< >` blocks.** `<` and `>` double as the block delimiters. A `<`
> after a complete operand is always less-than (a block can't start mid-expression). A
> `>` is the **block close** only when it is the **last token on its line** (followed by
> only spaces/tabs then a newline or end-of-file); any other `>`, like `a > b`, is
> greater-than. So: don't end a line with a comparison `>`. `<=`/`>=`/`>>` are distinct
> tokens and unaffected.

> **Statement boundaries — line-first `(` / `[` / `{`.** Quilon has no statement separator,
> and the grammar is newline-insensitive but for two rules: the line-final `>` above, and
> this one — a `(`, `[`, or `{` that is the **first token on its line** begins a new
> statement rather than continuing the previous expression as a call, index, or constructor.
> Those must open on the **same line** as the expression they apply to, though once opened
> they may span lines; a continuation line may still start with `.`, `|>`, or an operator.
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

- **Array spread** `[<-xs, 4, 5]` builds a new array of every element of `xs`, then `4, 5`.
  Multiple spreads apply left-to-right (`[0, <-a, <-b, 9]`). The source must be an array of
  the literal's element type; `[]Text`, `[]Num`, and nested arrays all splice. `[<-xs]` alone
  copies `xs`.
- **Record functional-update** `{<-p, x = 9}` builds a new record copying every field of
  `p`, then applying the overrides. Later entries override earlier ones (left-to-right),
  and an entry naming a field not in `p` **adds** it. If `p` is a **named** record and the
  result reproduces that type's fields exactly (only overriding existing fields, adding
  nothing), the result keeps the **named type and its methods**; otherwise it is an
  anonymous record.
- **Naming the type you are building** — `Vec {<-p, x = 9}` — is the same update as a
  constructor. The stated target constrains the source: it must be **already that type** or
  an **anonymous record of exactly its shape** (same fields and types, nothing extra). A
  different named type is never accepted however similar (`Point` and `Other` stay
  distinct), and an anonymous record cannot fill a type declaring **methods**. Every declared
  field must end up provided, by the spread or an override.

```quilon
xs = [1, 2, 3]
ys = [<-xs, 4, 5]        ~ [1, 2, 3, 4, 5]
zs = [0, <-xs, <-ys]     ~ [0, 1,2,3, 1,2,3,4,5]

Vec = { x :: Num, y :: Num, sum = => it.x + it.y }
a = Vec { x = 10, y = 20 }
b = { <-a, x = 5 }       ~ still a Vec: b.sum() → 25
c = Vec { <-a, x = 5 }   ~ the same update, naming the type being built
```

**Range vs. spread.** `<-` is both the infix inclusive range (`lo <- hi`) and the prefix
spread, told apart by **position**: first token of a `[ ]` element or `{ }` field is a
spread, following a complete expression is the range. So:

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
- The built-in modules are `core.io`, `core.test`, `core.cli`, `core.time`, and `core.net`; their members are real functions. See the [Standard library](#standard-library) index for each module's API reference.
- `Text` and the operators are built-ins and need **no** import.
- A module exposes only its `>>`-exported items.

(See `examples/use_module.ql`, which imports `examples/mathlib.ql`.)

---

## Standard library

The corelib modules ship with Quilon; import one with `<< core.<module>`. Each has its
own focused API reference under [`docs/corelib/`](corelib/) — signatures, behavior, and a
small example per function.

| Module | Import | What it gives you |
|--------|--------|-------------------|
| [`core.io`](corelib/io.md) | `<< core.io` | Output to file descriptors and stdin: `print` / `eprint` / `write`, the `stdout` / `stderr` descriptors, and the deferred `@readStdin` line read. |
| [`core.test`](corelib/test.md) | `<< core.test` | In-language assertions for self-verifying programs, reporting the caller's `file:line:column`: `assert` (+ `AssertOpts`) / `assertEq` / `assertNotEq` / `assertOk` / `assertNotOk` / `failAt` (fail → exit 101). |
| [`core.cli`](corelib/cli.md) | `<< core.cli` | Pipe-friendly helpers over the entry point's `args` / `env`: `getEnv` / `hasFlag` / `getOpt`. |
| [`core.time`](corelib/time.md) | `<< core.time` | Time primitives: the `@sleep` pause and the monotonic `now()` clock. |
| [`core.net`](corelib/net.md) | `<< core.net` | Networking: the deferred `@tcpRequest` raw TCP request exchange the HTTP client sits on. |

`Text` and the operators are built-ins and need **no** import. The [concurrency model](#concurrency--colorless-implicit-futures--in-progress) that governs the `@` leaf primitives (`@readStdin`, `@sleep`) is language semantics — see that section.

---

## Call-site locations — `Site`

A function whose **last** parameter is a `Site` receives the location of the call — and a
call that leaves that argument off has it **filled in by the compiler**:

```quilon
whereAmI = (site :: Site) -> Text => "`site.file`:`site.line`:`site.column`"

^ = () -> $ => <
  print(whereAmI())        ~ prints e.g. demo.ql:4:9 — the location of THIS call
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

`line`, `column`, and `width` are always at least 1. `Site` is a built-in type name, so a
program cannot declare its own (as with `Result`) — though it may *build* one
(`Site { file = "…", line = 1, column = 1, excerpt = "…", width = 1 }`) and pass it on, and
`failAt` will report wherever it says.

**A `Site` is read-only.** A location is a value, not a variable: writing one of its fields
(`site.line := 9`) is a compile error however the value was reached — records alias, so a
write through a `:=` rebinding writes the same thing. That is what lets the compiler lower
each call site to one shared constant.

**Passing one explicitly forwards it.** That is the whole propagation rule, and it makes a
chain of wrappers report the *user's* call rather than the innermost hop (Rust's
`#[track_caller]`, as an ordinary argument):

```quilon
inner = (site :: Site) -> Num => site.line
outer = (site :: Site) -> Num => inner(site)   ~ forwards: reports where `outer` was called
plain = (site :: Site) -> Num => inner()       ~ does not: reports THIS line
```

Only a **top-level function's last** parameter can be filled in; a `Site` anywhere else is a
compile error rather than an argument nothing supplies — not before another parameter, not on
a lambda or nested declaration (called through a value, not by name), and not on a record
method (dispatched by receiver type). The arity a caller sees never counts it: `whereAmI()`
above takes no arguments at the call site.

Filling one in **costs nothing at run time**: the fields are compile-time constants, so each
call site is a read-only constant whose address the call passes — no allocation, no unwinder,
no debug info, and JIT and native builds report identically. Assert as often as you like, in
the hottest loop you have. (A site does cost image space: the record plus two relocations for
its `Text` fields.) [`core.test`'s assertions](corelib/test.md) are built on this; nothing
about it is specific to them. See `examples/call_site.ql`.

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

Both are real Quilon arrays — `.size`, `[index]`, and the array methods work on them — and
an element bound out of one is a full `Text`: the whole `Text` API, and
[overload](#overloading) dispatch by its concrete type.
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

## Memory

Quilon uses a **conservative garbage collector** (Boehm). Heap values (`Text`, etc.) are GC-managed — there is no manual free. In 0.9 this is the system's **dynamic `libgc`** (a documented build- and run-time dependency); a statically-linked / vendored GC is a post-0.9 goal.

---

## Concurrency — colorless implicit futures (🚧 in progress)

> Colorless implicit futures on cooperative fibers: IO returns type-invisible deferreds, only strict operations force them — concurrency follows data dependence, not program order.

> **Status: 🚧 in progress.** The model below is locked, and its core runs: the
> single-threaded fiber scheduler, the effect-only `@sleep` pause (`core.time`), the
> deferred-value `@readStdin` (`core.io`), and the networked `@tcpRequest` (`core.net`).
> Not yet: **overlap** as a showcase (two independent reads finishing in max-time rather
> than sum-time), which needs a primitive like `@get`; and the multicore (M:N) runtime.

Quilon's concurrency is **colorless**: you write ordinary, blocking-*looking* code and the
runtime overlaps independent IO for you. No `async`, no `await`, no `go`/`spawn`, no resolve
token, and no **function coloring** — a function that does IO is written and typed exactly
like one that doesn't. `async`/`await` colors every function on the IO path; Go and Loom
still need an explicit `go`. The nearest precedent is **promise pipelining** (E, Cap'n Proto).

**`@` marks leaf IO primitives only** — the stdlib/runtime primitives that actually do IO
(`http.get`, a file read, a socket recv, `sleep`). All user code is unmarked: a function that
transitively calls an `@` primitive is concurrency-capable for free, with **no propagation**
up the call chain. That absence of propagation is what makes the model colorless.

**Deferred values.** Calling an `@` primitive launches the IO and returns immediately with a
*deferred* value, without parking the caller. Deferred-ness propagates as the value flows —
passed as an argument, stored in a record or array, returned from a function — forcing
nothing along the way. That lazy threading is the *pipelining*.

**Forcing happens at the leaves.** A deferred value is forced — the fiber parks until it is
ready — only at a **strict** operation: arithmetic, comparison, pattern match (`?`), IO
output (`print`/`write`), and native calls. Values *launched before they are forced* therefore
overlap automatically, with nothing written to ask for it.

**Deferral is type-invisible.** A deferred `Text` types as `Text`, so it does not disturb
exact-type [overload resolution](#overloading).

**Structured & scoped.** Deferred tasks are scoped to their enclosing `< >` block: the block
forces and joins everything it launched before returning, and a panic propagates out.

**Why it can be colorless.** Each fiber is **stackful** (via `corosensei`), so any function
can park at a force point without the compiler rewriting it into a state machine.

**Determinism.** Pure results are fully deterministic. The **ordering of side effects** across
independent deferred IO is unspecified — the accepted cost of implicit overlap.

**A program's entry runs on the fiber scheduler only when it uses an `@` primitive**, so pure
programs are byte-identical (zero overhead).

### Runnable today

`core.time` — **`@sleep(seconds)`** takes a fractional `Num` and is effect-only (`-> $`): it
waits right there on the current fiber, then execution continues in program order. It carries
no value, so nothing defers or forces. **`now()`** reads a **monotonic** clock in seconds;
only *differences* between readings are meaningful. It is a plain (non-`@`) primitive —
reading the clock is instant and never parks. (See `examples/sleep.ql`.)

```quilon
<< core.time

^ = () -> Num => <
  start = now()
  @sleep(0.05)            ~ pause ~50ms, then continue
  now() - start >= 0.05 ? 6 * 7 : 0   ~ the sleep really waited → 42
>
```

`core.io` — **`@readStdin() -> Text`** reads one line from stdin. Being value-returning makes
it the deferred one: it launches the read, returns immediately, and is forced only where a
strict operation reads its bytes. At end-of-input it yields `""`. (See
`examples/readStdin.ql`.)

```quilon
<< core.io
<< core.test

^ = () -> Num => <
  line = @readStdin()     ~ launches the read; returns a deferred Text (no wait here)
  assertEq(line, "")      ~ the comparison FORCES it; "" at end-of-input (no piped input)
  0
>
~ pipe a line to see a real value flow:  echo hello | quilon run examples/readStdin.ql
```

Binding `line` does not wait; the force is the `==` inside `assertEq`. Because
`print`/`eprint` force and write eagerly, per-fiber output stays in program order.

`core.net` — **`@tcpRequest(address :: Text, requestBytes :: Text) -> Text`** is a one-shot
request exchange: connect to `address` (`host:port`), write the request bytes, read the
response until the peer closes (close-delimited), and hand back all of it as a deferred
`Text`, forced on use like `@readStdin`. The HTTP client sits on it — framing and parsing
happen in ordinary Quilon on the forced bytes.

### Where it is headed

A networked value-returning primitive makes independent launches overlap, which is the reason
implicit futures matter:

```quilon
~ `@get` is a leaf IO primitive (stdlib/runtime) — the ONLY marked thing here.
~ `fetchJson` is ordinary, unmarked user code, yet concurrency-capable for free:
fetchJson = (url :: Text) -> Text => @get(url)   ~ launches IO, returns a deferred Text

loadDashboard = (user :: Text) -> Text => <
  profile = fetchJson("/users/" + user)     ~ launches the first fetch, returns immediately
  orders  = fetchJson("/orders/" + user)    ~ launches the second fetch — overlaps the first
  render(profile, orders)                    ~ each forced at a strict op inside render (block joins)
>
```

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

Add `--debug` (or `-g`) to emit **DWARF debug info** for source-level debugging — a
debugger (`gdb`/`lldb`) can then set breakpoints, step, show backtraces in terms of
`.ql` lines, and **inspect local variables with their Quilon types**:
```bash
quilon build program.ql --debug -o program
llvm-dwarfdump --debug-line program        # lists the .ql file + its line table
llvm-dwarfdump --debug-info program        # shows variables + their debug types
gdb ./program                              # break/step by .ql line, print locals
```
Debug info is opt-in: without `--debug` the binary carries none. It covers line tables,
per-function scopes, and **locals, parameters, and debug types** — every `=`/`:=` local and
parameter is emitted with its type, and nested `{ }` blocks and closures get their own
lexical scopes. Each Quilon type gets a distinct DWARF entry: `Num`/`Bool` as base types,
and `Text`, arrays (`[]T`), records, and sum types as distinctly-named composites, so a
debugger tells them apart despite their shared `{ptr, i64}`-ish machine shape. Only the
program's own source is attributed — functions from imported modules (`<<`) carry no line
info yet, since emission builds one `DIFile` from the root source. Multi-file line info is a
follow-up.

(During development, prefix any command with `cargo run --`, e.g. `cargo run -- run program.ql`.)

### Error messages

Every located failure — a compile error, a failing assertion, a fail-loud runtime check —
prints the same frame: a `path:line:col:` position line, the message on the line under it,
then the offending source line and a caret (`^`) underline beneath the exact span. Line and
column are **1-based** and count characters, not bytes. A path too long for the position line
is shown from its END behind a `…`, so the file name stays visible. For example, the program

```
add = (a :: Num) -> Num => a + true
```

reports (since `+` is an [overload set](#overloading), a `Num + Bool` matches no member):

```
program.ql:1:28:
error: No overload of '+' matches argument types (Num, Bool). Candidates: (Num, Num), (Text, Text)
  |
1 | add = (a :: Num) -> Num => a + true
  |                            ^^^^^^^^
```

A multi-line span underlines its first line. A failure with no source location (a missing
file, an unresolved import) prints a plain one-line message. Any compile error exits 1.

Runtime failures use the same frame at the expression responsible: a failing
[`core.test` assertion](corelib/test.md) at the assertion's call site, a fail-loud check (a
bad `arr[i]`, a violated `Text.replace`/`repeat` contract) at the call that broke the
contract. Those are colored when stderr is a terminal, plain when redirected or under
`NO_COLOR`/`TERM=dumb`; compile errors are not colored yet. Because a runtime report carries
the source line it names, that line's text is embedded in the built binary, with no way to
strip it yet.

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
| `Text` methods: `split`/`trim`/`trimStart`/`trimEnd`/`replaceAll`/`replace`/`repeat`/`contains`/`indexOf`/`slice`/`toUpper`/`toLower` (chainable; grapheme-based) | ✅ |
| Ad-hoc overloading: same-named typed defs, exact-type dispatch | ✅ |
| Operator overloading (`+`, comparisons, … on user types); built-ins as overloads | ✅ |
| `Bool` | ✅ |
| `Unit` type / value (`$`) | ✅ |
| Arrays: literals, `.size`, `[index]` | ✅ |
| Array methods: `map`/`filter`/`reduce`/`each`/`find`/`at` (chainable; lambda args inlined) | ✅ |
| Array `+`: concat `[]T + []T`, append `[]T + T`, prepend `T + []T` → new `[]T` (non-mutating) | ✅ |
| Maps `[\|K => V\|]`: literals, `.size`, `get` (safe, `Result`; no bracket indexing)/`has`/`set`/`keys`/`values`/`each`; keys Num/Text/Bool; immutable | ✅ |
| Sets `[\|T\|]`: literals, `.size`, `has`/`add`/`items`/`each`, algebra `+`/`-`/`+-` (union/difference/intersection); immutable | ✅ |
| Map/Set removal, and user-defined key types (via a `%` hash hook) | ❌ |
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
| Sum-type payload is a named **record** (`Method = Get / Post(Body)`; match binds it, reads its fields / calls its methods) | ✅ |
| Concrete `Result` payloads: a bound `Ok`/`NotOk` payload is usable at its real type (overload dispatch, across `-> Result` fn boundaries) | ✅ |
| Uniform `Result` layout: a `Result` of ANY payload (`Num`/`Text`/`[]Text`/composite) passes through a generic `(r :: Result)` param/return — powers `assertOk`/`assertNotOk` on `getEnv`/`getOpt` | ✅ |
| Modules: `<< core.io`, `<< core.test`, `<< core.cli`, `<< core.time`, `<< core.net`, file-path imports, `>>` exports | ✅ |
| I/O: `print` / `eprint` / `write` | ✅ |
| I/O: `@readStdin` — deferred stdin line read, forced on use | ✅ |
| Assertions: `<< core.test` (`assert` (+ `AssertOpts` message) / `assertEq` / `assertNotEq` / `assertOk` / `assertNotOk` / `failAt`; fail → exit 101) | ✅ |
| [Call-site locations](#call-site-locations--site): a trailing `site :: Site` parameter filled in by the compiler and forwarded by passing it on (track-caller) — a failing assertion reports YOUR call's `file:line:column` with a caret, identically under JIT and native | ✅ |
| Terminal-aware color: a failing assertion's report is colored on a terminal and plain when redirected or under `NO_COLOR`/`TERM=dumb`; the `\e` (ESC) string escape writes an ANSI sequence from `.ql` | ✅ |
| CLI helpers: `<< core.cli` (`getEnv` / `hasFlag` / `getOpt`; both `--name value` and `--name=value`; flag names with or without `--`) | ✅ |
| Conservative GC (Boehm) | ✅ |
| `Text` (and nested arrays) in records/arrays, or as a sum-type payload (`Ok(text)`) | ✅ |
| `^` receives `args :: []Text` (argv) and `env :: [][]Text` (environment pairs) | ✅ |
| Lambdas (`x => …`) as array-method arguments (inlined per element) | ✅ |
| Generics / type variables (overloading is the only polymorphism) | ❌ |
| Overloaded name passed as a value, or a closure as a param / return (higher-order) | ❌ |
| Generic / polymorphic-capturing closures | ❌ |
| String interpolation | ❌ |
| [Colorless implicit-futures concurrency](#concurrency--colorless-implicit-futures--in-progress) — `@` leaf IO primitives, deferred values, force-at-strict-op: the fiber scheduler, the `@sleep` pause, and the value-returning `@readStdin` (deferred `Text`, forced on use) run today; cross-source overlap (networked `@get`) and the multicore runtime are still to come | 🚧 |

---

## Known limitations

0.9 is a stable **core**, not the whole language. Notably:

- **No generics.** Overloading (ad-hoc, exact-type dispatch) is the only polymorphism; there are no type variables. The module system is minimal (`core.io`/`core.test` built-ins + file-path imports).
- **Closures are monomorphic.** Lexical capture works end-to-end (`=` by value / `:=` by reference; see [Closures](#closures--capture-by--value-vs--reference)), including recursion of non-capturing nested functions, capture across nesting levels, and capturing-then-calling another closure. Deferred (each needs the closure's type threaded through inference): capturing a *polymorphic* value, *generic* closures, passing a closure **as a parameter**, and **returning one from a function**. An unsupported position is rejected at compile time (a called unannotated parameter reports `Not a function`), never miscompiled.
- **Overloads (and closures) resolve at direct call sites only.** Passing an overloaded name as a value (higher-order use) is not yet supported.
- **Sum-type payloads mixing types across variants aren't unified yet.** Payload slots have a fixed representation sized to the widest variant. Distinct payload *types* per slot across variants (a position that is `Num` in one variant and `Text` in another) is deferred; the payload set (`Num`/`Text`/`Bool`/`$` and a named record, consistent per position) works.
- **A named-composite sum payload must be a record, and a record field cannot yet be a named composite.** A variant may carry a named **record** (`Post(Body)`), but not another named **sum**; and a record field is still limited to built-in types and arrays (a `{ inner :: Inner }` field of a user type is a deferred follow-up).
- **Concurrency is partly built.** The [model](#concurrency--colorless-implicit-futures--in-progress) is locked; the fiber scheduler, reactor, `@sleep`, and the deferred-value primitives (`@readStdin`, `@tcpRequest`) run. Remaining for 1.0: overlap as a showcase, deferred composites, further `@` primitives (file), and multicore M:N.

---

## Compiler architecture

A classic multi-pass pipeline (each stage a module under `src/`); `src/driver.rs` runs the shared front-end (read → lex → parse → resolve imports → typecheck) for all CLI commands and renders any failure through `src/diagnostic.rs` (the rustc-style `path:line:col` reporter described under [Error messages](#error-messages)).

1. **Lexer** — `src/lexer/` (`logos`), `Lexer::tokenize(&str)`.
2. **Parser** — `src/parser/ast_parser.rs`, hand-written recursive descent, `parse(&tokens)`.
3. **AST** — `src/ast/nodes.rs`.
4. **Type checker** — `src/typechecker/checker.rs` plus its per-area child modules.
5. **Code generator** — `src/codegen/generator.rs` plus its per-area child modules (`inkwell`, LLVM 22) → LLVM IR.
6. **Runtime intrinsics** — `src/runtime/` (`__write_bytes`, grapheme counting, GC glue), packaged as `libquilon_rt`.
7. **LLVM** — `quilon build` emits an object in-process and links `libquilon_rt` + `libgc` into a native binary; `quilon run` uses an in-process JIT.

See `CLAUDE.md` for contributor guidance.
