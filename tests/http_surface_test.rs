//! What `<< core.http` costs an importer's namespace: nothing bare.
//!
//! Every item a module contributes arrives under its qualified name (`core.http.Response`),
//! privates included — so no contributed name can collide with anything the importer
//! writes, and the module's exports are reached only through its binding.

mod common;

use std::path::Path;

use quilon::lexer::Lexer;
use quilon::parser;

#[test]
fn core_http_contributes_no_bare_names() {
    let tokens = Lexer::tokenize("<< core.http\n^ = () -> Num => < 0 >\n").expect("lexing");
    let program = parser::parse(&tokens).expect("parsing");
    let (linked, _sources) =
        quilon::modules::link(program, Path::new(".")).expect("import linking failed");
    // `core.http` imports `core.net` and `core.test`, whose items arrive with it. Every
    // contributed item — everything but the program's own `^` — is either qualified under
    // its module's canonical name or an `@` leaf primitive (which keeps its bare,
    // sigil-marked name by design).
    let items: Vec<_> = linked
        .items
        .iter()
        .filter(|item| item.name() != "^")
        .collect();
    for item in &items {
        let name = item.name();
        assert!(
            name.starts_with("core.") || name.starts_with('@'),
            "an imported module may not contribute a bare name: `{name}`"
        );
    }
    // The exported surface is reachable under the client's own prefix.
    for name in ["Body", "Method", "Response", "Request"] {
        let qualified = format!("core.http.{name}");
        assert!(
            items.iter().any(|item| item.name() == qualified),
            "`<< core.http` must contribute `{qualified}`"
        );
    }
}

/// The surface an importer feels: a program that claims the client's own helper names —
/// and even its exported type names — for itself, and still reads a reply through
/// `http.Response`. Qualified access means nothing merges, so nothing can collide.
#[test]
fn an_importer_may_define_every_name_the_client_uses() {
    let source = concat!(
        "<< core.http\n",
        "get = (url :: Text) -> Text => < url >\n",
        "parseResponse = (raw :: Text) -> Text => < raw >\n",
        "wire = (text :: Text) -> Text => < text >\n",
        "head = (text :: Text) -> Text => < text >\n",
        "blankLine = (text :: Text) -> Num => < text.length >\n",
        "authorityEnd = (text :: Text) -> Num => < text.length >\n",
        "withoutScheme = (text :: Text) -> Text => < text >\n",
        "Response = { note :: Text }\n",
        "^ = () -> $ => <\n",
        "  reply = http.Response { raw = \"HTTP/1.1 200 OK\\r\\nX-A: b\\r\\n\\r\\nhi\" }\n",
        "  assert(reply.validate(), isOk())\n",
        "  assert(reply.status(), equals(200))\n",
        "  assert(reply.body(), equals(\"hi\"))\n",
        "  assert(reply.header(\"x-a\"), isOk())\n",
        "  own = Response { note = \"mine\" }\n",
        "  assert(own.note, equals(\"mine\"))\n",
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
        "an importer must be free to define every name `core.http` keeps to itself:\n{}{}",
        run.stdout, run.stderr
    );
}
