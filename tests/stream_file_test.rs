//! End-to-end proof of `@streamFile` — the chunk-callback streaming file read (`core.io`),
//! which runs on the calling fiber, parking between reads. Each case here drives it through
//! the in-process JIT against a real temp file and reads the exit code it computed, the same
//! pattern `run_test.rs` uses for plain arithmetic.

mod common;
use common::assert_exit;

/// Write `bytes` to a fresh temp file and return the [`tempfile::NamedTempFile`] owning
/// it — kept bound for as long as the program reading it (via `assert_exit`, which runs
/// and returns synchronously) needs it to exist.
fn temp_data_file(tag: &str, bytes: &[u8]) -> tempfile::NamedTempFile {
    let file = tempfile::Builder::new()
        .prefix(&format!("quilon_streamfile_{tag}_"))
        .tempfile()
        .expect("create temp data file");
    std::fs::write(file.path(), bytes).expect("write temp data file");
    file
}

#[test]
fn a_small_file_with_a_large_chunk_size_yields_ok_with_the_file_size() {
    // A chunkSize far bigger than the file: the first read already gets every byte, but the
    // last grapheme still holds back until a second read confirms end-of-file (one extra
    // syscall a regular file answers at once) — two chunks, not one, and the full text either
    // way.
    let file = temp_data_file("whole", b"hello world");
    let src = format!(
        r#"<< core.io

^ = () -> Num => <
  seen := ""
  count := 0
  result = io.@streamFile("{path}", 1000, chunk => <
    seen := seen + chunk
    count := count + 1
    true
  >)
  result ?
    | Ok(bytesRead) => (seen == "hello world" ? 1 : 0) + (bytesRead == 11 ? 2 : 0) + (count == 2 ? 4 : 0)
    | NotOk(_) => 0
>
"#,
        path = file.path().display(),
    );
    assert_exit(&src, 7);
}

#[test]
fn on_chunk_returning_false_stops_after_the_first_chunk() {
    // chunkSize 3 against a 10-byte file: the first read is not known to be the file's
    // last (more remains on disk), so the trailing grapheme of what it read is held back
    // for a possible merge — the delivered chunk is "ab" (2 bytes), not the full "abc".
    let file = temp_data_file("stop", b"abcdefghij");
    let src = format!(
        r#"<< core.io

^ = () -> Num => <
  seen := ""
  count := 0
  result = io.@streamFile("{path}", 3, chunk => <
    seen := seen + chunk
    count := count + 1
    false
  >)
  result ?
    | Ok(bytesRead) => (seen == "ab" ? 1 : 0) + (bytesRead == 2 ? 2 : 0) + (count == 1 ? 4 : 0)
    | NotOk(_) => 0
>
"#,
        path = file.path().display(),
    );
    assert_exit(&src, 7);
}

#[test]
fn a_missing_file_yields_not_ok() {
    // A path that must not exist: reserve a fresh temp dir and name a file inside it that
    // is never created.
    let dir = tempfile::Builder::new()
        .prefix("quilon_streamfile_missing_")
        .tempdir()
        .expect("create temp dir");
    let path = dir.path().join("never_written");
    let src = format!(
        r#"<< core.io

^ = () -> Num => <
  io.@streamFile("{path}", 10, chunk => < true >) ?
    | Ok(_) => 0
    | NotOk(_) => 1
>
"#,
        path = path.display(),
    );
    assert_exit(&src, 1);
}

#[test]
fn a_non_positive_chunk_size_yields_not_ok() {
    let file = temp_data_file("chunksize", b"x");
    for chunk_size in ["0", "-5"] {
        let src = format!(
            r#"<< core.io

^ = () -> Num => <
  io.@streamFile("{path}", {chunk_size}, chunk => < true >) ?
    | Ok(_) => 0
    | NotOk(_) => 1
>
"#,
            path = file.path().display(),
        );
        assert_exit(&src, 1);
    }
}

