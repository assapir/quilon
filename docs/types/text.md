---
title: "Text"
sidebar:
  order: 1
---

# `Text`
UTF-8 text. A **built-in** type (like `Num`/`Bool`/arrays) — **no import needed**.
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

## Text methods

`Text` carries **built-in, compiler-provided methods**, called as `text.method(...)` and
freely chainable. User-visible indices and lengths are **grapheme-based** (matching
`.length`), not byte-based.

| Method | Result | Notes |
|--------|--------|-------|
| `split(separator :: Text)` | `[]Text` | split on `separator`; consecutive separators keep empty pieces (`"a,,b".split(",")` → `["a","","b"]`), an empty haystack yields `[""]`, and an **empty** `separator` splits into individual graphemes (`"abc".split("")` → `["a","b","c"]`) |
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

These methods are **reserved on `Text`**, like the [array methods](../collections/arrays.md#array-methods)
are on arrays. A same-named user overload on another type is fine, but on a `Text` receiver
the built-in wins. `split` yields a plain `[]Text`, so it composes with `.size`, `[i]`, the
[array methods](../collections/arrays.md#array-methods), and array `+`. There is **no `join`** — collapse a `[]Text`
with `reduce` + `+`.

`replace`/`replaceAll`/`repeat` **fail loudly**. They never silently no-op or clamp. Three
inputs are rejected: an empty `from`; a `replace` `count` that is `<= 0` or exceeds the
occurrences present; and a negative or fractional `repeat` count. A literal violation is a
compile error (`"a".replace("a", "b", 0)`, `"aa".replace("a", "b", 5)`). A computed one is
a [located diagnostic](../tooling/errors.md) at run time, with exit `101`. Use `replaceAll`
for "replace everything"; `replace(count)` means exactly that many.

(See `examples/text.qn` and `examples/text_methods.qn`.)

## String interpolation and the render operator (`` ` ``)

A string literal may contain **interpolation holes**: expressions wrapped in backticks.
Each hole is rendered to `Text` and spliced in:

```quilon ignore
"hi `user.name`"      ~ splices the rendered value of user.name
"sum: `a + b`"        ~ any expression, not just a variable
"port `getPort()`"    ~ a call
```

A hole can be **any expression**, and its value can be of **any type** — every type is
renderable. To write a **literal backtick**, double it: `` `` `` yields one `` ` `` (never
starts a hole). A plain string with no holes is an ordinary `Text` literal.

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

A **function** value is the one thing that does not render; handing one to `print` names the
missing member. There are **no format specifiers** (width/precision/etc.). (See
`examples/interpolation.qn`.)

**On output, `print` shows the text for a reader and `write` does not.** `print`/`eprint`
write text for a reader: a `Text` whose bytes are not valid UTF-8 arrives with each invalid
byte shown as the replacement character `�`. [`write`](../corelib/io.md) renders its argument
the same way but passes the bytes through as they are. Both write the whole `Text`: a NUL byte
is content, never a terminator.
