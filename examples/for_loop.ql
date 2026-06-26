~ Loops: `for <pattern> <- <collection> => <body>`. They run for side effects and
~ yield Num 0. Use `(item, index)` to also bind the 0-based index.
<< core.io

^ = () -> Num => <
  for n <- [1, 2, 3] => print(n)             ~ prints 1, 2, 3
  for (val, i) <- [10, 20, 30] => print(val + i)   ~ prints 10, 21, 32
  0
>
