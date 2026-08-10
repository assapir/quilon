~ Arrays are `{ptr, size}`. `.size` is the element count; `[i]` indexes (0-based).
~ `<< core.test` verifies the results; on success the program exits 0.
<< core.test

^ = () -> $ => <
  nums :: []Num = [1, 2, 3, 4, 5]
  first :: Num = nums[0]
  assertEq(first, 1)
  assertEq(nums.size, 5)
>
