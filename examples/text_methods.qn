~ Built-in `Text` methods, verified with `core.test` assertions: every assertion below
~ holds, so the program runs to completion and exits 0 (a failing assertion prints to
~ stderr and exits 101). Methods (grapheme-based where an index/length is user-visible):
~   split(sep)                        -> []Text  (empty sep -> graphemes; empties preserved)
~   trim() / trimStart() / trimEnd()  -> Text    (strip both / leading / trailing whitespace)
~   replaceAll(from, to)              -> Text    (replace every occurrence)
~   replace(from, to, count)          -> Text    (exactly the first `count`; count > 0)
~   contains(sub)                     -> Bool
~   indexOf(sub)                      -> Ok(Num) grapheme index / NotOk   (no -1 sentinel)
~   slice(start, end)                 -> Text    (grapheme indices, clamped; end exclusive)
~   toUpper() / toLower()             -> Text    (Unicode-aware case mapping)
<< core.test

^ = () -> $ => <
  ~ split -> []Text; its pieces are genuine Text values.
  parts :: []Text = "Hello, World".split(", ")
  assertEq(parts.size, 2)
  assertEq(parts[0], "Hello")
  assertEq(parts[1], "World")
  ~ consecutive separators keep empty pieces; empty haystack -> [""]; empty sep -> graphemes.
  assertEq("a,,b".split(",").size, 3)
  assertEq("".split(",").size, 1)
  assertEq("héllo".split("").size, 5)
  ~ split on a 4-byte emoji separator.
  assertEq("a🌍b🌍c".split("🌍").size, 3)

  ~ trim both sides; trimStart / trimEnd one side only (Unicode whitespace).
  assertEq("  hi  ".trim(), "hi")
  assertEq("  hi  ".trimStart(), "hi  ")
  assertEq("  hi  ".trimEnd(), "  hi")

  ~ replaceAll rewrites every match; replace(count) exactly the first `count`.
  assertEq("a-a-a".replaceAll("a", "xx"), "xx-xx-xx")
  assertEq("a-a-a".replace("a", "xx", 1), "xx-a-a")
  assertEq("a-a-a".replace("a", "xx", 2), "xx-xx-a")
  ~ multibyte from/to: replace the 4-byte emoji with a 2-byte "é".
  assertEq("a🌍b🌍c".replaceAll("🌍", "é"), "aébéc")

  ~ contains -> Bool.
  assert("Hello, World".contains("World"))
  assert(!"Hello".contains("zzz"))
  assert("a🌍b".contains("🌍"))

  ~ indexOf -> Ok(grapheme index) when found, NotOk when absent.
  assertOk("héllo".indexOf("llo"))
  assertNotOk("Hello".indexOf("z"))
  ~ the index counts graphemes: "b" sits past the 4-byte 🌍, at grapheme 2.
  idx :: Num = "a🌍b".indexOf("b") ?
    | Ok(i)    => i
    | NotOk(_) => 0 - 1
  assertEq(idx, 2)

  ~ slice over grapheme indices, end exclusive; out-of-range clamps; never splits a
  ~ multibyte codepoint mid-byte.
  assertEq("Hello".slice(1, 4), "ell")
  assertEq("Hello".slice(-5, 100), "Hello")
  assertEq("Hello".slice(3, 1), "")
  assertEq("héllo".slice(1, 3), "él")

  ~ case mapping, incl. non-ASCII and the 1->N "ß" -> "SS".
  assertEq("abc".toUpper(), "ABC")
  assertEq("ABC".toLower(), "abc")
  assertEq("é".toUpper(), "É")
  assertEq("ß".toUpper(), "SS")

  ~ []Text is a plain generic array: it composes with the array methods and `+`.
  assertEq("aa,b,ccc".split(",").map(w => w.size).reduce(0, (a, x) => a + x), 6)
  cat :: []Text = "a,b".split(",") + "c,d".split(",")
  assertEq(cat.size, 4)
  assertEq(cat[3], "d")
>
