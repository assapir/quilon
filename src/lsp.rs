//! `quilon lsp` — the Quilon language server.
//!
//! A synchronous server over stdio (the `lsp-server` crate): one loop, one request at a
//! time, and one full front-end run — the same lex → parse → link → check pipeline every
//! other subcommand uses — behind each answer. There is no incremental state: the open
//! documents' text is the only thing the server holds, so every answer reflects the
//! buffer as the editor last sent it, saved or not.
//!
//! Capabilities: publish diagnostics on open/change, go-to-definition, hover (the
//! expression's inferred type), semantic tokens (block `< >` delimiters versus comparison
//! operators, plus declared type/function/parameter names), and a code lens on every test
//! suite and case. The lens carries the client-side `quilon.runTests` command with the
//! block's own `/`-joined path; running is the editor's job. The custom `quilon/testItems`
//! request answers the same test tree as a flat list, each entry carrying that same path,
//! for a client building a test explorer rather than a lens.
//!
//! The protocol speaks UTF-16 line/column positions; every span crosses that boundary
//! through [`DocumentPositions`], the shared byte-offset translation in `source_map`.

pub mod analysis;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::{
    CodeLens, CodeLensOptions, Command, Diagnostic, DiagnosticSeverity,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    GotoDefinitionParams, Hover, HoverContents, HoverParams, HoverProviderCapability, Location,
    MarkedString, OneOf, Position, PublishDiagnosticsParams, Range, SemanticToken,
    SemanticTokenType, SemanticTokens, SemanticTokensFullOptions, SemanticTokensLegend,
    SemanticTokensOptions, SemanticTokensParams, SemanticTokensServerCapabilities,
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
};

use crate::lexer::{ROOT_FILE, Span};
use crate::source_map::DocumentPositions;
use analysis::{SemanticTokenKind, TestLensKind};

/// The semantic token types the server reports, in legend order. Block delimiters are
/// keywords (structural, like a brace — not operators, which is the point of the
/// distinction) and comparisons are operators; the name classifications use the matching
/// standard types.
const SEMANTIC_TOKEN_LEGEND: [SemanticTokenType; 5] = [
    SemanticTokenType::KEYWORD,
    SemanticTokenType::OPERATOR,
    SemanticTokenType::TYPE,
    SemanticTokenType::FUNCTION,
    SemanticTokenType::PARAMETER,
];

/// The index into [`SEMANTIC_TOKEN_LEGEND`] for one classified token.
fn legend_index(kind: SemanticTokenKind) -> u32 {
    match kind {
        SemanticTokenKind::BlockDelimiter => 0,
        SemanticTokenKind::ComparisonOperator => 1,
        SemanticTokenKind::TypeName => 2,
        SemanticTokenKind::FunctionName => 3,
        SemanticTokenKind::ParameterName => 4,
    }
}

/// The client-side command a test code lens invokes, with the document's path as its one
/// argument. The Visual Studio Code extension contributes it; any other client may too.
const RUN_TESTS_COMMAND: &str = "quilon.runTests";

/// Serve the Language Server Protocol over stdio until the client shuts the session down.
pub fn run() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    let (connection, io_threads) = Connection::stdio();
    serve(connection)?;
    io_threads.join()?;
    Ok(())
}

/// The server proper, over any connection — what `run` serves over stdio, and what a test
/// drives over an in-memory pair.
pub fn serve(connection: Connection) -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    let capabilities = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        definition_provider: Some(OneOf::Left(true)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        code_lens_provider: Some(CodeLensOptions {
            resolve_provider: Some(false),
        }),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                legend: SemanticTokensLegend {
                    token_types: SEMANTIC_TOKEN_LEGEND.to_vec(),
                    token_modifiers: Vec::new(),
                },
                full: Some(SemanticTokensFullOptions::Bool(true)),
                ..Default::default()
            },
        )),
        ..Default::default()
    };
    connection.initialize(serde_json::to_value(capabilities)?)?;

    let mut server = LanguageServer {
        documents: HashMap::new(),
    };
    for message in &connection.receiver {
        match message {
            Message::Request(request) => {
                if connection.handle_shutdown(&request)? {
                    break;
                }
                let response = server.answer(request);
                connection.sender.send(Message::Response(response))?;
            }
            Message::Notification(notification) => {
                for published in server.accept(notification) {
                    connection.sender.send(Message::Notification(published))?;
                }
            }
            Message::Response(_) => {}
        }
    }
    Ok(())
}

