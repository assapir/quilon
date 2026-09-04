//! The language server's per-capability analysis, driven directly: diagnostics, hover,
//! go-to-definition (including across an import), completion, semantic tokens, and test
//! lenses.

use std::path::{Path, PathBuf};

use quilon::lexer::ROOT_FILE;
use quilon::lsp::analysis::{
    self, CompletionKind, SemanticTokenKind, TestLensKind, check_text, completions_at,
    definition_at, hover_at, is_identifier, references_at, semantic_tokens, test_lenses,
};

/// A unique temporary directory for a test that needs real files (import resolution).
fn temporary_directory(tag: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "quilon_lsp_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).expect("create temp dir");
    directory
}

/// The byte offset of `needle`'s first occurrence in `text`, plus `into` bytes.
fn offset_of(text: &str, needle: &str, into: u32) -> u32 {
    text.find(needle).expect("needle present in text") as u32 + into
}

/// The front-end error `check_text` reports for `text`, which must not check clean.
fn check_error(path: &Path, text: &str) -> quilon::driver::FrontEndError {
    match check_text(path, text) {
        Err(error) => error,
        Ok(_) => panic!("the text must not check clean"),
    }
}

// --- Diagnostics ------------------------------------------------------------

#[test]
fn a_type_error_carries_its_span_and_message() {
    let text = "^ = () -> Num => < 1 + true >\n";
    let error = check_error(Path::new("buffer.qn"), text);
    let span = error.diagnostic.primary_span().expect("a located error");
    assert_eq!(span.file, ROOT_FILE);
    assert_eq!(span.start, offset_of(text, "1 + true", 0));
    assert!(
        error.diagnostic.message.contains("+"),
        "unexpected message: {}",
        error.diagnostic.message
    );
}

#[test]
fn a_clean_program_produces_no_diagnostic() {
    let text = "^ = () -> Num => < 0 >\n";
    assert!(check_text(Path::new("buffer.qn"), text).is_ok());
}

#[test]
fn a_test_suite_is_checked_with_its_blocks_compiled() {
    // Inside an `it` body, a type error must surface — a suite runs its blocks, so the
    // server checks them, where `quilon check` would erase them.
    let text = "<< core.test\n\ntest.describe(\"math\", () => <\n  test.it(\"adds\", () => <\n    expect(1 + true, equals(2))\n  >)\n>)\n";
    let error = check_error(Path::new("suite.qn"), text);
    let span = error.diagnostic.primary_span().expect("a located error");
    assert_eq!(span.start, offset_of(text, "1 + true", 0));

    // The same suite with the operands fixed checks clean.
    let clean = text.replace("1 + true", "1 + 1");
    assert!(check_text(Path::new("suite.qn"), &clean).is_ok());
}

// --- Hover ------------------------------------------------------------------

#[test]
fn hover_reports_the_smallest_covering_expressions_type() {
    let text = "double = (x :: Num) -> Num => < x * 2 >\n^ = () -> Num => < double(21) >\n";
    let checked = check_text(Path::new("buffer.qn"), text).expect("checks clean");

    // On the parameter reference `x` in the body: its own type, not the product's.
    let (label, span) = hover_at(&checked.types, offset_of(text, "x * 2", 0)).expect("a hover");
    assert_eq!(label, "Num");
    assert_eq!(
        (span.start, span.end),
        (offset_of(text, "x * 2", 0), offset_of(text, "x * 2", 1))
    );

    // On a `Text` literal.
    let text = "^ = () -> Num => < s = \"hi\"\n0 >\n";
    let checked = check_text(Path::new("buffer.qn"), text).expect("checks clean");
    let (label, _) = hover_at(&checked.types, offset_of(text, "\"hi\"", 1)).expect("a hover");
    assert_eq!(label, "Text");
}

// --- Go-to-definition -------------------------------------------------------

#[test]
fn definition_resolves_parameters_locals_and_top_level_functions() {
    let text = "double = (x :: Num) -> Num => < x * 2 >\n^ = () -> Num => < y = 3\ndouble(y) >\n";
    let checked = check_text(Path::new("buffer.qn"), text).expect("checks clean");

    // `x` in the body resolves to the parameter, at the parameter's own span — the name
    // through its type annotation.
    let definition =
        definition_at(&checked.program, offset_of(text, "x * 2", 0)).expect("x resolves");
    assert_eq!(definition.start, offset_of(text, "x :: Num", 0));

    // `y` in the call resolves to the block-local binding.
    let definition =
        definition_at(&checked.program, offset_of(text, "double(y)", 7)).expect("y resolves");
    assert_eq!(definition.start, offset_of(text, "y = 3", 0));

    // `double` in the call resolves to the top-level function.
    let definition =
        definition_at(&checked.program, offset_of(text, "double(y)", 0)).expect("double resolves");
    assert_eq!(definition.start, 0);

    // A position on nothing resolvable answers nothing.
    assert!(definition_at(&checked.program, offset_of(text, "* 2", 0)).is_none());
}

