# Pattern matching

```quilon ignore
result = value ?
  | 0        => "zero"
  | 1        => "one"
  | Ok(x)    => x
  | NotOk(e) => 0
  | _        => "other"      ~ wildcard
```
The type checker verifies matches are exhaustive (use `_` to cover the rest). (See `examples/pattern_match.qn`.)