#[test]
fn a_chunk_size_too_large_to_allocate_yields_not_ok() {
    // 1e20 bytes (spelled as a plain decimal — Quilon's lexer rejects `1e20` itself, see
    // QN005) is not a real allocation any machine grants: this must fail soft (`NotOk`),
    // never abort the process the way an infallible `Vec` allocation would.
    let file = temp_data_file("huge_chunksize", b"x");
    let src = format!(
        r#"<< core.io

^ = () -> Num => <
  io.@streamFile("{path}", 100000000000000000000, chunk => < true >) ?
    | Ok(_) => 0
    | NotOk(_) => 1
>
"#,
        path = file.path().display(),
    );
    assert_exit(&src, 1);
}

#[test]
fn a_file_ending_inside_a_utf_8_sequence_yields_not_ok() {
    // The file's last byte (0xC3) is a lone lead byte of a 2-byte sequence with no
    // continuation byte ever coming: at true EOF that tail is genuinely invalid UTF-8, not
    // merely incomplete, so it must never reach `onChunk` as a chunk.
    let file = temp_data_file("truncated_utf8", b"caf\xc3");
    let src = format!(
        r#"<< core.io

^ = () -> Num => <
  io.@streamFile("{path}", 100, chunk => < true >) ?
    | Ok(_) => 0
    | NotOk(_) => 1
>
"#,
        path = file.path().display(),
    );
    assert_exit(&src, 1);
}

#[test]
fn a_chunk_edge_inside_a_multi_byte_code_point_never_splits_it() {
    // "e" (precomposed U+00E9, a 2-byte UTF-8 sequence) between plain ASCII: a 2-byte
    // chunkSize forces a read boundary to land inside its bytes on more than one read.
    let e_acute = '\u{00e9}';
    let content = format!("ab{e_acute}cd");
    let file = temp_data_file("multibyte", content.as_bytes());
    let src = format!(
        r#"<< core.io

^ = () -> Num => <
  seen := ""
  count := 0
  result = io.@streamFile("{path}", 2, chunk => <
    seen := seen + chunk
    count := count + 1
    true
  >)
  result ?
    | Ok(bytesRead) => (seen == "ab{e_acute}cd" ? 1 : 0) + (bytesRead == 6 ? 2 : 0) + (count == 4 ? 4 : 0)
    | NotOk(_) => 0
>
"#,
        path = file.path().display(),
    );
    assert_exit(&src, 7);
}

#[test]
fn a_chunk_edge_inside_a_grapheme_cluster_never_splits_it() {
    // "e" + a combining acute accent (U+0301) is ONE grapheme cluster over 3 bytes; a
    // 2-byte chunkSize forces the two pieces apart across different reads.
    let combining_e = format!("e{}", '\u{0301}');
    let content = format!("x{combining_e}y");
    let file = temp_data_file("grapheme", content.as_bytes());
    let src = format!(
        r#"<< core.io

^ = () -> Num => <
  seen := ""
  count := 0
  result = io.@streamFile("{path}", 2, chunk => <
    seen := seen + chunk
    count := count + 1
    true
  >)
  result ?
    | Ok(bytesRead) => (seen == "x{combining_e}y" ? 1 : 0) + (bytesRead == 5 ? 2 : 0) + (count == 3 ? 4 : 0) + (seen.length == 3 ? 8 : 0)
    | NotOk(_) => 0
>
"#,
        path = file.path().display(),
    );
    assert_exit(&src, 15);
}

#[test]
fn io_stream_file_is_streamfile_with_a_default_chunk_size() {
    let file = temp_data_file("default", b"the quick fox");
    let src = format!(
        r#"<< core.io

^ = () -> Num => <
  seen := ""
  result = io.streamFile("{path}", chunk => <
    seen := seen + chunk
    true
  >)
  result ?
    | Ok(bytesRead) => (seen == "the quick fox" ? 1 : 0) + (bytesRead == 13 ? 2 : 0)
    | NotOk(_) => 0
>
"#,
        path = file.path().display(),
    );
    assert_exit(&src, 3);
}