#[test]
fn definition_resolves_across_a_file_import() {
    let directory = temporary_directory("import_definition");
    let module_text = ">> add = (a :: Num, b :: Num) -> Num => < a + b >\n";
    std::fs::write(directory.join("lib.qn"), module_text).expect("write module");

    let text = "<< \"lib.qn\"\n\n^ = () -> Num => < lib.add(1, 2) >\n";
    let root = directory.join("buffer.qn");
    let checked = check_text(&root, text).expect("checks clean");

    let definition = definition_at(&checked.program, offset_of(text, "lib.add", 4))
        .expect("the imported function resolves");
    assert_ne!(
        definition.file, ROOT_FILE,
        "the definition is in the module's own file"
    );

    let location = checked
        .sources
        .locate(&definition)
        .expect("a locatable definition");
    assert!(
        location.path.ends_with("lib.qn"),
        "unexpected path: {}",
        location.path
    );
    assert_eq!(definition.start, offset_of(module_text, ">> add", 0));

    std::fs::remove_dir_all(&directory).ok();
}

// --- Find references ---------------------------------------------------------

/// The byte offsets `references_at` answers, sorted (its own contract), for the easiest
/// possible assertion: a plain list of numbers.
fn reference_starts(program: &quilon::ast::Program, text: &str, offset: u32) -> Vec<u32> {
    references_at(program, text, offset)
        .expect("a resolvable target")
        .into_iter()
        .map(|span| span.start)
        .collect()
}

#[test]
fn references_cover_a_parameters_declaration_and_every_use() {
    let text = "double = (x :: Num) -> Num => < x * 2 >\n^ = () -> Num => < double(4) >\n";
    let checked = check_text(Path::new("buffer.qn"), text).expect("checks clean");

    let expected = vec![offset_of(text, "x :: Num", 0), offset_of(text, "x * 2", 0)];

    // From a use inside the body...
    assert_eq!(
        reference_starts(&checked.program, text, offset_of(text, "x * 2", 0)),
        expected
    );
    // ...and from the declaration itself, the answer is the same.
    assert_eq!(
        reference_starts(&checked.program, text, offset_of(text, "x :: Num", 0)),
        expected
    );
}

#[test]
fn references_to_a_block_local_stay_inside_its_own_function() {
    // Two functions each bind a local named `y`; references from one must never pull in
    // the other's declaration or uses.
    let text = "f = () -> Num => < y = 1\ny >\n^ = () -> Num => < y = 2\ny + y >\n";
    let checked = check_text(Path::new("buffer.qn"), text).expect("checks clean");

    assert_eq!(
        reference_starts(&checked.program, text, offset_of(text, "y = 1", 0)),
        vec![offset_of(text, "y = 1", 0), offset_of(text, "y >", 0)]
    );
    assert_eq!(
        reference_starts(&checked.program, text, offset_of(text, "y = 2", 0)),
        vec![
            offset_of(text, "y = 2", 0),
            offset_of(text, "y + y", 0),
            offset_of(text, "y + y", 4),
        ]
    );
}

#[test]
fn references_to_a_top_level_name_cover_every_overload_member() {
    let text = "same = (x :: Num) -> Num => < x >\n\
                 same = (x :: Text) -> Text => < x >\n\
                 ^ = () -> Num => < same(1) >\n";
    let checked = check_text(Path::new("buffer.qn"), text).expect("checks clean");

    assert_eq!(
        reference_starts(&checked.program, text, offset_of(text, "same(1)", 0)),
        vec![
            offset_of(text, "same = (x :: Num)", 0),
            offset_of(text, "same = (x :: Text)", 0),
            offset_of(text, "same(1)", 0),
        ]
    );
}

#[test]
fn references_to_a_pattern_binding_cover_its_uses() {
    let text = "^ = () -> Num => < Ok(5) ? | Ok(n) => n * n | NotOk(_) => 0 >\n";
    let checked = check_text(Path::new("buffer.qn"), text).expect("checks clean");

    assert_eq!(
        reference_starts(&checked.program, text, offset_of(text, "n * n", 0)),
        vec![
            offset_of(text, "Ok(n)", 3),
            offset_of(text, "n * n", 0),
            offset_of(text, "n * n", 4),
        ]
    );
}

#[test]
fn references_answer_nothing_for_an_unresolvable_offset() {
    let text = "^ = () -> Num => < 1 + 1 >\n";
    let checked = check_text(Path::new("buffer.qn"), text).expect("checks clean");
    assert!(references_at(&checked.program, text, offset_of(text, "+ 1", 0)).is_none());
}

#[test]
fn references_answer_nothing_for_a_name_declared_in_another_file() {
    let directory = temporary_directory("import_references");
    let module_text = ">> add = (a :: Num, b :: Num) -> Num => < a + b >\n";
    std::fs::write(directory.join("lib.qn"), module_text).expect("write module");

    let text = "<< \"lib.qn\"\n\n^ = () -> Num => < lib.add(1, 2) >\n";
    let root = directory.join("buffer.qn");
    let checked = check_text(&root, text).expect("checks clean");

    assert!(references_at(&checked.program, text, offset_of(text, "lib.add", 4)).is_none());

    std::fs::remove_dir_all(&directory).ok();
}

// --- Rename -------------------------------------------------------------------

