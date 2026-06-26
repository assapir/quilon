~ Pattern matching with `?` and `|` arms; `_` is the wildcard.
^ = () -> Num => <
  value = 5
  value ?
    | 0 => 10
    | 5 => 50      ~ matches here -> exit 50
    | _ => 99
>
