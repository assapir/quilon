//! End-to-end proof of `@streamFile` — the chunk-callback streaming file read (`core.io`).
//!
//! `@streamFile(path, chunkSize, onChunk)` runs STRICTLY, in program order on the calling
//! fiber (unlike `@readStdin`/`@tcpRequest`'s launch-and-defer): each case here drives it
//! through the in-process JIT against a real temp file and reads the exit code it computed,
//! the same pattern `run_test.rs` uses for plain arithmetic.

mod common;
use common::assert_exit;

/// Write `bytes` to a fresh temp file and return its path.
fn temp_data_file(tag: &str, bytes: &[u8]) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "quilon_streamfile_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, bytes).expect("write temp data file");
    path
}

#[test]
fn whole_small_file_in_one_chunk_yields_ok_with_the_file_size() {
    let file = temp_data_file("whole", b"hello world");
    let src = format!(
        r#"<< core.io

^ = () -> Num => <
  seen := ""
  count := 0
  result = @streamFile("{path}", 1000, chunk => <
    seen := seen + chunk
    count := count + 1
    true
  >)
  result ?
    | Ok(bytesRead) => (seen == "hello world" ? 1 : 0) + (bytesRead == 11 ? 2 : 0) + (count == 1 ? 4 : 0)
    | NotOk(_) => 0
>
"#,
        path = file.display(),
    );
    assert_exit(&src, 7);
    let _ = std::fs::remove_file(&file);
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
  result = @streamFile("{path}", 3, chunk => <
    seen := seen + chunk
    count := count + 1
    false
  >)
  result ?
    | Ok(bytesRead) => (seen == "ab" ? 1 : 0) + (bytesRead == 2 ? 2 : 0) + (count == 1 ? 4 : 0)
    | NotOk(_) => 0
>
"#,
        path = file.display(),
    );
    assert_exit(&src, 7);
    let _ = std::fs::remove_file(&file);
}

#[test]
fn a_missing_file_yields_not_ok() {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "quilon_streamfile_missing_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let src = format!(
        r#"<< core.io

^ = () -> Num => <
  @streamFile("{path}", 10, chunk => < true >) ?
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
  @streamFile("{path}", {chunk_size}, chunk => < true >) ?
    | Ok(_) => 0
    | NotOk(_) => 1
>
"#,
            path = file.display(),
        );
        assert_exit(&src, 1);
    }
    let _ = std::fs::remove_file(&file);
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
  result = @streamFile("{path}", 2, chunk => <
    seen := seen + chunk
    count := count + 1
    true
  >)
  result ?
    | Ok(bytesRead) => (seen == "ab{e_acute}cd" ? 1 : 0) + (bytesRead == 6 ? 2 : 0) + (count == 3 ? 4 : 0)
    | NotOk(_) => 0
>
"#,
        path = file.display(),
    );
    assert_exit(&src, 7);
    let _ = std::fs::remove_file(&file);
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
  result = @streamFile("{path}", 2, chunk => <
    seen := seen + chunk
    count := count + 1
    true
  >)
  result ?
    | Ok(bytesRead) => (seen == "x{combining_e}y" ? 1 : 0) + (bytesRead == 5 ? 2 : 0) + (count == 2 ? 4 : 0) + (seen.length == 3 ? 8 : 0)
    | NotOk(_) => 0
>
"#,
        path = file.display(),
    );
    assert_exit(&src, 15);
    let _ = std::fs::remove_file(&file);
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
        path = file.display(),
    );
    assert_exit(&src, 3);
    let _ = std::fs::remove_file(&file);
}
