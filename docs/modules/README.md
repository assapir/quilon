# Modules

```quilon ignore
<< core.io                 ~ import the built-in IO module
<< "lib/math.qn"           ~ import a user module by path (/ or \)

>> add = (a :: Num, b :: Num) => a + b   ~ `>>` exports an item; unmarked items are file-private
```
- The built-in modules are `core.io`, `core.test`, `core.test.report`, `core.cli`, `core.time`, `core.net`, and `core.http`; their members are real functions. See the [corelib](../corelib/README.md) index for each module's API reference.
- `Text` and the operators are built-ins and need **no** import.
- A module exposes only its `>>`-exported items.

(See `examples/use_module.qn`, which imports `examples/mathlib.qn`.)
