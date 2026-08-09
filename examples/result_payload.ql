~ Concrete `Result` payloads: a pattern-bound `Ok`/`NotOk` payload carries its REAL
~ type, so it is usable at the match site. A `Text` payload supports the full `Text`
~ API (`.size`/`.length`/`+`/comparison), and a payload routed through an overload set
~ dispatches by its concrete type — not the old generic-`Result` fallback to `Num`.
~ `<< core.test` verifies every result; on success the program exits 0.
<< core.io
<< core.test

~ A `getEnv`-shaped helper: annotated `-> Result`, carrying a `Text` payload in BOTH
~ arms (`Ok(Text)` on success, `NotOk(Text)` on failure). The annotation is generic,
~ but the concrete payload the body pins flows through to the caller's match.
lookup = (key :: Text) -> Result => key == "home"
  ? Ok("/usr/home")
  : NotOk("unset")

~ An overload set: the member is chosen by the payload's concrete static type.
describe = (s :: Text) -> Num => s.size
describe = (n :: Num)  -> Num => n

^ = () -> $ => <
  ~ Ok(Text): the match binds `p : Text` and yields it — a usable `Text` value.
  path :: Text = lookup("home") ?
    | Ok(p)     => p
    | NotOk(_)  => "?"
  print(path)                        ~ prints: /usr/home
  assertEq(path, "/usr/home")
  assertEq(path.size, 9)             ~ "/usr/home".size = 9

  ~ NotOk(Text): the error payload is Text too, and equally usable.
  miss :: Num = lookup("nope") ?
    | Ok(_)      => 0
    | NotOk(err) => err.size          ~ "unset".size = 5
  assertEq(miss, 5)

  ~ Overload dispatch on the bound payload: `s : Text` picks the Text member.
  viaOverload :: Num = Ok("hi") ?
    | Ok(s)     => describe(s)         ~ describe(Text) = "hi".size = 2
    | NotOk(_)  => 0
  assertEq(viaOverload, 2)

  ~ Numeric payloads still work end-to-end, and `Ok($)` (unit payload) too.
  num :: Num = Ok(5) ? | Ok(x) => x * 2 | NotOk(_) => 0   ~ 10
  unit :: Num = Ok($) ? | Ok(_) => 1     | NotOk(_) => 0  ~ 1
  assertEq(num, 10)
  assertEq(unit, 1)
>
