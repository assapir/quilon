~ In-place record mutation. Mutability is decided by the binding operator:
~   `=`  -> immutable (frozen): no field writes, no setter-method calls.
~   `:=` -> mutable: in-place field writes AND setter methods are allowed.
~ A method is a "setter" iff its body writes `it.field := …` (the visible `:=`
~ is the signal — there is no marker). Calling a setter needs a `:=` receiver.
Counter = {
  value :: Num,

  ~ A setter: its body writes `it.value := …`, so it mutates in place.
  bump = (by :: Num) => it.value := it.value + by,

  ~ A getter: no `it.field := …`, so it is callable on `=` instances too.
  peek = => it.value
}

^ = () -> Num => <
  c := Counter { value = 30 }   ~ `:=` -> mutable instance
  c.bump(5)                      ~ setter mutates: value = 35
  c.value := c.value + 7         ~ direct field write: value = 42
  c.peek()                       ~ exit 42
>
