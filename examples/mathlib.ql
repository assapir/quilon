~ A library module. Items prefixed with `>>` are exported; everything else is
~ private to the module. Imported elsewhere with `<< "mathlib.ql"`.

>> add = (a :: Num, b :: Num) -> Num => a + b

~ Not exported -> invisible to importers.
helper = (x :: Num) -> Num => x * x
