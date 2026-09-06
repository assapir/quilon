---
title: "Text"
sidebar:
  order: 1
---

# `Text`
UTF-8 text. A **built-in** type, like `Num`/`Bool`/arrays, available without an import.

A `Text` is a **sequence of graphemes** (`Text = []Grapheme`): every user-visible index
and length counts grapheme clusters — user-perceived characters — and a single grapheme is
itself a length-1 `Text`. `.at(i)` reads one grapheme, `.graphemes()` yields them all as a
`[]Text`, and `+` builds a `Text` back up. The representation is UTF-8 bytes (`.size` is
the byte length); the grapheme sequence is how the language addresses it.

```quilon
greeting = "héllo" + " 🌍"   ~ + concatenates (GC-allocated)
b = greeting.size            ~ byte length      → 11
c = greeting.length          ~ grapheme count   → 7
```
- `.size` = byte length.
- `.length` = grapheme-cluster count (user-perceived characters, full UTF-8).
- `+` = concatenation.

A literal accepts these escapes: `\n`, `\r`, `\t`, `\"`, `\\`, `\<`, and `\e`. `\<` writes
a literal `<`, which would otherwise open a block. `\e` is the ESC byte that leads an ANSI
terminal sequence (`"\e[1m" + text + "\e[0m"`). Any other escape is a lex error.

## Text order

`Text` is **logical order**: the order in which it was typed and is read, independent of
any script's display direction. Every index, length, search, and concatenation is logical
order — `.length`, `.at`, `.graphemes()`, `.slice`, `.indexOf`, `.split`, and `+` all
address and produce logical-order text. `print` and `write` emit `Text` in logical order;
the display device performs the Unicode Bidirectional Algorithm to render it in visual
order. Every operation preserves the data's order and content; direction and isolate marks
are content like any other grapheme. (See `examples/text.qn`.)

## Text methods

`Text` carries **built-in, compiler-provided methods**, called as `text.method(...)` and
freely chainable. User-visible indices and lengths are **grapheme-based**, matching
`.length`.