#[test]
fn only_a_bare_name_is_accepted_as_a_rename_target() {
    assert!(is_identifier("newName"));
    assert!(is_identifier("_private"));
    assert!(!is_identifier("1abc"));
    assert!(!is_identifier("a.b"));
    assert!(!is_identifier("two names"));
    assert!(!is_identifier(""));
}

// --- Semantic tokens --------------------------------------------------------

/// The classified kind of the token spanning `needle` (plus `into` bytes) in `text`.
fn kind_at(text: &str, needle: &str, into: u32) -> SemanticTokenKind {
    let offset = offset_of(text, needle, into);
    semantic_tokens(text)
        .into_iter()
        .find(|token| token.span.start <= offset && offset < token.span.end)
        .unwrap_or_else(|| panic!("no classified token at byte {offset}"))
        .kind
}

#[test]
fn block_delimiters_and_comparisons_are_told_apart() {
    let text = "max = (a :: Num, b :: Num) -> Num => < a > b ? a : b >\n";

    // The `<` opening the body and the `>` closing it are block delimiters.
    assert_eq!(kind_at(text, "< a", 0), SemanticTokenKind::BlockDelimiter);
    assert_eq!(kind_at(text, "b >\n", 2), SemanticTokenKind::BlockDelimiter);
    // The `>` between two operands is the comparison.
    assert_eq!(
        kind_at(text, "a > b", 2),
        SemanticTokenKind::ComparisonOperator
    );

    // A `<` after a completed operand is less-than, not a block opener.
    let text = "below = (a :: Num, b :: Num) -> Bool => < a < b >\n";
    assert_eq!(kind_at(text, "< a", 0), SemanticTokenKind::BlockDelimiter);
    assert_eq!(
        kind_at(text, "a < b", 2),
        SemanticTokenKind::ComparisonOperator
    );
}

#[test]
fn declared_names_classify_as_types_functions_and_parameters() {
    let text = "Point = { x :: Num }\nshift = (amount :: Num) -> Num => < amount + 1 >\n^ = () -> Num => < shift(2) >\n";
    assert_eq!(kind_at(text, "Point", 0), SemanticTokenKind::TypeName);
    assert_eq!(
        kind_at(text, "shift(2)", 0),
        SemanticTokenKind::FunctionName
    );
    assert_eq!(
        kind_at(text, "amount + 1", 0),
        SemanticTokenKind::ParameterName
    );
}

#[test]
fn unparseable_text_still_classifies_delimiters() {
    // A parse error leaves the name sets empty, but the token-level `<`/`>`
    // classification still answers.
    let text = "f = () -> Num => < 1 +\n";
    assert_eq!(kind_at(text, "<", 0), SemanticTokenKind::BlockDelimiter);
}

// --- Test lenses ------------------------------------------------------------

#[test]
fn test_lenses_locate_every_suite_and_case() {
    let text = "<< core.test\n\ntest.describe(\"outer\", () => <\n  test.it(\"first\", () => <\n    expect(1, equals(1))\n  >)\n  test.describe(\"inner\", () => <\n    test.it(\"second\", () => <\n      expect(2, equals(2))\n    >)\n  >)\n>)\n";
    let lenses = test_lenses(text);
    let summary: Vec<(TestLensKind, &str)> = lenses
        .iter()
        .map(|lens| (lens.kind, lens.name.as_str()))
        .collect();
    assert_eq!(
        summary,
        vec![
            (TestLensKind::Suite, "outer"),
            (TestLensKind::Case, "first"),
            (TestLensKind::Suite, "inner"),
            (TestLensKind::Case, "second"),
        ]
    );
    assert_eq!(
        lenses[0].span.start,
        offset_of(text, "test.describe(\"outer\"", 0)
    );
    assert_eq!(
        lenses[1].span.start,
        offset_of(text, "test.it(\"first\"", 0)
    );
}

#[test]
fn test_lens_paths_join_by_slash_through_the_nesting_matching_only() {
    // The `path` a lens carries is exactly what `quilon test --only` expects: the names
    // from the outermost `describe` down, joined by `/`.
    let text = "<< core.test\n\ntest.describe(\"outer\", () => <\n  test.it(\"first\", () => <\n    expect(1, equals(1))\n  >)\n  test.describe(\"inner\", () => <\n    test.it(\"second\", () => <\n      expect(2, equals(2))\n    >)\n  >)\n>)\n";
    let paths: Vec<String> = test_lenses(text).into_iter().map(|l| l.path).collect();
    assert_eq!(
        paths,
        vec!["outer", "outer/first", "outer/inner", "outer/inner/second"]
    );
}

#[test]
fn a_program_without_test_blocks_has_no_lenses() {
    assert!(test_lenses("^ = () -> Num => < 0 >\n").is_empty());
    assert!(test_lenses("not even quilon (((").is_empty());
}

// --- The protocol loop, end to end ------------------------------------------
//
// Shared helpers below, then: one full session over an in-memory connection (initialize,
// open a document with a type error, read the published diagnostic, fix the document, read
// the all-clear, ask for hover, semantic tokens, code lenses, and the testItems request,
// then shut down), and a focused session asserting the structured-diagnostic mapping.
//
// Shared by every test below that drives the server as a real LSP client would, rather
// than calling `analysis` directly: a request/notification pair, receiving the next
// message with a timeout, and unwrapping a response or a publishDiagnostics notification.

