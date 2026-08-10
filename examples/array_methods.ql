~ Built-in array methods (compiler-provided, chainable). Each takes a lambda that the
~ compiler INLINES per element (rather than passing it as a function value):
~   map(f)            -> new array, element = f's result type
~   filter(pred)      -> new array, same element type (pred must return Bool)
~   reduce(init, f)   -> fold-left accumulator
~   each(f)           -> runs f for effect, returns the array itself (chainable)
~   find(pred)        -> Ok(elem) of the first match, else NotOk (absent-safe)
~   at(n)             -> Ok(elem) if in bounds, else NotOk (safe index)
~ `<< core.test` verifies every result; on success the program exits 0.
<< core.test

^ = () -> $ => <
  nums :: []Num = [1, 2, 3, 4, 5, 6]

  ~ Chain map -> filter -> reduce: double, keep those > 4, sum them.
  sum :: Num = nums
    .map(x => x * 2)              ~ [2, 4, 6, 8, 10, 12]
    .filter(x => x > 4)           ~ [6, 8, 10, 12]
    .reduce(0, (acc, x) => acc + x)   ~ 6+8+10+12
  assertEq(sum, 36)

  doubled :: []Num = nums.map(x => x * 2)  ~ [2, 4, 6, 8, 10, 12]

  ~ find: first element > 8 is 10 (the Ok path).
  hit :: Num = doubled.find(x => x > 8) ?
    | Ok(v)    => v
    | NotOk(_) => 0
  assertEq(hit, 10)

  ~ find with no match -> NotOk path.
  miss :: Result = doubled.find(x => x > 99)
  assertNotOk(miss)

  ~ at: in-bounds Ok, then out-of-bounds NotOk.
  third :: Num = doubled.at(2) ?         ~ doubled[2] = 6
    | Ok(v)    => v
    | NotOk(_) => 0
  assertEq(third, 6)
  assertOk(doubled.at(2))
  assertNotOk(doubled.at(42))
>
