//! Recursive and heterogeneous sum types: a payload naming an array/map/record of
//! itself, a sum boxed directly into its own payload, positions that disagree in type
//! across variants, a sum nested inside another, and a record referencing itself or a
//! sum declared above it. Drives the full pipeline (lex -> parse -> typecheck ->
//! codegen -> JIT) and asserts real exit codes, plus the checker-only cases (the new
//! payload diagnostic's message and span).

mod common;
use common::assert_exit;
use quilon::diagnostic::codes::Code;
use quilon::lexer::Lexer;
use quilon::parser;
use quilon::typechecker::TypeChecker;

#[test]
fn array_of_self_payload_walked() {
    // `Forest`'s payload is an array of its own type — arrays are already indirect
    // (`{ ptr, i64 }`), so no boxing is needed for the element type.
    assert_exit(
        "Forest = Leaf(Num) / Branch([]Forest)\n\
         sumLeaves = (f :: Forest) -> Num => <\n\
           f ?\n\
             | Leaf(n)     => n\n\
             | Branch(kids) => kids.reduce(0, (total, k) => total + sumLeaves(k))\n\
         >\n\
         ^ = () -> Num => <\n\
           tree = Branch([Leaf(3), Leaf(4), Branch([Leaf(5)])])\n\
           sumLeaves(tree)\n\
         >",
        12,
    );
}

#[test]
fn map_of_self_payload_walked() {
    // A map value payload naming the enclosing sum stays inline (a map value is a
    // pointer regardless of what it holds), so no boxing is needed there either.
    assert_exit(
        "Registry = Empty / Node([|Text => Registry|])\n\
         depth = (r :: Registry) -> Num => <\n\
           r ?\n\
             | Empty => 0\n\
             | Node(children) => (children.get(\"child\") ?\n\
                 | Ok(c)    => 1 + depth(c)\n\
                 | NotOk(_) => 1)\n\
         >\n\
         ^ = () -> Num => < depth(Node([|\"child\" => Empty|])) >",
        1,
    );
}

#[test]
fn direct_self_payload_boxed_three_deep() {
    // `Node(Tree)` names the enclosing sum directly: boxed into a GC cell at
    // construction, unboxed when a pattern binds it. Built three levels deep and
    // walked back out.
    assert_exit(
        "Tree = Leaf / Node(Tree)\n\
         depth = (t :: Tree) -> Num => <\n\
           t ?\n\
             | Leaf => 0\n\
             | Node(inner) => 1 + depth(inner)\n\
         >\n\
         ^ = () -> Num => < depth(Node(Node(Node(Leaf)))) >",
        3,
    );
}

#[test]
fn record_with_direct_self_field_chained_twice() {
    // `Crate = { children :: []Crate, label :: Num }` — a record referencing its own
    // type (through an array, since a bare required `next :: Crate` field would need
    // an already-built `Crate` before the first one exists). Reading two levels down
    // reaches the innermost label.
    assert_exit(
        "Crate = { children :: []Crate, label :: Num }\n\
         ^ = () -> Num => <\n\
           leaf = Crate { children = [], label = 7 }\n\
           mid = Crate { children = [leaf], label = 0 }\n\
           root = Crate { children = [mid], label = 0 }\n\
           root.children.at(0) ?\n\
             | Ok(m) => (m.children.at(0) ? | Ok(l) => l.label | NotOk(_) => -1)\n\
             | NotOk(_) => -1\n\
         >",
        7,
    );
}

#[test]
fn heterogeneous_positions_matched_on_every_variant() {
    // Three variants disagree on their payload types position-by-position:
    // `C(Num, Text)` shares neither of `A`/`B`'s positions.
    assert_exit(
        "Mixed = A(Num) / B(Text) / C(Num, Text)\n\
         describe = (m :: Mixed) -> Num => <\n\
           m ?\n\
             | A(n)    => n\n\
             | B(t)    => t.length\n\
             | C(n, t) => n + t.length\n\
         >\n\
         ^ = () -> Num => < describe(A(2)) + describe(B(\"abc\")) + describe(C(1, \"xy\")) >",
        8,
    );
}

