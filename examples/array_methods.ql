~ Built-in array methods (compiler-provided, chainable). Each takes a lambda that the
~ compiler INLINES (Quilon has no first-class closures):
~   map(f)            -> new array, element = f's result type
~   filter(pred)      -> new array, same element type (pred must return Bool)
~   reduce(init, f)   -> fold-left accumulator
~   each(f)           -> runs f for effect, returns the array itself (chainable)
~   find(pred)        -> Ok(elem) of the first match, else NotOk (absent-safe)
~   at(n)             -> Ok(elem) if in bounds, else NotOk (safe index)
^ = () -> Num => <
  nums = [1, 2, 3, 4, 5, 6]

  ~ Chain map -> filter -> reduce: double, keep those > 4, sum them -> 36.
  sum = nums
    .map(x => x * 2)              ~ [2, 4, 6, 8, 10, 12]
    .filter(x => x > 4)           ~ [6, 8, 10, 12]
    .reduce(0, (acc, x) => acc + x)   ~ 6+8+10+12 = 36

  doubled = nums.map(x => x * 2)  ~ [2, 4, 6, 8, 10, 12]

  ~ find: first element > 8 is 10 (the Ok path).
  hit = doubled.find(x => x > 8) ?
    | Ok(v)    => v               ~ 10
    | NotOk(_) => 0

  ~ find with no match -> NotOk path (yields 0).
  miss = doubled.find(x => x > 99) ?
    | Ok(v)    => v
    | NotOk(_) => 0               ~ 0

  ~ at: in-bounds Ok, then out-of-bounds NotOk.
  third = doubled.at(2) ?         ~ doubled[2] = 6
    | Ok(v)    => v               ~ 6
    | NotOk(_) => 0

  oob = doubled.at(42) ?          ~ out of range
    | Ok(v)    => v
    | NotOk(_) => 0               ~ 0

  sum + hit + miss + third + oob  ~ 36 + 10 + 0 + 6 + 0 = 52
>
