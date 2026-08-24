~ Arrays are `{ptr, size}`. `.size` is the element count; `[i]` indexes (0-based).
~ Indexing is CHECKED: an out-of-bounds, negative, or NaN index is a runtime
~ error (stderr + exit 1), never a raw read. `at(i)` is the non-aborting form.
~ `<< core.test` verifies the results; on success the program exits 0.
<< core.test

^ = () -> $ => <
  nums :: []Num = [1, 2, 3, 4, 5]
  first :: Num = nums[0]
  assertEq(first, 1)
  assertEq(nums[4], 5)
  assertEq(nums[4.6], 5)          ~ fractional index truncates toward zero
  assertEq(nums.size, 5)

  ~ `at` returns Ok in bounds, NotOk otherwise — including a NaN index.
  assertOk(nums.at(2))
  assertNotOk(nums.at(9))
  assertNotOk(nums.at(0 - 1))
  assertNotOk(nums.at(0 / 0))
>
