~ Named record types can declare methods. Inside a method, `it` is the instance.
Counter = {
  value :: Num,

  bump = (by :: Num) -> Num => it.value + by
}

^ = () -> Num => <
  c = Counter { value = 30 }
  c.bump(5)        ~ it.value + 5 = 35
>