| Method | Result | Notes |
|--------|--------|-------|
| `split(separator :: Text)` | `[]Text` | split on `separator`; consecutive separators keep empty pieces (`"a,,b".split(",")` → `["a","","b"]`), an empty haystack yields `[""]`, and an **empty** `separator` splits into individual graphemes (`"abc".split("")` → `["a","b","c"]`) |
| `trim()` | `Text` | strip leading **and** trailing whitespace |
| `trimStart()` / `trimEnd()` | `Text` | strip leading-only / trailing-only whitespace |
| `replaceAll(from :: Text, to :: Text)` | `Text` | replace **every** occurrence of `from` with `to` |
| `replace(from :: Text, to :: Text, count :: Num)` | `Text` | replace **exactly** the first `count` occurrences (left→right); `count` truncates toward zero |
| `contains(sub :: Text)` | `Bool` | whether `sub` occurs in the text |
| `indexOf(sub :: Text)` | `Ok(Num)` / `NotOk` | grapheme index of the first occurrence (`Ok`), or `NotOk` when absent |
| `slice(start :: Num, end :: Num)` | `Text` | substring over grapheme indices `[start, end)`; out-of-range indices **clamp** to bounds, and `end ≤ start` yields `""` |
| `at(index :: Num)` | `Ok(Text)` / `NotOk` | the grapheme at `index` (a length-1 `Text`, multi-codepoint clusters kept whole), `NotOk` out of bounds — mirroring array [`.at`](../collections/arrays.md#array-methods) |
| `graphemes()` | `[]Text` | every grapheme cluster in order, one length-1 `Text` each (`""` → `[]`); composes with the array methods |
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

These methods are **reserved on `Text`**, like the [array methods](../collections/arrays.md#array-methods)
are on arrays: on a `Text` receiver the built-in wins over a same-named user overload on
another type. `split`/`graphemes` yield a plain `[]Text`, which composes with `.size`, `[i]`, the
[array methods](../collections/arrays.md#array-methods), and array `+`. A `[]Text` collapses
to a `Text` with `reduce` + `+`.

The primitives are native: segmentation (`length`/`graphemes`/`at`), `indexOf`,
`slice`, `split`, `replaceAll`, `trimStart`/`trimEnd`, `toUpper`/`toLower`, comparison,
and `+`. The composable methods — `trim`, `contains`, `replace`, `repeat` — are ordinary
Quilon over those (`corelib/text.qn`), merged in by the compiler under its qualified
names, binding nothing in the program's own scope, when a program uses one. That module
is the compiler's own: member syntax is the way its methods are reached, and `<< core.text`
is rejected.

`replace`/`replaceAll`/`repeat` **fail loudly**. Three inputs are rejected: an empty
`from`; a `replace` `count` that is `<= 0` or exceeds the occurrences present; and a
negative or fractional `repeat` count. A literal violation is a compile error
(`"a".replace("a", "b", 0)`, `"aa".replace("a", "b", 5)`). A computed one is a
[located diagnostic](../tooling/errors.md) at run time, with exit `101`. `replaceAll`
replaces every occurrence; `replace(count)` replaces exactly `count`.

(See `examples/text.qn` and `examples/text_methods.qn`.)

## String interpolation and the render operator (`` ` ``)

A string literal may contain **interpolation holes**: expressions wrapped in backticks.
Each hole is rendered to `Text` and spliced in:

```quilon ignore
"hi `user.name`"      ~ splices the rendered value of user.name
"sum: `a + b`"        ~ any expression
"port `getPort()`"    ~ a call
```

A hole can be **any expression**, and its value can be of **any type** — every type is
renderable. To write a **literal backtick**, double it: `` `` `` yields one `` ` `` and
opens no hole. A plain string with no holes is an ordinary `Text` literal.

**One render path.** Interpolation and [`print`/`eprint`/`write`](../corelib/io.md) all
render a value by invoking its `` ` `` (backtick) operator. Every built-in type has a
**default** `` ` ``. Any user type may **override** its rendering by defining its own
`` ` `` operator as a member of
the [record](records.md#named-record-types-with-methods) or [sum](sum-types.md#methods--the-optional---block).
The member binds `it` to the value, returns `Text`, and may use interpolation itself:

```quilon
User = {
  name :: Text,
  age  :: Num,
  ` = () -> Text => < "User(`it.name`, `it.age`)" > ~ override: `it` is the instance
}
~ Now both `io.print(u)` and `"`u`"` render as  User(Ada, 36)
```

`io.print(u)` and `` "`u`" `` take the same path through `u`'s `` ` `` — the override when
present, the built-in default otherwise. A `` ` `` that renders `it` *wholesale* renders
through the default.

**Default rendering** (the built-in `` ` `` per type):

| Type | Renders as | Example |
|------|-----------|---------|
| `Num` | integer-valued → no decimals; else shortest round-trip | `5`, `5.5`, `0.5` |
| `Bool` | `True` / `False` — **capitalized**; the literals are `true`/`false` | `True` |
| `Text` | itself | `hi` |
| record | the **type name** (unless overridden) | `Point` |
| sum type | the **variant/constructor name** (unless overridden) | `Green`, `Ok` |
| array | length **≤ 10** → full `[a, b, c]` (each element via its own `` ` ``); length **> 10** → truncated `[first <- last]` | `[1, 2, 3]`, `[1 <- 100]` |
| [`Map`](../collections/map.md) | `[|=>|]` empty; length **≤ 10** → full `[|k => v, ...|]` (each key/value via its own `` ` ``); length **> 10** → truncated `[|first <- last|]`; entry order is unspecified | `[|ada => 36|]` |
| [`Set`](../collections/set.md) | `[||]` empty; length **≤ 10** → full `[|e, ...|]` (each element via its own `` ` ``); length **> 10** → truncated `[|first <- last|]`; element order is unspecified | `[|1, 2|]` |

Every type renders except a **function** value; handing one to `print` is a compile error
naming the missing member. Rendering takes no format specifiers. (See
`examples/interpolation.qn`.)

**On output, `print` shows the text for a reader and `write` passes the bytes through.**
`print`/`eprint` write text for a reader: each byte of a `Text` outside valid UTF-8
arrives as the replacement character `�`. [`write`](../corelib/io.md) renders its argument
the same way and passes the bytes through as they are. Both write the whole `Text`: a NUL
byte is content.
