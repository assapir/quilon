//! What `<< core.http` costs an importer's namespace.
//!
//! Corelib exports are merged FLAT into the importing program's global scope, and a
//! same-name-same-signature definition there is a hard `Duplicate definition` error — so the
//! client's public surface is held to four type names, and these tests are what holds it there.

mod common;

use std::path::Path;

use quilon::ast::Item;
use quilon::lexer::Lexer;
use quilon::parser;

/// The whole public surface of `core.http`: type names, no free functions.
const EXPORTED: &[&str] = &["Body", "Method", "Response", "Request"];

/// Names the client keeps to itself — free functions it does not have, and methods that are
/// members of a record rather than items of their own. None may reach an importer's scope.
const NOT_EXPORTED: &[&str] = &[
    "get",
    "parseResponse",
    "wire",
    "head",
    "blankLine",
    "authorityEnd",
    "withoutScheme",
];

#[test]
fn core_http_merges_its_type_names_and_nothing_else() {
    let tokens = Lexer::tokenize("<< core.http\n^ = () -> Num => 0\n").expect("lexing");
    let program = parser::parse(&tokens).expect("parsing");
    let (items, _sources) =
        quilon::modules::resolve_imports(&program, Path::new(".")).expect("import resolution");
    // `core.http` imports `core.net`, whose primitive arrives with it; the client's own
    // contribution is what is under test.
    let contributed: Vec<&str> = items.iter().map(Item::name).collect();
    for name in EXPORTED {
        assert!(
            contributed.contains(name),
            "`<< core.http` must contribute `{name}`: {contributed:?}"
        );
    }
    for name in NOT_EXPORTED {
        assert!(
            !contributed.contains(name),
            "`<< core.http` must not contribute `{name}`: {contributed:?}"
        );
    }
}

/// The surface an importer feels: a program that claims every one of those names for itself and
/// still reads a reply through `Response`. Only `get` and `parseResponse` could ever have
/// collided — the rest are record members, which occupy no top-level name — so this is the
/// end-to-end half of the check above, run rather than inspected.
#[test]
fn an_importer_may_define_every_name_the_client_does_not_export() {
    let source = concat!(
        "<< core.http\n",
        "get = (url :: Text) -> Text => url\n",
        "parseResponse = (raw :: Text) -> Text => raw\n",
        "wire = (text :: Text) -> Text => text\n",
        "head = (text :: Text) -> Text => text\n",
        "blankLine = (text :: Text) -> Num => text.length\n",
        "authorityEnd = (text :: Text) -> Num => text.length\n",
        "withoutScheme = (text :: Text) -> Text => text\n",
        "^ = () -> $ => <\n",
        "  reply = Response { raw = \"HTTP/1.1 200 OK\\r\\nX-A: b\\r\\n\\r\\nhi\" }\n",
        "  assert(reply.validate(), isOk())\n",
        "  assert(reply.status(), equals(200))\n",
        "  assert(reply.body(), equals(\"hi\"))\n",
        "  assert(reply.header(\"x-a\"), isOk())\n",
        "  assert(get(\"u\"), equals(\"u\"))\n",
        "  assert(parseResponse(\"r\"), equals(\"r\"))\n",
        "  assert(wire(\"w\"), equals(\"w\"))\n",
        "  assert(head(\"h\"), equals(\"h\"))\n",
        "  assert(blankLine(\"abc\"), equals(3))\n",
        "  assert(authorityEnd(\"abcd\"), equals(4))\n",
        "  assert(withoutScheme(\"s\"), equals(\"s\"))\n",
        ">\n",
    );

    let run = common::run_program_named("own_names.qn", source);
    assert_eq!(
        run.code, 0,
        "an importer must be free to define every name `core.http` does not export:\n{}{}",
        run.stdout, run.stderr
    );
}
