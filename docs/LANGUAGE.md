# Quilon Language Reference

**Version:** 0.9.2 — "Hegemon" (stable basics — the core is solid and verified end-to-end, but the language is **not** yet feature-complete; see [Known limitations](#known-limitations)).

Quilon is a statically-typed, **symbol-based** language (no control-flow keywords) that compiles to native code via LLVM. Every example below has a passing end-to-end test: each `examples/*.qn` program is **self-asserting** — it verifies its own results in-language with `assert(value, matcher)` and exits 0 (a failing assertion aborts with exit 101), under both the JIT (`quilon run`) and native AOT.

---

## Design principles

Quilon's identity, and the rules that guide its design:

- **No keywords.** Every construct is punctuation, not words — *nothing was removed from the language; the words were.* Branching is `?` / `|`, the entry point is `^`, import/export are `<<` / `>>`, mutability is `:=`, sum-type alternatives are `/`. Not one word is reserved: `if`, `while`, `for` and the rest are ordinary identifiers you may bind.
- **Symbols mirror notation that already exists.** A symbol reuses a notation the world already has rather than inventing one: `/` separates sum-type alternatives the way you already write "red / green / blue".
- **The playful choice wins.** On a genuine toss-up, the more delightful option is picked — `^` for the entry point, `$` for Unit. Syntax is allowed a sense of humor.
- **Deliberate simplicity.** The smallest system that works: no generics (ad-hoc overloading is the only polymorphism), no `while`, no interfaces, a single `Num` type. Features are omitted on purpose.
- **Fail loud, never silent.** Invalid inputs and meaningless operations must *fail* — never silently no-op, clamp, or return a magic sentinel. A statically-determinable problem is a **compile error**; anything else is a runtime error on stderr with a non-zero exit, saying [where it happened](#error-messages). (Hence `Text.indexOf → Ok(Num)/NotOk` rather than a `-1` sentinel, and `Text.replace`'s count/empty-argument checks failing rather than clamping.)
- **No magic.** No hidden coercions, no implicit dispatch. Overloads are exact-typed; operators mean what they say.
- **Immutable by default.** `=` binds immutably, `:=` binds mutably — for variables, for record bindings, and for methods: a method declared `name := …` may mutate its receiver, and one declared `name = …` is checked to make sure it does not.
- **Errors are values.** Fallible operations return `Ok` / `NotOk` (a normal sum type) — no exceptions, no sentinels.
- **Library APIs hide internals.** A library never makes the caller do its own conversion/desugaring (`print(x)`, never `print(show(x))`).

---

## Symbols

| Symbol | Meaning | Example |
|--------|---------|---------|
| `=` | Immutable binding | `x = 42` |
| `:=` | Mutable bind / reassign / in-place field write | `counter := 0`, `obj.field := v` |
| `::` | Type annotation | `x :: Num` |
| `=>` | Function body / match arm | `f = (x :: Num) => x + 1` |
| `->` | Return type; also a [function type](#function-types--higher-order-functions) | `f = (x :: Num) -> Num => x` · `(Num) -> Bool` |
| `+` `-` `*` `/` `%` | [Arithmetic](#expressions) (`-x` negates) | `a + b` · `x % 2` |
| `==` `!=` `<` `<=` `>` `>=` | [Comparison](#expressions) → `Bool` · `==`/`!=` over `Num`/`Text`/`Bool`, ordering over `Num`/`Text` | `a == b` · `x <= 3` |
| `&&` `\|\|` `!` | Logical and / or / not (short-circuit) | `a && !b` |
| `< >` | Block delimiters · also `<`/`>` comparison ([rule](#expressions)) | `< a b a + b >` · `a < b` · `a > b` |
| `^` | Entry point (main) | `^ = () -> Num => 0` |
| `$` | Unit type **and** its sole value | `f = () -> $ => $` |
| `<<` | Import a module | `<< core.io` |
| `>>` | Export an item from a module | `>> add = (a :: Num, b :: Num) => a + b` |
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

(See `examples/text.qn` and `examples/text_methods.qn`.)

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
user type may **override** its rendering by defining its own `` ` `` operator — a member of
the [record](#named-record-types-with-methods) or [sum](#methods--the-optional---block),
with `it` bound to the value, returning `Text`, and free to use interpolation itself:

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

There are **no format specifiers** (width/precision/etc.). (See `examples/interpolation.qn`.)

**On output, `print` renders and `write` does not.** `print`/`eprint` write text for a
reader: a `Text` whose bytes are not valid UTF-8 arrives with each invalid byte shown as the
replacement character `�`. [`write`](corelib/io.md) is the byte-exact form — a `Text`'s bytes
as they are. Both write the whole `Text`: a NUL byte is content, never a terminator.

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
Arrays are `{ ptr, size }` internally. (See `examples/arrays.qn`.)

Indexing is **checked** (fail loud, never silent): an out-of-bounds, negative, or NaN index
is a runtime error naming the read that failed ([shape](#error-messages)), exit status 1 —
never a raw memory read. A **fractional** in-range index truncates toward zero (`nums[1.7]`
reads `nums[1]`); with one unified `Num`, index arithmetic like `size / 2` legitimately
produces fractions. Use [`at(n)`](#array-methods) for the non-aborting `Ok`/`NotOk` form when
an index might be out of range — see the computed-index case at the end of
`examples/array_methods.qn`.

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
(e.g. `[]Text`), not just `[]Num`. (See `examples/array_methods.qn`.)

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
(See `examples/array_concat.qn`.)

### Maps

A `Map` is a **built-in parametric collection** — like `[]T`, not a user-defined generic —
written with a **pipe fence** `[|K => V|]` (`=>` reads "maps to"). It is immutable, keyed by
`Num`/`Text`/`Bool` or a **user type** that defines both a `%` hash hook and an `==` member,
and read through `.get` (which returns a `Result` — there is no bracket indexing on a map).
Full reference: [`docs/collections/map.md`](collections/map.md) (and `examples/maps.qn`).

### Sets

A `Set` is a **built-in parametric collection** — like `[]T`, not a user-defined generic —
written with the same **pipe fence** `[|T|]` (which keeps a set literal distinct from an array).
It is immutable, holds unique `Num`/`Text`/`Bool` elements (or a **user type** defining both
a `%` hash hook and an `==` member), and supports set algebra
(`+` union, `-` difference, `+-`/`-+` intersection). Full reference:
[`docs/collections/set.md`](collections/set.md) (and `examples/sets.qn`).

### Records
Anonymous structs with named fields:
```quilon
user = { name = "Alice", age = 30 }
n    = user.name
```
Fields may hold any type — `Text`, arrays, nested arrays, etc. — and read back at
their real type (no numeric-only restriction). (See `examples/records.qn` and
`examples/composites.qn`, which exercises a `Text` record field, an array of `Text`,
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
(See `examples/methods.qn`.)

A method declared with `:=` instead of `=` is a **setter** — it may mutate its
receiver — and calling one requires a mutable (`:=`) receiver
(see [Mutation](#mutation-in-place-field-writes--setters)).

An unannotated method parameter defaults to `Num` (as in any [ordinary
definition](#overloading--ad-hoc-and-explicit)), and call sites are held to that default:
`t.add("hi")` on `add = (x) => it.v + x` is a type error, not a runtime surprise.

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
  `examples/nested_composites.qn`.)
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
(or a lowercase binding) wildcard. (See `examples/sum_types.qn`.)

#### Methods — the optional `{ }` block
A sum type may carry a trailing `{ }` block of **methods** (the block is optional — a sum
with no methods is written exactly as above). `it` is the whole sum value, so a method
typically matches on it. A member is a named method, an
[operator](#operator-overloading), or the render `` ` ``. The block holds **methods only**
— a sum has no fields, so a field-like entry there is a compile error, and its methods are
always `=` (see [Mutation](#mutation-in-place-field-writes--setters)).
```quilon
Shape = Circle(Num) / Rect(Num, Num) {
  area = () -> Num => it ? | Circle(r) => 3 * r * r | Rect(w, h) => w * h
  == = (other :: Shape) -> Bool => it.area() == other.area()      ~ operator member
  ` = () -> Text => it ? | Circle(r) => "Circle(`r`)" | Rect(w, h) => "Rect(`w`x`h`)"
}
Rect(6, 7).area()                ~ 42
```
(See `examples/sum_methods.qn`.)

#### `Result` is a normal sum type
`Result` is just a predefined sum type — there is no special case:
```quilon
Result = Ok(...) / NotOk(...)    ~ predefined; `Ok` = success, `NotOk` = failure
```
Use it exactly like any other sum type:
```quilon
classify = (v :: Result) => v ?
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
`getEnv`/`getOpt` shape — carries **both** arms' payloads. (See `examples/result.qn` and
`examples/result_payload.qn`.)

Every `Result` shares **one uniform layout** regardless of its payload, so a `Result`
carrying *any* payload — `Num`, `Text`, `[]Text`, a composite — passes through a generic
`(r :: Result)` parameter or return. This is what lets the `isOk()` / `isNotOk()`
[matchers](corelib/test.md#the-matchers) read a `Result` of any shape, including the
composite-payload results of `getEnv` / `getOpt` (see `examples/cli.qn`). Extracting a
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
scale = (n :: Num) => n * 3   ~ fine — a function value
counter := 4            ~ fine — and writable from a function

doubled = limit * 2     ~ error: has to be computed
greeting = "hi"         ~ error: Text is a { pointer, length } pair, built at runtime
sizes = [1, 2]          ~ error: an array is built at runtime
origin = { x = 0 }      ~ error: so is a record
```

A rejected binding reports what it is and how to fix it — move the work into the function
that uses it. Anything computed is perfectly ordinary *inside* a function; the restriction
is only about globals. (See `examples/globals.qn` and `examples/global_computed.qn`.)

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

A method is a **setter** when it is **declared** with `:=`, and the binding operator is
the marker exactly as it is for a variable — a method's right to mutate is part of its
signature.

```quilon
Counter = {
  value :: Num,
  bump := (by :: Num) => it.value := it.value + by  ~ may mutate `it`
  peek = => it.value                                ~ promises not to
}

c := Counter { value = 30 }   ~ `:=` -> mutable
c.bump(5)                      ~ setter mutates in place -> value = 35
c.value := c.value + 7         ~ direct field write    -> value = 42
```

An `=` method is **held to its promise**: writing `it.field := …` in one, or calling a
`:=` sibling on `it`, is a compile error telling you to declare it `:=`. The write counts
**wherever it appears in the body** — nested inside a lambda, an array or record literal,
a match arm, an argument list, or a function declared inside the body. Nesting does not
launder a mutation:

```quilon
~ error: Method 'Counter.bumpAll' mutates 'it' but is declared with '='
bumpAll = (steps :: []Num) => steps.each(s => it.value := s)
```

A setter call requires a `:=` receiver:

```quilon
c = Counter { value = 30 }   ~ `=` -> immutable
c.value := 99                 ~ error: cannot write a field of immutable `c`
c.bump(5)                     ~ error: cannot call mutating method `bump` on immutable `c`
```

**Setters live on records.** Only a record's named methods may be declared `:=`. A sum's
methods, and operator members on either kind (`` ` ``, `==`, `+`, …), are always `=` and
non-mutating; `:=` on one is a compile error. Nothing they can do mutates the receiver
anyway: a sum keeps its data in variant payloads, whose match bindings are immutable, and
an operator or render member yields a value.

```quilon
Shape = Circle(Num) / Rect(Num, Num) {
  area := () -> Num => 0      ~ error: a sum cannot have a mutating method
}

Counter = {
  value :: Num,
  + := (other :: Counter) -> Num => it.value   ~ error: an operator member is never `:=`
}
```

(See `examples/mutation.qn`.)

---

## Functions

```quilon
greet  = => "Hello!"                       ~ no params
double = (x :: Num) => x * 2               ~ one param
add    = (a :: Num, b :: Num) => a + b     ~ multiple params
typed  = (a :: Num, b :: Num) -> Num => a + b
```
Every function and method parameter must be annotated — there is no default type; an
unannotated parameter is a compile error that names it. The one exception is a lambda
passed to a built-in collection method (`.map` / `.filter` / `.reduce` / `.each`), whose
parameter type is taken from the element type of the receiver.
Multi-statement bodies use `< >` blocks (the last expression is the value):
```quilon
compute = (x :: Num) => <
  doubled = x * 2
  doubled * doubled
>
```
Functions may recurse; a recursive function needs a `-> Type` annotation:
```quilon
factorial = (n :: Num) -> Num => n == 0 ? 1 : n * factorial(n - 1)
```
(See `examples/factorial.qn`, `examples/fibonacci.qn`.)

### Function types & higher-order functions

A **function type** is written with the arrow, reusing `->`. The parameter types go in
parentheses; `$` (Unit) names a function that returns nothing:

```quilon
() -> $              ~ takes nothing, returns unit
(Num) -> Bool        ~ one parameter
(Num, Text) -> Bool  ~ two parameters
```

A function type may be a **parameter type**, which is what makes a function *higher-order*
— it takes another function as an argument and calls it:

```quilon
apply = (f :: (Num) -> Num, x :: Num) -> Num => f(x)
twice = (f :: (Num) -> Num, x :: Num) -> Num => f(f(x))

^ = () -> Num => twice((n :: Num) => n * 2, 3)   ~ ((3*2)*2) = 12
```

The value passed in is a closure — a lambda literal (as above) or a named closure passed
by its name. Function types may nest as parameter types (`((Num) -> Bool, Num) -> Bool`).
A function-typed **return** (currying, `(A) -> (B) -> C`) is not supported yet. (See
`examples/higher_order.qn`.)

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
(See `examples/tail_recursion.qn`, which recurses 1,000,000 deep.)

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
  bump = (n :: Num) => <
    total := total + n       ~ writes the SHARED cell; the effect persists across calls
    total
  >
  bump(10)                   ~ total -> 10
  bump(20)                   ~ total -> 30  (same cell)

  base = 7                   ~ `=`  -> captured BY VALUE (a frozen copy)
  addBase = (x :: Num) => x + base

  total + addBase(5)         ~ 30 + 12 = 42
>
```

A non-capturing nested function may **recurse** (`fact = (n :: Num) => … fact(n-1) …`);
nested closures may capture from any enclosing frame (the shared `:=` cell is threaded
through every level), and a closure value may itself be captured by another closure and
called. A closure may also be **passed to a function** whose parameter has the matching
[function type](#function-types--higher-order-functions) and called there.

Closures are **monomorphic**: parameters and captured values are concrete-typed. Capturing a
polymorphic value, generic closures, and **returning** a closure across frames are deferred
— see [Known limitations](#known-limitations). (See `examples/closures.qn`.)

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
- **The compiler's own definitions are members, not reserved names.** The built-in
  operators, and the corelib functions the compiler provides (`print`/`eprint`, `write`,
  `now`), are members of their sets like any other. Defining one of those names with a
  different signature ADDS a member that wins for its argument types, and the built-in
  stays reachable for the types it claims; defining the built-in's own signature is the
  usual duplicate-definition error:
  ```quilon
  write = (content :: Text) -> Num => write(content, stdout)  ~ adds a member…
  write("raw")           ~ …which this call picks
  write("raw", stdout)   ~ while this one still reaches the built-in
  ```
- A member joins its set where it is written, so a call resolves only against the members
  above it ([names resolve top to bottom](#names-resolve-top-to-bottom)).
- Dispatch is resolved at **direct call sites** by static argument types. Passing an
  overloaded name as a value (higher-order use) is not yet supported.

### Operator overloading

An operator is user-overloadable — `+ - * / %`, `== != < <= > >=` — as a **member of the
type it operates on** (a [record](#named-record-types-with-methods) or a
[sum](#sum-types--)). `it` is the **left** operand; a **binary** operator member takes one
explicit parameter (the **right** operand), a unary one (the render `` ` ``) takes none.
An operator member is always `=`-declared and yields a value; it never mutates `it`
(see [Mutation](#mutation-in-place-field-writes--setters)):

```quilon
Vec = {
  x :: Num, y :: Num,
  + = (other :: Vec) -> Vec => Vec { x = it.x + other.x, y = it.y + other.y }
  == = (other :: Vec) -> Bool => it.x == other.x && it.y == other.y
}

v = Vec { x = 1, y = 2 } + Vec { x = 3, y = 4 }   ~ resolves to Vec's `+`
```

`a <op> b` resolves the operator from the **left operand's** type; the right operand need
not be the same type (`Vec * Num -> Vec`). Resolution is exact-typed like any overload, and
lowers to a direct call. The built-in operators (`Num`/`Text` `+`, `==` over any scalar,
`<`/`>`/`<=`/`>=` over `Num`/`Text`) are members of the same sets, so `"abc" < "abd"` works
out of the box. (`<`/`>` are not definable as members — a `<`/`>` at member-name position
would read as a block; use `<=`/`>=`.)

A **comparison/equality** member (`== != < <= > >=`) **must return `Bool`**; **arithmetic**
members (`+ - * / %`) return whatever they declare. A **top-level** operator definition is
rejected — the operator must be a member of its type.

**The `%` hash hook.** A **unary** `% = () -> Num => …` member (`it` the value, no explicit
parameter) is the type's **hash**, letting it be a [Map/Set key](#maps) alongside its `==`
member. Both are required together, and `%`/`==` must agree (equal values hash the same).
This unary `%` is distinct from the binary `%` remainder operator (which takes one
parameter), and has no call syntax of its own — the collections invoke it.

(See `examples/overloading.qn`, `examples/sum_methods.qn`, `examples/maps.qn`,
`examples/sets.qn`, and `examples/overload_dispatch.qn` for dispatch on argument types out
of an array element, a match, a call, or a lambda.)

---

## Expressions

- **Arithmetic:** `+ - * / %` (and `-x`). `+` is an [overload set](#overloading): `Num + Num` adds, `Text + Text` concatenates, and on arrays it concatenates / appends / prepends (`[]T + []T`, `[]T + T`, `T + []T`, all yielding a new `[]T` — see [Array concatenation](#array-concatenation--)). `%` is the f64 remainder and works on fractional operands too (`7.5 % 2` → `1.5`); the result takes the **dividend's** sign (`-7 % 3` → `-1`, `7 % -3` → `1`), like C `fmod` / Rust `%`.
- **Comparison:** `== != < <= > >=`; all return `Bool`. Equality (`==`/`!=`) is over `Num`, `Text` and `Bool`; ordering (`< <= > >=`) is over `Num` and (lexicographically) `Text`. Each is a [user-overloadable operator](#operator-overloading), and comparing two different types is a no-matching-overload error — there is no coercion.
- **Logical:** `&& || !` (short-circuit).

> **`<` and `>` vs. `< >` blocks.** `<` and `>` double as the block delimiters. A `<`
> after a complete operand is always less-than (a block can't start mid-expression). A `>`
> **closes a block by default**; it is **greater-than only when an operand follows it on
> the same line** — an identifier, a literal, `(`, `[`, `{`, or a prefix `-`/`!`. So `a > b`,
> `f(x > y)`, `a > -b` and `"b" > "a"` are comparisons, while a `>` before a `)`, `]`, `}`,
> `,`, a `~` comment, or the end of the line closes its block — which is what lets a
> block-bodied lambda sit inside a call on one line:
> ```quilon
> xs.each(x => <
>   total := total + x
> >)
> ```
> Two rules follow: don't end a line with a comparison `>` (the right operand must be on
> that line), and separate two adjacent closers with a space — `> >`, since `>>` is the
> export marker. `<=`/`>=`/`>>` are distinct tokens and unaffected.

> **Statement boundaries — line-first `(` / `[` / `{`.** Quilon has no statement separator,
> and the grammar is newline-insensitive but for two rules: the `>` rule above, and
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
> (See `examples/statements.qn`.)
- **Ternary:** `cond ? then : else`.
- **Blocks:** `< stmt… last >` evaluate to their last expression. A block goes in **body**
  position — a function's, a lambda's, or a method's — not in operand position, so a block
  is never the left or right side of an operator:
```quilon
total = () -> Num => <
  x = 10
  y = 20
  x + y          ~ total() is 30
>
```

### Operator precedence
Least-priority level first; every level is **left-associative** except `<-`, which is
non-associative (`1 <- 2 <- 3` is a parse error).

| | Operators |
|---|---|
| less priority | `:=` (reassignment) |
| | `? :` ternary · `?` `\|` match |
| | `\|\|` |
| | `&&` |
| | `==` `!=` |
| | `<` `<=` `>` `>=` |
| | `<-` (range) |
| | `\|>` (pipe) |
| | `+` `-` |
| | `*` `/` `%` `+-` |
| | `-x` `!x` (prefix) |
| more priority | `.field` · `.method(…)` · `f(…)` · `xs[i]` |

So `2 + 3 |> double` is `double(5)`, `1 <- 2 + 2` is `1 <- 4`, and `1 < 2 == true` is
`(1 < 2) == true`. Parenthesize anything else. `>` appears in the table in its operator
reading; whether a given `>` gets that reading at all is settled first, in the lexer — see
the [`>` rule](#expressions).

### Pipe — `|>`
`|>` feeds its left operand in as the **first argument** of the right-hand call:
```quilon
x |> f          ~ ≡ f(x)
x |> f(a)       ~ ≡ f(x, a)
10 |> double |> addFive   ~ ≡ addFive(double(10))
```
(See `examples/pipeline.qn`.)

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
so even deep recursion runs in constant stack. (See `examples/iteration.qn`.)

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
direction (ascending vs descending) is decided at runtime. (See `examples/ranges.qn`.)

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

(See `examples/spread.qn`.)

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
The type checker verifies matches are exhaustive (use `_` to cover the rest). (See `examples/pattern_match.qn`.)

---

## Modules

```quilon
<< core.io                 ~ import the built-in IO module
<< "lib/math.qn"           ~ import a user module by path (/ or \)

>> add = (a :: Num, b :: Num) => a + b   ~ `>>` exports an item; unmarked items are file-private
```
- The built-in modules are `core.io`, `core.test`, `core.cli`, `core.time`, and `core.net`; their members are real functions. See the [corelib](#corelib) index for each module's API reference.
- `Text` and the operators are built-ins and need **no** import.
- A module exposes only its `>>`-exported items.

(See `examples/use_module.qn`, which imports `examples/mathlib.qn`.)

---

## Corelib

The corelib — Quilon's standard library — ships with the compiler; import a module with
`<< core.<module>`. Each has its own API reference under [`docs/corelib/`](corelib/):
signatures, behavior, and a small example per function.

| Module | Import | What it gives you |
|--------|--------|-------------------|
| [`core.io`](corelib/io.md) | `<< core.io` | Output to file descriptors and stdin: `print` / `eprint` / `write`, the `stdout` / `stderr` descriptors, and the deferred `@readStdin` line read. |
| [`core.test`](corelib/test.md) | `<< core.test` | The [test harness](corelib/test.md#the-test-harness) `quilon test` runs — `describe` / `it` and the reporter — plus `failAt`, for a check of your own. The assertions themselves need no import: `assert` / `expect` and their matchers are compiler-provided. |
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
  print(whereAmI())        ~ prints e.g. demo.qn:4:9 — the location of THIS call
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
its `Text` fields.) [`failAt`](corelib/test.md#building-a-check-of-your-own) is built on this; nothing
about it is specific to them. See `examples/call_site.qn`.

---

## Entry point

Every executable defines `^` (main); the compiler generates a C-compatible `main()` wrapper (and initializes the GC).
```quilon
^ = () -> Num => 42                              ~ no args/env
^ = (args :: []Text) -> Num => args.size         ~ command-line arguments
^ = (args :: []Text, env :: [|Text => Text|]) -> Num => env.get("HOME")   ~ args + environment
```
**Arguments & environment.** `^` may declare, in order, two typed parameters that the
generated `main()` wrapper fills from the C `argc`/`argv`/`envp`:
- `args :: []Text` — the command-line arguments (argv), **including** `argv[0]` (the
  program name), so `args.size` is always at least 1, and `args[i]` is the *i*-th
  argument as a `Text`.
- `env :: [|Text => Text|]` — the environment, as a Map from each variable's name to its
  value. An entry `KEY=val` is split on its **first** `=` (so `KEY=a=b` maps `KEY` to
  `a=b`); an entry with no `=` maps the whole string to `""`. Read a variable with
  `env.get("HOME")` (or `<< core.cli`'s `getEnv`), both giving `Ok(value)`/`NotOk`.

`args` is a real Quilon array (`.size`, `[index]`, the array methods) and `env` a real Map
(`.get`/`.has`/`.keys`/`.size`); a value read out of either is a full `Text`: the whole
`Text` API, and [overload](#overloading) dispatch by its concrete type.
`quilon run <file> [args...]` and a native build agree on `args`: under `run`, the
program sees `argv = [<file>, <args...>]` (the `quilon`/`run` CLI prefix is stripped and
the `.qn` path becomes `argv[0]`), so `quilon run f.qn a b c` gives the same `args.size`
and trailing arguments as a native `./f a b c` — `argv[0]` is the `.qn` path rather than
the compiled binary's path, but everything the program indexes past it matches. (The
legacy `^ = (argc :: Num, argv :: Num)` form, where `argv` was a placeholder `0`, still
compiles for backward compatibility but is superseded by `args :: []Text`.) Any other
`^` signature (e.g. a non-`Text` array element, or an unexpected parameter) is a
compile-time error, reported by `check` as well as `run`/`build`.

**Exit code:** if `^`'s body evaluates to a `Num`, that value is the exit code. If the body is **not** a `Num` (e.g. a side-effecting block), the program exits **0** — so an effect-only `main` needs no trailing `0`. (This implicit-0 applies only to `^`; ordinary functions always return their last expression's value.)

(See `examples/hello_world.qn` and `examples/args.qn`.)

---

## Memory

Quilon uses a **conservative garbage collector** (Boehm). Heap values (`Text`, etc.) are GC-managed — there is no manual free. The collector is **linked statically** into every binary, so a compiled program carries its own GC and needs nothing installed to run.

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

**`@` marks leaf IO primitives only** — the corelib/runtime primitives that actually do IO
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
reading the clock is instant and never parks. (See `examples/sleep.qn`.)

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
`examples/readStdin.qn`.)

```quilon
<< core.io

^ = () -> Num => <
  line = @readStdin()          ~ launches the read; returns a deferred Text (no wait here)
  assert(line, equals(""))     ~ the comparison FORCES it; "" at end-of-input (no piped input)
  0
>
~ pipe a line to see a real value flow:  echo hello | quilon run examples/readStdin.qn
```

Binding `line` does not wait; the force is the `==` behind `equals`. Because
`print`/`eprint` force and write eagerly, per-fiber output stays in program order.

`core.net` — **`@tcpRequest(address :: Text, requestBytes :: Text) -> Result`** is a one-shot
request exchange: connect to `address` (`host:port`), write the request bytes, read the
response until the peer closes (close-delimited), and hand back a deferred `Result` —
`Ok(responseBytes)` on success or `NotOk(errorMessage)` on any network failure — forced on use
like `@readStdin`. A failure is a value to match, never a crash; the response is capped at 16 MiB.
The HTTP client sits on it — framing and parsing happen in ordinary Quilon on the forced bytes.

### Where it is headed

A networked value-returning primitive makes independent launches overlap, which is the reason
implicit futures matter:

```quilon
~ `@get` is a leaf IO primitive (corelib/runtime) — the ONLY marked thing here.
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

Source files are **`.qn`**, and the compiler rejects a source named anything else. (Quilon
used `.ql` until 0.9.1; it is CodeQL's extension, so GitHub attributed Quilon programs to
CodeQL. Rename a `.ql` file to `.qn` — nothing else about it changes.)

```bash
quilon check   program.qn   # front-end only (lex + parse + resolve imports + typecheck)
quilon run     program.qn   # front-end, then JIT-execute in-process (exit code = ^'s result)
quilon build   program.qn   # produce a native executable
quilon compile program.qn   # emit LLVM IR → program.ll (for inspection)
quilon test    [path]       # run the test suites under a file or directory (default: .)
```

`quilon test` is JIT-only, and exits non-zero if any case failed. It runs a file's top-level
`describe` blocks, which every other command erases — so tests may sit in the file they test,
its `^` included, and still cost a release build nothing. See
[`core.test`](corelib/test.md#the-test-harness).

`quilon build` emits an object file in-process and links it (with the Quilon runtime `libquilon_rt`, which carries the GC) into a native executable:
```bash
quilon build program.qn -o program       # default linker: clang
quilon build program.qn --linker gcc      # gcc also supported (CI checks both)
./program; echo "exit: $?"
```

Add `--debug` (or `-g`) to emit **DWARF debug info** for source-level debugging — a
debugger (`gdb`/`lldb`) can then set breakpoints, step, show backtraces in terms of
`.qn` lines, and **inspect local variables with their Quilon types**:
```bash
quilon build program.qn --debug -o program
llvm-dwarfdump --debug-line program        # lists the .qn file + its line table
llvm-dwarfdump --debug-info program        # shows variables + their debug types
gdb ./program                              # break/step by .qn line, print locals
```
Debug info is opt-in: without `--debug` the binary carries none. It covers line tables,
per-function scopes, and **locals, parameters, and debug types** — every `=`/`:=` local and
parameter is emitted with its type, and nested `{ }` blocks and closures get their own
lexical scopes. Each Quilon type gets a distinct DWARF entry: `Num`/`Bool` as base types,
and `Text`, arrays (`[]T`), records, and sum types as distinctly-named composites, so a
debugger tells them apart despite their shared `{ptr, i64}`-ish machine shape. Line info is
multi-file: a function from an imported module (`<<`) — corelib included — is attributed to
its OWN source, so a debugger steps into it. The entry frame reads `^` (the generated C
`main` shim is named for the entry point and marked artificial). The leaf `@` primitives and
the inert built-in placeholders (`print`/`now`/…) lower to intrinsics and emit no subprogram,
so a debugger steps over them.

(During development, prefix any command with `cargo run --`, e.g. `cargo run -- run program.qn`.)

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
program.qn:1:28:
error: No overload of '+' matches argument types (Num, Bool). Candidates: (Num, Num), (Text, Text)
  |
1 | add = (a :: Num) -> Num => a + true
  |                            ^^^^^^^^
```

A multi-line span underlines its first line. A failure with no source location (a missing
file, an unresolved import) prints a plain one-line message. Any compile error exits 1.

Runtime failures use the same frame at the expression responsible: a failing
[assertion](corelib/test.md) at its own call site, a fail-loud check (a
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
| Operator overloading as a type member (`+`, comparisons, … with `it` the left operand); built-ins as overloads | ✅ |
| `Bool` | ✅ |
| `Unit` type / value (`$`) | ✅ |
| Arrays: literals, `.size`, `[index]` | ✅ |
| Array methods: `map`/`filter`/`reduce`/`each`/`find`/`at` (chainable; lambda args inlined) | ✅ |
| Array `+`: concat `[]T + []T`, append `[]T + T`, prepend `T + []T` → new `[]T` (non-mutating) | ✅ |
| Maps `[\|K => V\|]`: literals, `.size`, `get` (safe, `Result`; no bracket indexing)/`has`/`set`/`remove`/`keys`/`values`/`each`; keys Num/Text/Bool or a user type; immutable | ✅ |
| Sets `[\|T\|]`: literals, `.size`, `has`/`add`/`remove`/`items`/`each`, algebra `+`/`-`/`+-` (union/difference/intersection); immutable | ✅ |
| Map/Set user-defined key types (via a `%` hash hook + `==` member) | ✅ |
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
| Sum-type methods: optional trailing `{ }` block (named methods, operators, render `` ` ``; `it` = the value); no fields, no `:=` methods | ✅ |
| `Result` as a normal predefined sum type (`Ok`/`NotOk`) | ✅ |
| Sum-type payloads: `Num` / `Bool` / `Text` | ✅ |
| Sum-type payload is a named **record** (`Method = Get / Post(Body)`; match binds it, reads its fields / calls its methods) | ✅ |
| Concrete `Result` payloads: a bound `Ok`/`NotOk` payload is usable at its real type (overload dispatch, across `-> Result` fn boundaries) | ✅ |
| Uniform `Result` layout: a `Result` of ANY payload (`Num`/`Text`/`[]Text`/composite) passes through a generic `(r :: Result)` param/return — powers `isOk()`/`isNotOk()` on `getEnv`/`getOpt` | ✅ |
| Modules: `<< core.io`, `<< core.test`, `<< core.cli`, `<< core.time`, `<< core.net`, file-path imports, `>>` exports | ✅ |
| I/O: `print` / `eprint` / `write` | ✅ |
| I/O: `@readStdin` — deferred stdin line read, forced on use | ✅ |
| Assertions: compiler-provided `assert(value, matcher)` (fatal) and `expect(value, matcher)` (recorded, test cases only), over `equals` / `contains` / `not` / `isOk` / `isNotOk`; `core.test`'s `failAt` for a check of your own | ✅ |
| Test harness: [`quilon test`](corelib/test.md#the-test-harness) over top-level `describe` / `it` blocks, which may sit in the file they test; the blocks are erased from every other command | ✅ |
| [Call-site locations](#call-site-locations--site): a trailing `site :: Site` parameter filled in by the compiler and forwarded by passing it on (track-caller) — a failing assertion reports YOUR call's `file:line:column` with a caret, identically under JIT and native | ✅ |
| Terminal-aware color: a failing assertion's report is colored on a terminal and plain when redirected or under `NO_COLOR`/`TERM=dumb`; the `\e` (ESC) string escape writes an ANSI sequence from `.qn` | ✅ |
| CLI helpers: `<< core.cli` (`getEnv` / `hasFlag` / `getOpt`; both `--name value` and `--name=value`; flag names with or without `--`) | ✅ |
| Conservative GC (Boehm) | ✅ |
| `Text` (and nested arrays) in records/arrays, or as a sum-type payload (`Ok(text)`) | ✅ |
| `^` receives `args :: []Text` (argv) and `env :: [\|Text => Text\|]` (the environment as a Map) | ✅ |
| Lambdas (`x => …`) as array-method arguments (inlined per element) | ✅ |
| [Function types](#function-types--higher-order-functions) (`(Num) -> Bool`, `() -> $`) + higher-order functions: a function-typed parameter called inside, taking a closure by literal or by name | ✅ |
| Generics / type variables (overloading is the only polymorphism) | ❌ |
| Overloaded or top-level function name passed as a value; a closure **returned** from a function | ❌ |
| Generic / polymorphic-capturing closures | ❌ |
| String interpolation | ❌ |
| [Colorless implicit-futures concurrency](#concurrency--colorless-implicit-futures--in-progress) — `@` leaf IO primitives, deferred values, force-at-strict-op: the fiber scheduler, the `@sleep` pause, and the value-returning `@readStdin` (deferred `Text`, forced on use) run today; cross-source overlap (networked `@get`) and the multicore runtime are still to come | 🚧 |

---

## Known limitations

0.9 is a stable **core**, not the whole language. Notably:

- **No generics.** Overloading (ad-hoc, exact-type dispatch) is the only polymorphism; there are no type variables — which is why the [matchers](corelib/test.md#the-matchers) are compiler-provided rather than written in `.qn`. The module system is minimal (`core.io`/`core.test` built-ins + file-path imports).
- **Closures are monomorphic.** Lexical capture works end-to-end (`=` by value / `:=` by reference; see [Closures](#closures--capture-by--value-vs--reference)), including recursion of non-capturing nested functions, capture across nesting levels, and capturing-then-calling another closure. A closure can also be passed to a [function-typed parameter](#function-types--higher-order-functions) and called there. Deferred (each needs the closure's type threaded through inference): capturing a *polymorphic* value, *generic* closures, and **returning** a closure from a function.
- **Overloaded and top-level function names are not first-class values.** A closure is passed as a *lambda literal* or a *named closure binding*; passing a top-level function or an overloaded name as a value is not yet supported.
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
7. **LLVM** — `quilon build` emits an object in-process and links `libquilon_rt` into a native binary; `quilon run` uses an in-process JIT.

See `CLAUDE.md` for contributor guidance.