fn lsp_request(id: i32, method: &str, params: serde_json::Value) -> lsp_server::Message {
    lsp_server::Message::Request(lsp_server::Request {
        id: id.into(),
        method: method.to_string(),
        params,
    })
}

fn lsp_notification(method: &str, params: serde_json::Value) -> lsp_server::Message {
    lsp_server::Message::Notification(lsp_server::Notification {
        method: method.to_string(),
        params,
    })
}

fn lsp_receive(client: &lsp_server::Connection) -> lsp_server::Message {
    client
        .receiver
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the server answers within the timeout")
}

fn lsp_response(message: lsp_server::Message) -> serde_json::Value {
    match message {
        lsp_server::Message::Response(response) => response
            .response_result
            .unwrap_or_else(|error| panic!("error response: {error:?}")),
        other => panic!("expected a response, got {other:?}"),
    }
}

fn lsp_diagnostics_of(message: lsp_server::Message) -> Vec<serde_json::Value> {
    match message {
        lsp_server::Message::Notification(published) => {
            assert_eq!(published.method, "textDocument/publishDiagnostics");
            published.params["diagnostics"]
                .as_array()
                .expect("an array")
                .clone()
        }
        other => panic!("expected publishDiagnostics, got {other:?}"),
    }
}

/// A fresh server, initialized and ready for requests/notifications, plus the join handle
/// to await once the caller shuts it down.
fn started_session() -> (lsp_server::Connection, std::thread::JoinHandle<()>) {
    let (client, server) = lsp_server::Connection::memory();
    let served =
        std::thread::spawn(move || quilon::lsp::serve(server).expect("server exits cleanly"));
    client
        .sender
        .send(lsp_request(
            0,
            "initialize",
            serde_json::json!({ "capabilities": {} }),
        ))
        .unwrap();
    let initialized = lsp_response(lsp_receive(&client));
    assert!(
        initialized["capabilities"]["hoverProvider"]
            .as_bool()
            .unwrap_or(false)
    );
    assert_eq!(
        initialized["capabilities"]["completionProvider"]["triggerCharacters"],
        serde_json::json!(["."])
    );
    client
        .sender
        .send(lsp_notification("initialized", serde_json::json!({})))
        .unwrap();
    (client, served)
}

