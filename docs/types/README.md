# Types

## `Num`
All numbers — integers and floats are one unified type: an IEEE-754 double (64-bit float).
```quilon
x = 42
y = 3.14
z = x + y          ~ mixed arithmetic
```

## `Bool`
`true` / `false` (the literals are lowercase; note that a `Bool` *renders* as capitalized
`True`/`False` — see [interpolation](text.md#string-interpolation-and-the-render-operator-)).

## `Unit` — `$`
The **unit type**, written `$`. It has exactly one value, also written `$` — so `$` is
both the type (in type position, e.g. `-> $`) and its sole value (in value position),
analogous to `()` in Rust/ML. Use it for side-effecting expressions and functions whose
result is meaningless. `print` and `eprint` return `$`. `$` is compatible only with `$`.
```quilon
log = (m :: Text) -> $ => print(m)   ~ a function whose result is meaningless
^ = () -> $ => log("started")        ~ a `$` body exits 0 (it is not a Num)
```

Arrays (`[]T`) live with the other built-in parametric collections — see
[`collections/arrays.md`](../collections/arrays.md).
