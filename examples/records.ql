~ Records are anonymous structs with named fields; access fields with `.`.
~ (Fields are numeric here — Text/array fields inside composites await richer
~ payload support; see LANGUAGE.md "Known limitations".)
^ = () -> Num => <
  rect = { width = 4, height = 7 }
  rect.width * rect.height      ~ exit 28
>
