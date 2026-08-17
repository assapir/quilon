~ Built-in Map and Set collection types, pipe-fenced: `[|K => V|]` and `[|T|]`.
~   Map: get / has / set / keys / values / size / each ; fail-loud `m[k]`, safe `.get`.
~   Set: has / add / items / size / each ; `+` union, `-` difference, `+-`/`-+` intersect.
~ Both are IMMUTABLE — every mutator returns a NEW collection. Iteration order is
~ UNSPECIFIED (reproducible run-to-run via a fixed-seed hasher, but never rely on it).
~ `<< core.test` verifies every result; on success the program exits 0.
<< core.test

^ = () -> $ => <
  ~ ---- Map ----------------------------------------------------------------
  ages :: [|Text => Num|] = [|"ada" => 36, "alan" => 41, "grace" => 45|]

  ~ Fail-loud keyed access with `m[k]` (crashes on a missing key — see `.get` below).
  assertEq(ages["ada"], 36)
  assertEq(ages.size, 3)
  assert(ages.has("grace"))
  assert(!ages.has("nobody"))

  ~ Safe lookup returns a Result: Ok(value) present, NotOk absent.
  assertOk(ages.get("alan"))
  assertNotOk(ages.get("nobody"))
  found :: Num = ages.get("alan") ?
    | Ok(v)    => v
    | NotOk(_) => 0
  assertEq(found, 41)

  ~ `set` is persistent: it returns a NEW map, leaving the original untouched.
  older :: [|Text => Num|] = ages.set("ada", 37)
  assertEq(ages["ada"], 36)
  assertEq(older["ada"], 37)
  assertEq(older.size, 3)

  ~ keys / values are plain arrays (order unspecified), so array methods compose.
  total :: Num = ages.values().reduce(0, (acc, x) => acc + x)
  assertEq(total, 122)
  assertEq(ages.keys().size, 3)

  ~ each visits every entry for effect and returns the receiver (so it chains).
  sum := 0
  ages.each((name, age) => <
    sum := sum + age
  >
  )
  assertEq(sum, 122)

  ~ ---- Set ----------------------------------------------------------------
  primes :: [|Num|] = [|2, 3, 5, 7|]
  evens :: [|Num|] = [|2, 4, 6, 8|]

  ~ Duplicates collapse; membership and size are built in.
  dups :: [|Num|] = [|1, 1, 2, 2, 3|]
  assertEq(dups.size, 3)
  assert(primes.has(7))
  assert(!primes.has(4))

  ~ add is persistent, like map.set.
  more :: [|Num|] = primes.add(11)
  assertEq(primes.size, 4)
  assertEq(more.size, 5)

  ~ items is a plain array of the elements.
  assertEq(primes.items().size, 4)

  ~ Set algebra: `+` union, `-` difference, `+-`/`-+` (symmetric) intersection.
  assertEq((primes + evens).size, 7)   ~ {2,3,5,7,4,6,8}
  assertEq((primes - evens).size, 3)   ~ {3,5,7}
  assertEq((primes +- evens).size, 1)  ~ {2}
  assertEq((primes -+ evens).size, 1)  ~ same operator, other spelling

  ~ each over a set, chaining on the returned receiver.
  count := 0
  primes.each(p => <
    count := count + 1
  >
  )
  assertEq(count, 4)
>