#[test]
fn a_protocol_session_answers_over_an_in_memory_connection() {
    use serde_json::{Value, json};

    let (client, served) = started_session();

    // Open a document with a type error: one diagnostic, on the offending line.
    let uri = "file:///buffer.qn";
    let broken = "^ = () -> Num => < 1 + true >\n";
    client
        .sender
        .send(lsp_notification(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": uri, "languageId": "quilon", "version": 1, "text": broken } }),
        ))
        .unwrap();
    let diagnostics = lsp_diagnostics_of(lsp_receive(&client));
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["range"]["start"]["line"], 0);
    assert_eq!(
        diagnostics[0]["range"]["start"]["character"],
        broken.find("1 + true").unwrap()
    );

    // Fix it: the diagnostics clear.
    let fixed = "double = (x :: Num) -> Num => < x * 2 >\n^ = () -> Num => < double(4) >\n";
    client
        .sender
        .send(lsp_notification(
            "textDocument/didChange",
            json!({ "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [ { "text": fixed } ] }),
        ))
        .unwrap();
    assert!(lsp_diagnostics_of(lsp_receive(&client)).is_empty());

    // Hover over the `x` in the body: its inferred type.
    client
        .sender
        .send(lsp_request(
            2,
            "textDocument/hover",
            json!({ "textDocument": { "uri": uri },
                "position": { "line": 0, "character": fixed.find("x * 2").unwrap() } }),
        ))
        .unwrap();
    let hover = lsp_response(lsp_receive(&client));
    assert_eq!(hover["contents"]["value"], "Num");

    // Semantic tokens exist (the delimiter/operator classification at least).
    client
        .sender
        .send(lsp_request(
            3,
            "textDocument/semanticTokens/full",
            json!({ "textDocument": { "uri": uri } }),
        ))
        .unwrap();
    let tokens = lsp_response(lsp_receive(&client));
    assert!(!tokens["data"].as_array().expect("token data").is_empty());

    // A test suite document gets a code lens per suite and case, carrying the
    // client-side run command.
    let suite_uri = "file:///suite.qn";
    let suite = "<< core.test\n\ntest.describe(\"s\", () => <\n  test.it(\"c\", () => <\n    expect(1, equals(1))\n  >)\n>)\n";
    client
        .sender
        .send(lsp_notification(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": suite_uri, "languageId": "quilon", "version": 1, "text": suite } }),
        ))
        .unwrap();
    lsp_diagnostics_of(lsp_receive(&client));
    client
        .sender
        .send(lsp_request(
            4,
            "textDocument/codeLens",
            json!({ "textDocument": { "uri": suite_uri } }),
        ))
        .unwrap();
    let lenses = lsp_response(lsp_receive(&client));
    let lenses = lenses.as_array().expect("a lens array");
    // Each suite/case gets a Run lens and a Debug lens, in that order.
    assert_eq!(lenses.len(), 4);
    assert_eq!(lenses[0]["command"]["command"], "quilon.runTests");
    assert_eq!(lenses[0]["command"]["title"], "▶ Run suite");
    assert_eq!(lenses[1]["command"]["command"], "quilon.debugTests");
    assert_eq!(lenses[1]["command"]["title"], "🐞 Debug suite");
    assert_eq!(lenses[2]["command"]["command"], "quilon.runTests");
    assert_eq!(lenses[2]["command"]["title"], "▶ Run case");
    assert_eq!(lenses[3]["command"]["command"], "quilon.debugTests");
    assert_eq!(lenses[3]["command"]["title"], "🐞 Debug case");
    // Each lens's second argument is its own `/`-joined path, so the client can run or
    // debug just that suite or case with `--only`.
    assert_eq!(lenses[0]["command"]["arguments"][1], "s");
    assert_eq!(lenses[1]["command"]["arguments"][1], "s");
    assert_eq!(lenses[2]["command"]["arguments"][1], "s/c");
    assert_eq!(lenses[3]["command"]["arguments"][1], "s/c");

    // The custom `quilon/testItems` request answers the same test tree flat, each entry
    // carrying the `/`-joined path `quilon test --only` expects.
    client
        .sender
        .send(lsp_request(
            5,
            "quilon/testItems",
            json!({ "textDocument": { "uri": suite_uri } }),
        ))
        .unwrap();
    let items = lsp_response(lsp_receive(&client));
    let items = items.as_array().expect("a test item array");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["kind"], "suite");
    assert_eq!(items[0]["name"], "s");
    assert_eq!(items[0]["path"], "s");
    assert_eq!(items[1]["kind"], "case");
    assert_eq!(items[1]["name"], "c");
    assert_eq!(items[1]["path"], "s/c");

    // References on the parameter `x`: its own declaration and its one use.
    client
        .sender
        .send(lsp_request(
            7,
            "textDocument/references",
            json!({ "textDocument": { "uri": uri },
                "position": { "line": 0, "character": fixed.find("x * 2").unwrap() },
                "context": { "includeDeclaration": true } }),
        ))
        .unwrap();
    let locations = lsp_response(lsp_receive(&client));
    let locations = locations.as_array().expect("a location array");
    assert_eq!(locations.len(), 2);
    let mut starts: Vec<u32> = locations
        .iter()
        .map(|location| location["range"]["start"]["character"].as_u64().unwrap() as u32)
        .collect();
    starts.sort();
    assert_eq!(
        starts,
        vec![
            fixed.find("x :: Num").unwrap() as u32,
            fixed.find("x * 2").unwrap() as u32,
        ]
    );

    // Renaming `x` rewrites both the declaration and its use, in the same document.
    client
        .sender
        .send(lsp_request(
            8,
            "textDocument/rename",
            json!({ "textDocument": { "uri": uri },
                "position": { "line": 0, "character": fixed.find("x * 2").unwrap() },
                "newName": "renamed" }),
        ))
        .unwrap();
    let edit = lsp_response(lsp_receive(&client));
    let edits = edit["changes"][uri]
        .as_array()
        .expect("edits for the document");
    assert_eq!(edits.len(), 2);
    assert!(edits.iter().all(|edit| edit["newText"] == "renamed"));

    // Renaming to something that is not a bare identifier is refused.
    client
        .sender
        .send(lsp_request(
            9,
            "textDocument/rename",
            json!({ "textDocument": { "uri": uri },
                "position": { "line": 0, "character": fixed.find("x * 2").unwrap() },
                "newName": "1abc" }),
        ))
        .unwrap();
    match lsp_receive(&client) {
        lsp_server::Message::Response(response) => {
            let error = response.response_result.expect_err("an error response");
            assert!(
                error.message.contains("1abc"),
                "unexpected message: {error:?}"
            );
        }
        other => panic!("expected a response, got {other:?}"),
    }

    client
        .sender
        .send(lsp_request(6, "shutdown", Value::Null))
        .unwrap();
    lsp_response(lsp_receive(&client));
    client
        .sender
        .send(lsp_notification("exit", Value::Null))
        .unwrap();
    served.join().expect("the server thread joins");
}

