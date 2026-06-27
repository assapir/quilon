~ Records are anonymous structs with named fields; access fields with `.`.
~ (Fields can hold any type — Text, arrays, etc.; see examples/composites.ql.)
^ = () -> Num => <
  rect = { width = 4, height = 7 }
  rect.width * rect.height      ~ exit 28
>
