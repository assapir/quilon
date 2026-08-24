~ Built-in Map collection type, pipe-fenced: `[|K => V|]` (`=>` reads "maps to").
~   Map: get / has / set / keys / values / size / each.
~ Values are read only through `.get`, which returns `Ok(value)` when the key is present
~   and `NotOk` when it is absent — there is no bracket indexing on a map.
~ Maps are IMMUTABLE — every mutator returns a NEW map. Iteration order is UNSPECIFIED
~   (reproducible run-to-run via a fixed-seed hasher, but never rely on it).
~ `<< core.test` verifies every result; on success the program exits 0.
<< core.test

^ = () -> $ => <
  ages :: [|Text => Num|] = [|"ada" => 36, "alan" => 41, "grace" => 45|]

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
  assertNotOk(older.get("nobody"))
  assertEq(older.size, 3)
  originalAge :: Num = ages.get("ada") ?
    | Ok(v)    => v
    | NotOk(_) => 0
  updatedAge :: Num = older.get("ada") ?
    | Ok(v)    => v
    | NotOk(_) => 0
  assertEq(originalAge, 36)
  assertEq(updatedAge, 37)

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
>
