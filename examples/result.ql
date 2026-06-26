~ The built-in `Result` sum type: `Ok(value)` for success, `NotOk(error)` for
~ failure. Pattern-match to extract the payload. (Numeric payloads; richer payload
~ types await generics — see LANGUAGE.md.)
^ = () -> Num => <
  outcome = Ok(42)
  outcome ?
    | Ok(x) => x * 2       ~ 42 * 2 = 84
    | NotOk(e) => 0
>