/// A rename request on a name reached through an import cannot rewrite that name's
/// declaration — it lives in a document this session never opened — so it answers an
/// error naming the file where it actually is.
#[test]
fn rename_on_an_imported_name_answers_an_error_naming_its_file() {
    use serde_json::{Value, json};

    let directory = temporary_directory("import_rename");
    let module_text = ">> add = (a :: Num, b :: Num) -> Num => < a + b >\n";
    std::fs::write(directory.join("lib.qn"), module_text).expect("write module");

    let text = "<< \"lib.qn\"\n\n^ = () -> Num => < lib.add(1, 2) >\n";
    let root = directory.join("buffer.qn");
    let uri = format!("file://{}", root.display());

    let (client, served) = started_session();
    client
        .sender
        .send(lsp_notification(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": uri, "languageId": "quilon", "version": 1, "text": text } }),
        ))
        .unwrap();
    lsp_diagnostics_of(lsp_receive(&client));

    let call_line = text
        .lines()
        .position(|line| line.contains("lib.add"))
        .expect("the call is on some line");
    let character = text
        .lines()
        .nth(call_line)
        .unwrap()
        .find("lib.add")
        .unwrap() as u32
        + 4;
    client
        .sender
        .send(lsp_request(
            1,
            "textDocument/rename",
            json!({ "textDocument": { "uri": uri },
                "position": { "line": call_line, "character": character },
                "newName": "sum" }),
        ))
        .unwrap();
    match lsp_receive(&client) {
        lsp_server::Message::Response(response) => {
            let error = response.response_result.expect_err("an error response");
            assert!(
                error.message.contains("lib.qn"),
                "unexpected message: {error:?}"
            );
        }
        other => panic!("expected a response, got {other:?}"),
    }

    client
        .sender
        .send(lsp_request(2, "shutdown", Value::Null))
        .unwrap();
    lsp_response(lsp_receive(&client));
    client
        .sender
        .send(lsp_notification("exit", Value::Null))
        .unwrap();
    served.join().expect("the server thread joins");

    std::fs::remove_dir_all(&directory).ok();
}

/// A `Num + Text` overload mismatch — `Code::NoMatchingOverload` (`QN311`) — carries the
/// code in `Diagnostic.code`, its own message (with the operator's dedicated help text
/// appended) as the message, and each operand's type as a separate `relatedInformation`
/// entry at that operand's own location.
#[test]
fn a_num_plus_text_diagnostic_carries_its_code_and_related_operand_labels() {
    use serde_json::json;

    let (client, served) = started_session();

    let uri = "file:///overload.qn";
    let text = "^ = () -> Num => < 1 + \"x\" >\n";
    client
        .sender
        .send(lsp_notification(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": uri, "languageId": "quilon", "version": 1, "text": text } }),
        ))
        .unwrap();
    let diagnostics = lsp_diagnostics_of(lsp_receive(&client));
    assert_eq!(diagnostics.len(), 1);
    let diagnostic = &diagnostics[0];

    assert_eq!(diagnostic["code"], "QN311");
    assert!(
        diagnostic["message"]
            .as_str()
            .unwrap()
            .contains("no overload of `+`"),
        "unexpected message: {diagnostic}"
    );
    // The operator's dedicated help for exactly this mismatch, appended to the message.
    assert!(
        diagnostic["message"]
            .as_str()
            .unwrap()
            .contains("interpolate"),
        "unexpected message: {diagnostic}"
    );
    // The range opens at the primary label — the left (`Num`) operand.
    assert_eq!(
        diagnostic["range"]["start"]["character"],
        text.find('1').unwrap()
    );

    // The right (`Text`) operand is the one remaining label, as related information at its
    // own location in this same document.
    let related = diagnostic["relatedInformation"]
        .as_array()
        .expect("related info");
    assert_eq!(related.len(), 1);
    assert_eq!(related[0]["message"], "Text");
    assert_eq!(related[0]["location"]["uri"], uri);
    assert_eq!(
        related[0]["location"]["range"]["start"]["character"],
        text.find("\"x\"").unwrap()
    );

    client
        .sender
        .send(lsp_request(1, "shutdown", serde_json::Value::Null))
        .unwrap();
    lsp_response(lsp_receive(&client));
    client
        .sender
        .send(lsp_notification("exit", serde_json::Value::Null))
        .unwrap();
    served.join().expect("the server thread joins");
}

// --- Completion ---------------------------------------------------------------
//
// Every case below inserts the marker `HERE` right where the cursor sits — a bare word
// (case 1) or the start of a member name (cases 2/3) actually being typed, so `HERE`'s
// own end is the byte offset a client's completion request would carry, and the
// document with `HERE` in it is exactly the (unparseable, mid-edit) buffer
// `completions_at` has to work from.

/// Case 1: locals and parameters of the enclosing scopes, top-level functions/types
/// defined above the cursor (never the enclosing `^` itself), a sum type's constructors,
/// and an import's binding.
#[test]
fn completions_in_scope_list_locals_top_level_names_sum_constructors_and_imports() {
    let text = "double = (x :: Num) -> Num => < x * 2 >\n\
Color = Red / Green / Blue\n\
<< core.io\n\n\
^ = () -> Num => <\n  y = 3\n  HERE\n  0\n>\n";
    let offset = offset_of(text, "HERE", 4);
    let items = completions_at(Path::new("buffer.qn"), text, offset);
    let labels: std::collections::HashSet<&str> =
        items.iter().map(|item| item.label.as_str()).collect();

    assert!(labels.contains("y"), "a local above the cursor: {labels:?}");
    assert!(
        labels.contains("double"),
        "a top-level function above the cursor: {labels:?}"
    );
    assert!(
        labels.contains("Color"),
        "a top-level type above the cursor: {labels:?}"
    );
    assert!(labels.contains("Red") && labels.contains("Green") && labels.contains("Blue"));
    assert!(
        labels.contains("io"),
        "the import's own binding: {labels:?}"
    );
    assert!(
        !labels.contains("^"),
        "the enclosing definition is excluded from its own body: {labels:?}"
    );

    let y = items.iter().find(|item| item.label == "y").unwrap();
    assert_eq!(y.kind, CompletionKind::Variable);
    let double = items.iter().find(|item| item.label == "double").unwrap();
    assert_eq!(double.kind, CompletionKind::Function);
    assert_eq!(double.detail.as_deref(), Some("(Num) -> Num"));
    let red = items.iter().find(|item| item.label == "Red").unwrap();
    assert_eq!(red.kind, CompletionKind::EnumMember);
    let io = items.iter().find(|item| item.label == "io").unwrap();
    assert_eq!(io.kind, CompletionKind::Module);
}

