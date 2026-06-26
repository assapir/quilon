#!/usr/bin/env bash
# Ahead-of-time compile a Quilon program to a native executable.
#
#   scripts/aot.sh path/to/prog.ql [output-binary]
#
# Pipeline: `quilon compile` (Quilon -> LLVM IR) -> `llc` (IR -> object) ->
# `clang` (link against libquilon_rt + Boehm GC -> native binary). The runtime
# intrinsics (__alloc/__gc_init/__write_bytes/__text_length/...) live in the
# `quilon-rt` static library; libgc must be installed (libgc-dev / gc).
#
# Requires: a built `quilon` + `libquilon_rt.a`, plus `llc` and `clang` on PATH
# (`gcc` also works as the linker if you prefer).
set -euo pipefail

SRC="${1:?usage: scripts/aot.sh <prog.ql> [output-binary]}"
OUT="${2:-${SRC%.ql}}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RTDIR="$ROOT/target/debug"

# Build the compiler and the runtime static lib (libquilon_rt.a).
cargo build --manifest-path "$ROOT/Cargo.toml" -p quilon -p quilon-rt >/dev/null

LL="$(mktemp --suffix=.ll)"
OBJ="$(mktemp --suffix=.o)"
trap 'rm -f "$LL" "$OBJ"' EXIT

"$RTDIR/quilon" compile "$SRC" -o "$LL" >/dev/null
# PIC so the string/data relocations link into a (default) PIE binary.
llc -relocation-model=pic -filetype=obj "$LL" -o "$OBJ"
# Link the object against the runtime static lib, Boehm GC, and the system libs
# the Rust staticlib needs. clang is the natural linker for LLVM-produced objects.
clang "$OBJ" -L "$RTDIR" -lquilon_rt -lgc -lpthread -ldl -lm -o "$OUT"

echo "built native binary: $OUT"
