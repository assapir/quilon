~ String interpolation: backtick holes inside a string splice in rendered values.
~ Every value renders through its `` ` `` operator — a built-in default per type, or a
~ user override. `print` uses the same render path. `<< core.test` checks each result.
<< core.test

~ A record overrides its own rendering with the `` ` `` operator: `it` is the instance,
~ and the body may itself interpolate.
User = {
  name :: Text,
  age :: Num,
  ` = () -> Text => "User(`it.name`, `it.age`)"
}

~ A record with no override renders as just its type name.
Point = { x :: Num, y :: Num }

~ A sum type renders as the variant/constructor name.
Color = Red / Green / Blue

^ = () -> $ => <
  ~ Numbers: integer-valued without decimals, otherwise shortest round-trip.
  assertEq("`42`", "42")
  assertEq("half is `1 / 2`", "half is 0.5")

  ~ Booleans render capitalized (True/False), unlike the true/false literals.
  assertEq("`true` and `false`", "True and False")

  ~ Arbitrary expression holes.
  assertEq("sum `1 + 2 + 3`", "sum 6")

  ~ A doubled backtick is one literal backtick (no interpolation): "a`b" is 3 bytes.
  assertEq("a``b".size, 3)

  ~ A user override drives both interpolation and print.
  u :: User = User { name = "Ada", age = 36 }
  assertEq("`u`", "User(Ada, 36)")
  assertEq("hi `u`!", "hi User(Ada, 36)!")

  ~ Default record rendering is the type name.
  p :: Point = Point { x = 1, y = 2 }
  assertEq("`p`", "Point")

  ~ Default sum rendering is the variant name.
  c :: Color = Green
  assertEq("`c`", "Green")

  ~ Arrays up to 10 elements render in full; longer ones truncate to first <- last.
  small :: []Num = [1, 2, 3]
  assertEq("`small`", "[1, 2, 3]")
  big :: []Num = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]
  assertEq("`big`", "[1 <- 12]")

  $
>
