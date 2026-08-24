~ Built-in Set collection type, pipe-fenced: `[|T|]`.
~   Set: has / add / items / size / each ; `+` union, `-` difference, `+-`/`-+` intersect.
~ Sets are IMMUTABLE — every mutator returns a NEW set. Iteration order is UNSPECIFIED
~   (reproducible run-to-run via a fixed-seed hasher, but never rely on it).
~ `<< core.test` verifies every result; on success the program exits 0.
<< core.test

^ = () -> $ => <
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
