~ A sum type may carry a trailing `{ }` block of METHODS (the block is optional; a sum
~ with no methods is written exactly as before). Inside a method `it` is the whole sum
~ value — a method typically matches on it. A method may be named, an operator member
~ (`==`, with `it` the left operand and the one parameter the right), or the render `` ` ``.
~ `<< core.test` verifies every result; on success the program exits 0.
<< core.test

Shape = Circle(Num) / Rect(Num, Num) {
  ~ A named method that matches on `it`:
  area = () -> Num => it ?
    | Circle(r)  => 3 * r * r         ~ ~pi*r^2 (pi ~= 3 here)
    | Rect(w, h) => w * h

  ~ A binary operator member: two shapes are equal when their areas match.
  == = (other :: Shape) -> Bool => it.area() == other.area()

  ~ The render operator `` ` `` — a Shape's Text form:
  ` = () -> Text => it ?
    | Circle(r)  => "Circle(`r`)"
    | Rect(w, h) => "Rect(`w`x`h`)"
}

^ = () -> $ => <
  ~ A named method, dispatched on the receiver's type:
  assertEq(Rect(6, 7).area(), 42)     ~ 6 * 7
  assertEq(Circle(4).area(), 48)      ~ 3 * 4 * 4

  ~ The `==` member, resolved from Shape's methods (equal areas):
  assert(Rect(6, 7) == Rect(2, 21))   ~ 42 == 42
  assert(!(Rect(6, 7) == Circle(4)))  ~ 42 != 48

  ~ The `` ` `` member renders a Shape (interpolation takes the same path as `print`):
  assertEq("`Rect(6, 7)`", "Rect(6x7)")
  assertEq("`Circle(4)`", "Circle(4)")
>
