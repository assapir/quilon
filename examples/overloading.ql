~ Ad-hoc overloading: multiple same-named typed definitions form an overload set,
~ resolved at each call site by the EXACT static argument types (no coercion).
~ Operators are user-overloadable too — an operator is just a named overload set.

~ --- A user function overload set: same name, different parameter types. ---
~ The call site picks the member whose parameter type matches exactly.
score = (n :: Num) -> Num => n + 1        ~ Num overload
score = (s :: Text) -> Num => s.size      ~ Text overload (byte length)

~ --- A user operator overload on a record type. ---
~ `==` on Color compares the two components; it returns Bool, like any `==`.
Color = { r :: Num, g :: Num }
== = (a :: Color, b :: Color) -> Bool => a.r == b.r && a.g == b.g

^ = () -> Num => <
  ~ Overload dispatch by argument type:
  fromNum  = score(41)        ~ Num overload  -> 42
  fromText = score("abcd")    ~ Text overload -> 4

  ~ User operator overload (`==` on Color):
  sameColor = Color { r = 1, g = 2 } == Color { r = 1, g = 2 } ? 100 : 0   ~ -> 100
  diffColor = Color { r = 1, g = 2 } == Color { r = 9, g = 2 } ? 1 : 0     ~ -> 0

  ~ Built-in Text comparison overloads — equality and lexicographic ordering:
  textEq = "quilon" == "quilon" ? 7 : 0    ~ -> 7
  textLt = "abc" < "abd" ? 3 : 0           ~ -> 3 (lexicographic: 'c' < 'd')
  textGt = "b" > "a" ? 5 : 0               ~ -> 5 (bare `>` works on one line)

  ~ Deterministic total: 42 + 4 + 100 + 0 + 7 + 3 + 5 = 161
  fromNum + fromText + sameColor + diffColor + textEq + textLt + textGt
>
