~ Concrete `Result` payloads: a pattern-bound `Ok`/`NotOk` payload carries its REAL
~ type, so it is usable at the match site. A `Text` payload supports the full `Text`
~ API (`.size`/`.length`/`+`/comparison), and a payload routed through an overload set
~ dispatches by its concrete type — not the old generic-`Result` fallback to `Num`.
<< core.io

~ A `getEnv`-shaped helper: annotated `-> Result`, carrying a `Text` payload in BOTH
~ arms (`Ok(Text)` on success, `NotOk(Text)` on failure). The annotation is generic,
~ but the concrete payload the body pins flows through to the caller's match.
lookup = (key :: Text) -> Result => key == "home"
  ? Ok("/usr/home")
  : NotOk("unset")

~ An overload set: the member is chosen by the payload's concrete static type.
describe = (s :: Text) -> Num => s.size
describe = (n :: Num)  -> Num => n

^ = () -> Num => <
  ~ Ok(Text): the match binds `p : Text` and yields it — a usable `Text` value.
  path = lookup("home") ?
    | Ok(p)     => p
    | NotOk(_)  => "?"
  print(path)                        ~ prints: /usr/home
  hit = path.size                    ~ "/usr/home".size = 9

  ~ NotOk(Text): the error payload is Text too, and equally usable.
  miss = lookup("nope") ?
    | Ok(_)      => 0
    | NotOk(err) => err.size          ~ "unset".size = 5

  ~ Overload dispatch on the bound payload: `s : Text` picks the Text member.
  viaOverload = Ok("hi") ?
    | Ok(s)     => describe(s)         ~ describe(Text) = "hi".size = 2
    | NotOk(_)  => 0

  ~ Numeric payloads still work end-to-end, and `Ok($)` (unit payload) too.
  num  = Ok(5) ? | Ok(x) => x * 2 | NotOk(_) => 0   ~ 10
  unit = Ok($) ? | Ok(_) => 1       | NotOk(_) => 0  ~ 1

  hit + miss + viaOverload + num + unit              ~ 9 + 5 + 2 + 10 + 1 = 27
>