/// The server's whole state: the text of every open document, keyed by its URI. The
/// editor's buffer — not the disk — is the source of truth for these.
struct LanguageServer {
    documents: HashMap<Uri, String>,
}

impl LanguageServer {
    // --- Notifications (document lifecycle) --------------------------------

    /// Apply a client notification; the returned notifications (diagnostics) go back.
    fn accept(&mut self, notification: Notification) -> Vec<Notification> {
        match notification.method.as_str() {
            "textDocument/didOpen" => {
                let Ok(params) =
                    serde_json::from_value::<DidOpenTextDocumentParams>(notification.params)
                else {
                    return Vec::new();
                };
                let uri = params.text_document.uri;
                self.documents
                    .insert(uri.clone(), params.text_document.text);
                vec![self.diagnostics_for(&uri)]
            }
            "textDocument/didChange" => {
                let Ok(params) =
                    serde_json::from_value::<DidChangeTextDocumentParams>(notification.params)
                else {
                    return Vec::new();
                };
                let uri = params.text_document.uri;
                // Full sync: the last change carries the whole new text.
                if let Some(change) = params.content_changes.into_iter().last() {
                    self.documents.insert(uri.clone(), change.text);
                }
                vec![self.diagnostics_for(&uri)]
            }
            "textDocument/didClose" => {
                let Ok(params) =
                    serde_json::from_value::<DidCloseTextDocumentParams>(notification.params)
                else {
                    return Vec::new();
                };
                self.documents.remove(&params.text_document.uri);
                vec![publish(params.text_document.uri, Vec::new())]
            }
            _ => Vec::new(),
        }
    }

    /// The publishDiagnostics notification for the document at `uri`, from a fresh
    /// front-end run: empty when the document checks clean.
    fn diagnostics_for(&self, uri: &Uri) -> Notification {
        let Some((path, text)) = self.document(uri) else {
            return publish(uri.clone(), Vec::new());
        };
        let diagnostics = match analysis::check_text(&path, text) {
            Ok(_) => Vec::new(),
            Err(error) => {
                let positions = DocumentPositions::new(text);
                let (range, message) = match error.span() {
                    // An error in the open document points at its own span.
                    Some(span) if span.file == ROOT_FILE => {
                        (range_of(&positions, span), error.message().to_string())
                    }
                    // An error elsewhere (an imported module, or no location at all)
                    // lands at the top of the document, carrying the rendered position.
                    _ => {
                        let location = error.to_string().lines().next().unwrap_or("").to_string();
                        let message = match location.is_empty() {
                            true => error.message().to_string(),
                            false => format!("{} {}", location, error.message()),
                        };
                        (Range::default(), message)
                    }
                };
                vec![Diagnostic {
                    range,
                    severity: Some(DiagnosticSeverity::ERROR),
                    source: Some("quilon".to_string()),
                    message,
                    ..Default::default()
                }]
            }
        };
        publish(uri.clone(), diagnostics)
    }

    // --- Requests -----------------------------------------------------------

    fn answer(&self, request: Request) -> Response {
        let id = request.id.clone();
        match request.method.as_str() {
            "textDocument/hover" => match serde_json::from_value(request.params) {
                Ok(params) => self.hover(id, params),
                Err(error) => invalid_params(id, error),
            },
            "textDocument/definition" => match serde_json::from_value(request.params) {
                Ok(params) => self.definition(id, params),
                Err(error) => invalid_params(id, error),
            },
            "textDocument/semanticTokens/full" => match serde_json::from_value(request.params) {
                Ok(params) => self.semantic_tokens(id, params),
                Err(error) => invalid_params(id, error),
            },
            "textDocument/codeLens" => match serde_json::from_value(request.params) {
                Ok(params) => self.code_lenses(id, params),
                Err(error) => invalid_params(id, error),
            },
            "quilon/testItems" => match request_uri(&request.params) {
                Some(uri) => self.test_items(id, &uri),
                None => Response::new_err(
                    id,
                    lsp_server::ErrorCode::InvalidParams as i32,
                    "expected { textDocument: { uri } }".to_string(),
                ),
            },
            _ => Response::new_err(
                id,
                lsp_server::ErrorCode::MethodNotFound as i32,
                format!("unhandled method {}", request.method),
            ),
        }
    }

