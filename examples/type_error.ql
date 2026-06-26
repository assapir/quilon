~ NEGATIVE example — this is intentionally rejected by the type checker, to show
~ Quilon catches type mismatches. `cargo run -- check examples/type_error.ql`
~ reports an error; the examples test asserts that it fails to compile.
^ = () -> Num => <
  x :: Num = "not a number"   ~ type mismatch: Text assigned to Num
  x
>
