~ Pattern matching with `?` and `|` arms; `_` is the wildcard.
~ A constructor pattern's argument must be IRREFUTABLE — a binding (`Ok(x)`) or
~ `_` (`Ok(_)`). A literal there (`Ok(1)`) is a compile error: dispatch tests the
~ constructor tag only, so it would silently match ANY payload. Bind the payload
~ and compare it in the arm body instead, as `pick` does below.
<< core.test

^ = () -> Num => <
  ~ Binding and wildcard payload patterns.
  r = Ok(2)
  bound = r ?
    | Ok(x)     => x + 1
    | NotOk(_)  => 0
  assertEq(bound, 3)

  ~ The compare-in-the-body idiom replacing the illegal `| Ok(1) =>` form.
  pick = (v :: Num) -> Num => <
    q = Ok(v)
    q ?
      | Ok(n)    => n == 1 ? 10 : 20
      | NotOk(_) => 30
  >
  assertEq(pick(1), 10)
  assertEq(pick(2), 20)

  value = 5
  value ?
    | 0 => 10
    | 5 => 50      ~ matches here -> exit 50
    | _ => 99
>