    fn hover(&self, id: RequestId, params: HoverParams) -> Response {
        let position_params = params.text_document_position_params;
        let Some((path, text)) = self.document(&position_params.text_document.uri) else {
            return Response::new_ok(id, serde_json::Value::Null);
        };
        let positions = DocumentPositions::new(text);
        let offset = positions.byte_offset(
            position_params.position.line,
            position_params.position.character,
        ) as u32;
        let Ok(checked) = analysis::check_text(&path, text) else {
            return Response::new_ok(id, serde_json::Value::Null);
        };
        match analysis::hover_at(&checked.types, offset) {
            Some((label, span)) => Response::new_ok(
                id,
                Hover {
                    contents: HoverContents::Scalar(MarkedString::from_language_code(
                        "quilon".to_string(),
                        label,
                    )),
                    range: Some(range_of(&positions, &span)),
                },
            ),
            None => Response::new_ok(id, serde_json::Value::Null),
        }
    }

    fn definition(&self, id: RequestId, params: GotoDefinitionParams) -> Response {
        let position_params = params.text_document_position_params;
        let uri = position_params.text_document.uri;
        let Some((path, text)) = self.document(&uri) else {
            return Response::new_ok(id, serde_json::Value::Null);
        };
        let positions = DocumentPositions::new(text);
        let offset = positions.byte_offset(
            position_params.position.line,
            position_params.position.character,
        ) as u32;
        let Ok(checked) = analysis::check_text(&path, text) else {
            return Response::new_ok(id, serde_json::Value::Null);
        };
        let Some(span) = analysis::definition_at(&checked.program, offset) else {
            return Response::new_ok(id, serde_json::Value::Null);
        };

        let location = match span.file == ROOT_FILE {
            true => Some(Location {
                uri: uri.clone(),
                range: range_of(&positions, &span),
            }),
            false => location_in_other_file(&checked, &span),
        };
        match location {
            Some(location) => Response::new_ok(id, location),
            None => Response::new_ok(id, serde_json::Value::Null),
        }
    }

    fn semantic_tokens(&self, id: RequestId, params: SemanticTokensParams) -> Response {
        let Some((_, text)) = self.document(&params.text_document.uri) else {
            return Response::new_ok(id, serde_json::Value::Null);
        };
        let positions = DocumentPositions::new(text);
        let mut data = Vec::new();
        let (mut previous_line, mut previous_start) = (0u32, 0u32);
        for token in analysis::semantic_tokens(text) {
            let (line, start) = positions.position_utf16(token.span.start as usize);
            let (end_line, end) = positions.position_utf16(token.span.end as usize);
            if end_line != line {
                continue; // No multi-line tokens in the classification.
            }
            data.push(SemanticToken {
                delta_line: line - previous_line,
                delta_start: match line == previous_line {
                    true => start - previous_start,
                    false => start,
                },
                length: end - start,
                token_type: legend_index(token.kind),
                token_modifiers_bitset: 0,
            });
            (previous_line, previous_start) = (line, start);
        }
        Response::new_ok(
            id,
            SemanticTokens {
                result_id: None,
                data,
            },
        )
    }

    fn code_lenses(&self, id: RequestId, params: lsp_types::CodeLensParams) -> Response {
        let uri = params.text_document.uri;
        let Some((path, text)) = self.document(&uri) else {
            return Response::new_ok(id, serde_json::Value::Null);
        };
        let positions = DocumentPositions::new(text);
        let lenses: Vec<CodeLens> = analysis::test_lenses(text)
            .into_iter()
            .map(|lens| {
                let title = match lens.kind {
                    TestLensKind::Suite => "▶ Run suite",
                    TestLensKind::Case => "▶ Run case",
                };
                CodeLens {
                    range: range_of(&positions, &lens.span),
                    command: Some(Command {
                        title: title.to_string(),
                        command: RUN_TESTS_COMMAND.to_string(),
                        // The file, then the lens's own `/`-joined path — the client passes
                        // the second as `--only`, so a suite lens runs just that suite and
                        // a case lens just that case, rather than the whole file.
                        arguments: Some(vec![
                            serde_json::Value::String(path.display().to_string()),
                            serde_json::Value::String(lens.path),
                        ]),
                    }),
                    data: None,
                }
            })
            .collect();
        Response::new_ok(id, lenses)
    }