#[test]
fn sum_nested_inside_a_different_sum() {
    // `Wrap(Shape)` embeds a DIFFERENT sum by value — `Shape` is declared above `Wrap`,
    // so its layout is already known and no boxing applies.
    assert_exit(
        "Shape = Circle(Num) / Square(Num)\n\
         Wrap = Empty / Holds(Shape)\n\
         area = (s :: Shape) -> Num => < s ? | Circle(r) => r * r | Square(n) => n * n >\n\
         ^ = () -> Num => <\n\
           w = Holds(Square(4))\n\
           w ? | Empty => 0 | Holds(s) => area(s)\n\
         >",
        16,
    );
}

#[test]
fn record_referencing_a_sum_declared_above() {
    // A record field may name a sum declared above it — top-down resolution, no
    // special-casing beyond the ordinary "declared above" rule.
    assert_exit(
        "Shape = Circle(Num) / Square(Num)\n\
         Holder = { shape :: Shape }\n\
         area = (s :: Shape) -> Num => < s ? | Circle(r) => r * r | Square(n) => n * n >\n\
         ^ = () -> Num => < area(Holder { shape = Circle(5) }.shape) >",
        25,
    );
}

#[test]
fn invalid_payload_type_names_variant_position_and_type() {
    // A payload naming nothing declared reports `InvalidPayloadType`, pointing at that
    // field's own span (not the whole declaration).
    let source = "Mystery = Wrap(Nope) / Empty\n^ = () -> Num => < 0 >";
    let tokens = Lexer::tokenize(source).expect("lexing failed");
    let program = parser::parse(&tokens).expect("parsing failed");
    let error = TypeChecker::new()
        .check_program(&program)
        .expect_err("expected a type error");
    assert_eq!(error.code(), Code::InvalidPayloadType);
    assert_eq!(
        error.to_string(),
        "`Wrap`'s payload 1 is Nope, which a sum type cannot carry"
    );
    let field_start = source.find("Nope").unwrap() as u32;
    assert_eq!(error.span().start, field_start);
    assert_eq!(error.span().end, field_start + "Nope".len() as u32);
}

#[test]
fn a_method_on_a_self_referencing_sum_may_call_itself_on_the_payload() {
    // An annotated method is registered under its own name before its body is
    // checked, so a self-call through a payload of the same type (or the render
    // member reached via interpolation) resolves instead of "no such member".
    assert_exit(
        "Tree = Leaf / Node(Tree, Num, Tree) {\n\
           sum = () -> Num => <\n\
             it ?\n\
               | Leaf => 0\n\
               | Node(left, value, right) => left.sum() + value + right.sum()\n\
           >\n\
           ` = () -> Text => <\n\
             it ?\n\
               | Leaf                => \"Leaf\"\n\
               | Node(left, value, _) => \"Node(`left`, `value`)\"\n\
           >\n\
         }\n\
         ^ = () -> Num => <\n\
           tree = Node(Node(Leaf, 1, Leaf), 2, Leaf)\n\
           assert(\"`tree`\", equals(\"Node(Node(Leaf, 1), 2)\"))\n\
           tree.sum()\n\
         >",
        3,
    );
}

#[test]
fn indexing_a_self_referencing_array_field_reaches_the_real_record() {
    // A read out of an array/map whose element type is a frozen self-reference
    // placeholder must resolve to the real record, the same way direct field access
    // does (`Expression::FieldAccess`'s `resolve_payload_type` call).
    assert_exit(
        "Wagon = { next :: []Wagon, cargo :: Num }\n\
         ^ = () -> Num => <\n\
           w = Wagon { next = [Wagon { next = [], cargo = 2 }], cargo = 1 }\n\
           w.next[0].cargo + w.cargo\n\
         >",
        3,
    );
}
