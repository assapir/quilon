~ The unit type `$` has exactly one value, also written `$` (like () in Rust/ML).
~ Use it for side-effecting functions whose result is meaningless. `print` -> $.
<< core.io

~ A function whose result is meaningless: it returns `$` after logging.
log = (m :: Text) -> $ => print(m)

~ A `$` body exits 0 (the value is not a Num), so main needs no trailing 0.
^ = () -> $ => log("started")