    /// The document's whole test tree, flat: one entry per suite and case, in document
    /// order, each carrying the `/`-joined path `quilon test --only` expects. Answers the
    /// custom `quilon/testItems` request — a client building a test explorer reads this
    /// instead of the `describe`/`it` lenses `textDocument/codeLens` carries.
    fn test_items(&self, id: RequestId, uri: &Uri) -> Response {
        let Some((_, text)) = self.document(uri) else {
            return Response::new_ok(id, Vec::<serde_json::Value>::new());
        };
        let positions = DocumentPositions::new(text);
        let items: Vec<serde_json::Value> = analysis::test_lenses(text)
            .into_iter()
            .map(|lens| {
                let kind = match lens.kind {
                    TestLensKind::Suite => "suite",
                    TestLensKind::Case => "case",
                };
                serde_json::json!({
                    "path": lens.path,
                    "name": lens.name,
                    "kind": kind,
                    "range": range_of(&positions, &lens.span),
                })
            })
            .collect();
        Response::new_ok(id, items)
    }

    // --- Shared helpers -----------------------------------------------------

    /// The open document's filesystem path and buffer text — `None` for a document the
    /// client has not opened, or one that is not a file.
    fn document(&self, uri: &Uri) -> Option<(PathBuf, &str)> {
        let path = file_path(uri)?;
        let text = self.documents.get(uri)?;
        Some((path, text.as_str()))
    }
}

/// The `textDocument.uri` out of a request's raw params — `{ textDocument: { uri } }`,
/// the shape every request naming one document (but nothing else) shares. `quilon/testItems`
/// is the only caller; it needs no dedicated params type for that one field.
fn request_uri(params: &serde_json::Value) -> Option<Uri> {
    params
        .get("textDocument")?
        .get("uri")?
        .as_str()?
        .parse()
        .ok()
}

/// The filesystem path a `file:` URI names, percent-decoded. `None` for any other scheme.
fn file_path(uri: &Uri) -> Option<PathBuf> {
    if uri.scheme()?.as_str() != "file" {
        return None;
    }
    let decoded = uri.path().as_estr().decode().into_string().ok()?;
    Some(PathBuf::from(decoded.into_owned()))
}

/// The `file:` URI naming `path`, with every byte outside the unreserved set and `/`
/// percent-encoded.
fn file_uri(path: &Path) -> Option<Uri> {
    let mut encoded = String::from("file://");
    for byte in path.to_str()?.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded.parse().ok()
}

/// A protocol `Location` for a span in one of the program's OTHER files (an imported
/// module). `None` when that file is not a real file on disk — a bundled built-in module
/// has no file to open.
fn location_in_other_file(checked: &crate::driver::Checked, span: &Span) -> Option<Location> {
    let resolved = checked.sources.locate(span)?;
    let path = std::fs::canonicalize(&resolved.path).ok()?;
    let uri = file_uri(&path)?;
    let text = checked.sources.get_text(span.file)?;
    let positions = DocumentPositions::new(text);
    Some(Location {
        uri,
        range: range_of(&positions, span),
    })
}

/// The protocol range of `span` within the document `positions` indexes.
fn range_of(positions: &DocumentPositions, span: &Span) -> Range {
    let (start_line, start_character) = positions.position_utf16(span.start as usize);
    let (end_line, end_character) = positions.position_utf16(span.end as usize);
    Range {
        start: Position::new(start_line, start_character),
        end: Position::new(end_line, end_character),
    }
}

fn publish(uri: Uri, diagnostics: Vec<Diagnostic>) -> Notification {
    Notification {
        method: "textDocument/publishDiagnostics".to_string(),
        params: serde_json::to_value(PublishDiagnosticsParams {
            uri,
            diagnostics,
            version: None,
        })
        .unwrap_or_default(),
    }
}

fn invalid_params(id: RequestId, error: serde_json::Error) -> Response {
    Response::new_err(
        id,
        lsp_server::ErrorCode::InvalidParams as i32,
        error.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uris_round_trip_through_percent_encoding() {
        let path = Path::new("/tmp/my project/héllo.qn");
        let uri = file_uri(path).expect("a file uri");
        assert_eq!(uri.as_str(), "file:///tmp/my%20project/h%C3%A9llo.qn");
        assert_eq!(file_path(&uri).as_deref(), Some(path));
    }

    #[test]
    fn a_non_file_uri_names_no_path() {
        let uri: Uri = "untitled:Untitled-1".parse().unwrap();
        assert_eq!(file_path(&uri), None);
    }
}
