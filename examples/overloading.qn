~ Ad-hoc overloading: multiple same-named typed definitions form an overload set,
~ resolved at each call site by the EXACT static argument types (no coercion).
~ Operators are user-overloadable too — an operator is just a named overload set.
~ `<< core.test` verifies every dispatch; on success the program exits 0.
<< core.test

~ --- A user function overload set: same name, different parameter types. ---
~ The call site picks the member whose parameter type matches exactly.
score = (n :: Num) -> Num => n + 1        ~ Num overload
score = (s :: Text) -> Num => s.size      ~ Text overload (byte length)

~ --- A user operator overload on a record type. ---
~ `==` on Color compares the two components; it returns Bool, like any `==`.
Color = { r :: Num, g :: Num }
== = (a :: Color, b :: Color) -> Bool => a.r == b.r && a.g == b.g

^ = () -> $ => <
  ~ Overload dispatch by argument type:
  assertEq(score(41), 42)      ~ Num overload
  assertEq(score("abcd"), 4)   ~ Text overload

  ~ User operator overload (`==` on Color):
  assert(Color { r = 1, g = 2 } == Color { r = 1, g = 2 })
  assert(!(Color { r = 1, g = 2 } == Color { r = 9, g = 2 }))

  ~ Built-in Text comparison overloads — equality and lexicographic ordering:
  assert("quilon" == "quilon")
  assert("abc" < "abd")        ~ lexicographic: 'c' < 'd'
  assert("b" > "a")            ~ bare `>` works on one line
>