/// Case 2: after a bare import binding (`http.`), the module's own exports — including a
/// sum's variants, reached the same qualified way `http.Get` is written in code.
#[test]
fn completions_after_an_import_binding_list_the_modules_exports() {
    let text = "<< core.http\n\n^ = () -> Num => <\n  http.HERE\n  0\n>\n";
    let offset = offset_of(text, "HERE", 4);
    let items = completions_at(Path::new("buffer.qn"), text, offset);
    let labels: std::collections::HashSet<&str> =
        items.iter().map(|item| item.label.as_str()).collect();

    for expected in [
        "Body", "Method", "Response", "Request", "Get", "Post", "Delete",
    ] {
        assert!(
            labels.contains(expected),
            "missing `{expected}`: {labels:?}"
        );
    }

    let response = items.iter().find(|item| item.label == "Response").unwrap();
    assert_eq!(response.kind, CompletionKind::Class);
    let get = items.iter().find(|item| item.label == "Get").unwrap();
    assert_eq!(get.kind, CompletionKind::EnumMember);
    assert_eq!(get.detail.as_deref(), Some("Method"));
}

/// Case 3, a user record: after `.` on an expression of a user-declared record type, its
/// fields and methods — never the record's own name (that would be case 1).
#[test]
fn completions_after_an_expression_list_the_receivers_record_members() {
    let text = "Point = {\n  x :: Num,\n  y :: Num,\n\n  sum = () -> Num => < it.x + it.y >\n}\n\n\
^ = () -> Num => <\n  p = Point { x = 1, y = 2 }\n  p.HERE\n  0\n>\n";
    let offset = offset_of(text, "HERE", 4);
    let items = completions_at(Path::new("buffer.qn"), text, offset);
    let labels: std::collections::HashSet<&str> =
        items.iter().map(|item| item.label.as_str()).collect();
    assert_eq!(labels, std::collections::HashSet::from(["x", "y", "sum"]));

    let x = items.iter().find(|item| item.label == "x").unwrap();
    assert_eq!(x.kind, CompletionKind::Field);
    assert_eq!(x.detail.as_deref(), Some("Num"));
    let sum = items.iter().find(|item| item.label == "sum").unwrap();
    assert_eq!(sum.kind, CompletionKind::Method);
    assert_eq!(sum.detail.as_deref(), Some("() -> Num"));
}

/// Case 3, `Text`: the native primitives, the composable `corelib/text.qn` methods, and
/// the `size`/`length` fields — all reserved on a `Text` receiver.
#[test]
fn completions_after_a_text_expression_list_its_built_in_members() {
    let text = "^ = () -> Num => <\n  s = \"hi\"\n  s.HERE\n  0\n>\n";
    let offset = offset_of(text, "HERE", 4);
    let items = completions_at(Path::new("buffer.qn"), text, offset);
    let labels: std::collections::HashSet<&str> =
        items.iter().map(|item| item.label.as_str()).collect();
    for expected in ["trim", "split", "contains", "size", "length"] {
        assert!(
            labels.contains(expected),
            "missing `{expected}`: {labels:?}"
        );
    }
    let split = items.iter().find(|item| item.label == "split").unwrap();
    assert_eq!(split.kind, CompletionKind::Method);
    assert_eq!(split.detail.as_deref(), Some("(Text) -> []Text"));
    let size = items.iter().find(|item| item.label == "size").unwrap();
    assert_eq!(size.kind, CompletionKind::Field);
    assert_eq!(size.detail.as_deref(), Some("Num"));
}

/// Case 3 on a corelib record reached through an import: `http.Response`'s own fields and
/// methods, exactly as any other record's.
#[test]
fn completions_on_a_corelib_response_list_its_fields_and_methods() {
    let text = "<< core.http\n\n^ = () -> Num => <\n  \
response = http.Response { raw = \"HTTP/1.1 200 OK\\r\\n\\r\\n\" }\n  response.HERE\n  0\n>\n";
    let offset = offset_of(text, "HERE", 4);
    let items = completions_at(Path::new("buffer.qn"), text, offset);
    let labels: std::collections::HashSet<&str> =
        items.iter().map(|item| item.label.as_str()).collect();
    for expected in ["raw", "status", "header", "body"] {
        assert!(
            labels.contains(expected),
            "missing `{expected}`: {labels:?}"
        );
    }
    let raw = items.iter().find(|item| item.label == "raw").unwrap();
    assert_eq!(raw.kind, CompletionKind::Field);
    let header = items.iter().find(|item| item.label == "header").unwrap();
    assert_eq!(header.kind, CompletionKind::Method);
    assert_eq!(header.detail.as_deref(), Some("(Text) -> Result"));
}

/// A `textDocument/completion` request over the protocol, on a document that could never
/// check clean AS SENT (`response.` has no member yet): the server still answers the
/// receiver's members.
#[test]
fn completion_over_the_protocol_lists_an_expressions_members() {
    use serde_json::{Value, json};

    let (client, served) = started_session();

    let uri = "file:///completion.qn";
    let text = "<< core.http\n\n^ = () -> Num => <\n  \
response = http.Response { raw = \"HTTP/1.1 200 OK\\r\\n\\r\\n\" }\n  response.\n  0\n>\n";
    client
        .sender
        .send(lsp_notification(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": uri, "languageId": "quilon", "version": 1, "text": text } }),
        ))
        .unwrap();
    lsp_diagnostics_of(lsp_receive(&client));

    let line = text
        .lines()
        .position(|line| line.contains("response."))
        .expect("the member access is on some line");
    let character = text.lines().nth(line).unwrap().find("response.").unwrap() as u32
        + "response.".len() as u32;
    client
        .sender
        .send(lsp_request(
            1,
            "textDocument/completion",
            json!({ "textDocument": { "uri": uri },
                "position": { "line": line, "character": character } }),
        ))
        .unwrap();
    let completions = lsp_response(lsp_receive(&client));
    let items = completions.as_array().expect("a completion array");
    let labels: Vec<&str> = items
        .iter()
        .map(|item| item["label"].as_str().unwrap())
        .collect();
    assert!(labels.contains(&"status"), "unexpected labels: {labels:?}");
    assert!(labels.contains(&"raw"), "unexpected labels: {labels:?}");
    let status = items.iter().find(|item| item["label"] == "status").unwrap();
    assert_eq!(status["kind"], 2, "2 is the protocol's Method kind");

    client
        .sender
        .send(lsp_request(2, "shutdown", Value::Null))
        .unwrap();
    lsp_response(lsp_receive(&client));
    client
        .sender
        .send(lsp_notification("exit", Value::Null))
        .unwrap();
    served.join().expect("the server thread joins");
}

/// Case 3 when the receiver sits inside a larger expression that does not itself
/// type-check with the receiver's bare type (`1 + s` is `Num + Text`, a real mismatch —
/// exactly what typing `.someMethod()` would have fixed): completion still isolates the
/// receiver from the surrounding `1 +` and answers `Text`'s members.
#[test]
fn completions_on_a_receiver_inside_a_mismatched_expression_still_resolve() {
    let text = "^ = () -> Num => <\n  s = \"hi\"\n  n = 1 + s.HERE\n  0\n>\n";
    let offset = offset_of(text, "HERE", 4);
    let items = completions_at(Path::new("buffer.qn"), text, offset);
    let labels: std::collections::HashSet<&str> =
        items.iter().map(|item| item.label.as_str()).collect();
    assert!(labels.contains("trim"), "unexpected labels: {labels:?}");
    assert!(labels.contains("size"), "unexpected labels: {labels:?}");
}

/// The same isolation, with the receiver as a call argument (`double(s.)` — `s` is `Text`,
/// `double` wants a `Num`).
#[test]
fn completions_on_a_receiver_inside_a_mismatched_call_argument_still_resolve() {
    let text = "double = (x :: Num) -> Num => < x * 2 >\n\
^ = () -> Num => <\n  s = \"hi\"\n  double(s.HERE)\n  0\n>\n";
    let offset = offset_of(text, "HERE", 4);
    let items = completions_at(Path::new("buffer.qn"), text, offset);
    let labels: std::collections::HashSet<&str> =
        items.iter().map(|item| item.label.as_str()).collect();
    assert!(labels.contains("trim"), "unexpected labels: {labels:?}");
}

/// Case 3 on a type declared LOCALLY inside a block (not just at the top level): its
/// methods are offered, not just its fields.
#[test]
fn completions_on_a_block_local_records_receiver_list_its_methods_too() {
    let text = "^ = () -> Num => <\n  \
Point = { x :: Num, sum = () -> Num => < it.x > }\n  \
p = Point { x = 1 }\n  p.HERE\n  0\n>\n";
    let offset = offset_of(text, "HERE", 4);
    let items = completions_at(Path::new("buffer.qn"), text, offset);
    let labels: std::collections::HashSet<&str> =
        items.iter().map(|item| item.label.as_str()).collect();
    assert_eq!(labels, std::collections::HashSet::from(["x", "sum"]));
}

// --- The parse helper -------------------------------------------------------

#[test]
fn parse_text_answers_none_for_broken_text() {
    assert!(analysis::parse_text("^ = () -> Num => < 0 >\n").is_some());
    assert!(analysis::parse_text("((((").is_none());
}
