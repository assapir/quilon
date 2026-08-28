# Changelog

All notable changes to Quilon are documented here.

## Unreleased

### Added

- **A lambda takes its parameter types from the signature that receives it.** Where a
  lambda lands on a known function type, that type says what its parameters are:

  ```quilon ignore
  apply = (x :: Num, f :: (Num) -> Num) -> Num => f(x)

  apply(10, (n) => n + 1)                ~ `f`'s type already says `n` is a Num
  10 |> apply((n) => n + 1)              ~ the pipe injects the first argument
  c.applyTo((n) => n * 2)                ~ a method's function-typed parameter
  scale :: (Num) -> Num = (n) => n * 4   ~ a binding that declares its function type
  ```

  An annotation stays legal and wins where written. Where no target type is known — a
  lambda in a plain expression, or an overload set the other arguments do not narrow to one
  member — the parameters must still be annotated, and the error says which is missing
  rather than assuming `Num`. An overload set may now also dispatch on a function-typed
  parameter. See `docs/functions/README.md`.

- **`core.http` — an HTTP client written in Quilon** over `core.net`'s `@tcpRequest`. HTTP
  only, no TLS. It exports four type names and no free functions, since every exported name is
  a word an importer can no longer use:

  ```quilon ignore
  << core.http

  ^ = () -> $ => <
    reply = Request { method = Get, url = "http://example.com/" }.send() ?
      | Ok(response) => response
      | NotOk(_)     => Response { raw = "" }
    assert(reply.status(), equals(200))
  >
  ```

  A reply is wrapped and checked in one step — `Response { raw = text }.validate()` — then
  read through `status()` / `statusLine()` / `header(name)` / `headers()` / `body()`. Replies
  are read leniently (HTTP/1.0 or 1.1, CRLF or bare LF) and the close, not `Content-Length`,
  delimits the body. See `docs/corelib/http.md`.

### Changed

- **`print` takes anything renderable: the per-type overload set is gone.** `print`,
  `eprint` and `write` no longer carry a member per built-in type. At a call site the
  compiler resolves the `` ` `` render member on the argument's type, calls it, and writes
  the resulting `Text` — the path string interpolation already took:

  ```quilon
  Money = {
    amount :: Num, currency :: Text,
    ` = () -> Text => "`it.amount` `it.currency`"
  }

  price = Money { amount = 12, currency = "EUR" }
  print(price)                ~ 12 EUR — no corelib involvement
  ```

  A type becomes printable by defining that member, never by extending `print`. `write`
  renders too, so it is no longer limited to `Text` (a `Text` renders as itself, so its
  bytes still go out as they are). A type with no member of its own keeps the default
  rendering for its shape — a record shows its type name, a sum its variant. A **function**
  value is the one thing that does not render, and the error now names the missing member
  rather than listing overload candidates.

  The compiler claims these three names at their own arity, so a definition there is
  rejected and points at the render member; another arity is still an ordinary overload set
  beside the built-in. An existing user `print` member migrates to a `` ` `` member on the
  type it printed. See `docs/corelib/io.md` and `examples/printing.qn`.

- **`>` closes a block by default; it is greater-than only when an operand follows it.**
  A block-bodied lambda now fits inside a call on one line:

  ```quilon
  xs.each(x => <
    total := total + x
  >)
  ```

  Previously `>` closed a block only as the last token on its line, so the closer above
  had to dangle on a line of its own with the `)` beneath it. A `>` is now the operator
  only where a comparison can be written — when the next token is on the same line and can
  begin an operand (identifier, literal, `(`, `[`, `{`, prefix `-`/`!`) — and closes before
  a `)`, `]`, `}`, `,`, a `~` comment, or the end of the line. `a > b`, `f(x > y)`,
  `a > -b` and `"b" > "a"` are unaffected, as are `>=` and the `>>` export marker.

  Two consequences. A trailing comment no longer changes a `>`: `a > ~why` followed by the
  operand on the next line was a comparison and is now a block close, so a comparison's
  right operand must share its line. And two adjacent closers now need a space (`> >`),
  because `>>` is the export marker — a diagnostic names that fix.

- **BREAKING: assertions take a matcher, and the old forms are gone.** An assertion is now
  the value under test first and a matcher second, with two entry points over one
  vocabulary:

  ```quilon
  assert(2 + 2, equals(4))              ~ fatal: report at the call site and exit 101
  expect(body, contains("HTTP/1.1"))    ~ recorded: mark the case failed, carry on
  expect(status, not(equals(500)))
  expect(response, isOk())
  ```

  `assert(cond)`, `assert(cond, opts)`, `AssertOpts`, `assertEq`, `assertNotEq`, `assertOk`
  and `assertNotOk` are **removed**. Migration is mechanical: `assertEq(a, b)` becomes
  `assert(a, equals(b))`, `assertNotEq(a, b)` becomes `assert(a, not(equals(b)))`,
  `assertOk(r)` becomes `assert(r, isOk())`, and `assert(a == b)` becomes
  `assert(a, equals(b))`. A custom failure message has no replacement — the matcher's own
  report names what was expected and what was found.

  The matchers are `equals`, `contains`, `not`, `isOk` and `isNotOk`. `equals` compares
  through the `==` member and renders through `` ` ``, so a user record or sum works exactly
  as far as its own members do; `contains` reads a `Text` or an array; `not` wraps any
  matcher. A matcher applied to a type it cannot read is a compile error naming the missing
  member. Comparisons (`greaterThan`, …) come later.

  `assert` and the matchers are **compiler-provided**, like `print` — no `<< core.test`, and
  a program reaches them with no import. That is what lets one matcher name work over every
  type while the language has no generics: a matcher holds a value of the type under test,
  which otherwise needs a matcher type per type. `core.test` keeps `failAt`, for building a
  check of your own.

- **A test run reports every case, and tallies them.** `expect` records its failure instead
  of ending the process: the first failing `expect` in a case skips what is left of that case
  — the assertions after it never run, so their subjects are never evaluated — and the suite
  carries on with the next case. A run therefore prints `N passed, M failed`, marks each case
  `✓` or `✗`, and exits non-zero when any case failed.

  `expect` outside an `it` case is a compile error pointing at `assert`: outside a `describe`
  block there is no run to record with (the blocks are stripped from
  `run`/`compile`/`build`), and inside one but outside a case there is nothing to mark, so
  the failure would print and never be counted. `assert` inside a case stays fatal, for a
  precondition the case cannot continue past.

- **`core.test` exports only what a suite and the test entry point call.** `describe`, `it`
  and `reportSummary` ship in `core.test` beside `failAt`, the run's recorded state
  (`casesPassed`, `casesFailed`, `nestingDepth`) and the case lifecycle (`enterSuite`,
  `leaveSuite`, `caseFailing`, `finishCase`). The report's colors and its per-group and
  per-case lines are written out inside those three rather than behind exported helpers, so
  `indent`, `green`, `red`, `reportSuite` and `reportCase` are names a program is free to
  define: imports are transitive, and an export nothing outside the module calls is a name
  taken from every importer for nothing. What the report looks like is fixed for now.

  A suite with no harness in scope is a compile error at its first `describe`, naming the
  import that fixes it.

- **BREAKING: a setter is now declared with `:=`, and `=` methods are verified
  non-mutating ([#198](https://github.com/assapir/quilon/issues/198)).** A method that
  mutates its receiver is written `name := (…) => …`; one written `name = (…) => …`
  promises not to, and the checker holds it to that — writing `it.field := …` in an `=`
  method, or calling a `:=` sibling on `it`, is now a compile error naming the fix
  (`Method 'T.bump' mutates 'it' but is declared with '='; declare it with ':='`).

  The binding operator now means the same thing for a method as it does for a variable or
  a record binding, and a method's right to mutate becomes part of its signature. Before,
  setter-ness was *inferred* from the body, so adding a cache write to a getter silently
  reclassified it and broke every `=`-receiver call site with no visible change to the
  method's shape. Migration is mechanical: re-declare each mutating method with `:=`. Only
  `examples/mutation.qn` needed it in this repository; corelib had none.

  `:=` is confined to where it is enforced. The receiver-mutability gate lives in the
  method-call path, which operator, render and hash members never reach, and a sum has no
  field to write, so `+ := …`, `` ` := … `` and a `:=` sum method are rejected rather than
  accepted as promises nothing checks.

  Calling a setter still requires a `:=` receiver — unchanged, and that rule is what the
  contract exists to serve. What is gone is the inference that decided which methods were
  setters, along with the fixpoint it needed: every sibling's contract is now known from
  its declaration.

- **BREAKING: `recv.name(...)` looks for `name` on the receiver's type and nowhere else
  ([#265](https://github.com/assapir/quilon/issues/265)).** A name the type does not have is
  a compile error naming both (`'Counter' has no member 'bump'`); a top-level function of
  that name no longer answers the call.

  What breaks: `(5).double()` is an error where it used to call `double = (x :: Num) …`.
  Write `double(5)`, or pipe it (`5 |> double()`) — both still find the function. Where
  there is one, the error spells that call out for you.

### Fixed

- **A method's receiver no longer keeps unreachable code alive.** Reachability collects
  names mentioned without resolving them, and every method body mentions `it`, the receiver
  — read as a top-level mention, that kept `core.test`'s `it` function and the whole
  harness chain behind it (`finishCase`, `caseFailing`, `enterSuite`, `leaveSuite`) in any
  program that declared a type with a method. A bare `it` is no longer
  a mention; an `it` in callee position — the callee of a call, or the right side of a `|>`,
  which desugars to one — still is, since that is where it can name a top-level function.

  With that, an import the erased `describe` blocks were the only user of reaches no build
  on its own: `check`, `compile`, `build` and `run` erase the blocks and with them every
  reference the harness had, and a function nothing reaches is not emitted. So `<< core.test`
  beside a program's own code costs that program's build nothing, and needs no marker.

- **`print` writes the whole `Text`, and renders it deliberately
  ([#220](https://github.com/assapir/quilon/issues/220)).** `print`/`eprint` took only a
  pointer, so output stopped at the first NUL byte: a `Text` read from stdin as
  `a<NUL>b` printed just `a`, while `write` of the same value emitted all three bytes.
  Output now takes the `Text`'s length like `write` does, so the value reaches the
  descriptor whole, and the difference between the two is deliberate rather than accidental:
  `print` renders for a reader (an invalid UTF-8 byte shows as `�`), `write` is byte-exact —
  the rule `docs/LANGUAGE.md` now carries. With length carried through, a `Text` is exactly
  its bytes: nothing appends a terminator past them, so no future producer of one can
  forget to.

- **Type confusions in codegen, reachable if the checker ever loosened
  ([#220](https://github.com/assapir/quilon/issues/220)).** Comparing two composite values
  chose `Text` comparison from their LLVM *shape*, so an array (or closure, or sum) would
  have been read as a `Text`'s pointer and length; and a sum's payload slots were laid out
  by two rules that disagreed, either of which could size a slot below the payload stored in
  it. Both now follow one rule, keyed on the type checker's own answer.

- **`quilon build` on macOS: a comma in the runtime archive's path no longer mangles the
  link.** The flag that force-loads `libquilon_rt.a` was passed as `-Wl,-force_load,<path>`,
  which the compiler driver splits on commas — so a cache path containing one (it follows
  `XDG_CACHE_HOME`/`HOME`, and a home directory may have a comma in it) reached ld64 as two
  broken flags. The flag and the path are now separate `-Xlinker` arguments, which nothing
  comma-parses.

- **Method checking: an immutability bypass and an unchecked call site.** Two soundness
  holes in how methods were checked, both of which let a broken program past the checker.

  A method that mutated `it` from **inside a lambda** — `steps.each(s => it.value := s)` —
  was not classified a setter, because the walk that looks for the write had no case for a
  lambda (nor for array, record, index, field-access or spread forms). An unclassified
  setter stays callable on an `=`-bound receiver, which it then mutates: the headline
  immutability promise, breakable in four lines. The same was true of a write inside a
  function *declared* in the body. That walk is now the **verifier** behind the declared
  contract above, rather than a classifier: it is a flat per-node predicate
  over the AST's shared structural walk — one traversal that every analysis uses, exhaustive
  with no catch-all arm, so a new expression form cannot silently reopen the hole: it fails
  to compile until it is classified. (That walk moved from `deferral.rs` to `src/ast/walk.rs`
  to be reachable from the checker.) The transitive rule (a method that calls another setter
  is a setter) composes on top as before, so a setter reached only through a lambda-writing
  sibling is caught too.

  Separately, a method parameter with **no type annotation** defaults to `Num` when the body
  is checked, but the call site skipped those arguments entirely. `t.add("hello")` on
  `add = (x) => it.v + x` passed the checker and then died in codegen, printing a raw LLVM
  verifier dump at the user. Call sites now hold arguments to the same `Num` the body was
  checked against — which is what a plain function's unannotated parameter already did, so
  this makes methods consistent rather than introducing a rule.

### Changed

- **The release publishes a binary per platform, under a new name each
  ([#49](https://github.com/assapir/quilon/issues/49)).** A release used to carry one asset,
  named `quilon`, built on Ubuntu. It now carries two, each named for what it runs on —
  which **breaks any script or link that fetched the old bare `quilon` asset**:

  | Platform | Asset |
  | --- | --- |
  | Linux, x86_64 (glibc) | `quilon-x86_64-unknown-linux-gnu` |
  | macOS, Apple silicon | `quilon-aarch64-apple-darwin` |

  Intel Macs are not covered: `macos-latest` is Apple silicon, and the asset is arm64 only.

  Both are self-contained — LLVM is linked into them, as the collector already was, so they
  run on a machine that has neither. That took work on macOS, where Homebrew's `llvm@22`
  ships no static LLVM at all (it is built `LLVM_LINK_LLVM_DYLIB=ON`, and a binary linked
  against it starts only where that formula is installed): the job links the static archives
  from the upstream LLVM release package instead.

  There is no separate Arch asset, because there is nothing for it to do. The Linux asset is
  built against an older glibc than a rolling distro carries and glibc runs binaries built
  against older versions of itself, so it is the portable one everywhere — Arch included,
  where it was checked by hand. A native Arch build would be the opposite: Arch's `llvm`
  ships only a shared `libLLVM.so`, and the upstream static package cannot be linked there
  either (it names `/usr/lib/x86_64-linux-gnu/libzstd.a` by absolute path, a Debian layout,
  and Arch ships no static `zstd` — the same gap that made the collector a vendored
  submodule), so it would have named `libLLVM.so.22.1` and needed LLVM 22 installed. CI
  still builds and tests on Arch.

  Neither claim is asserted on trust: each job runs `ldd`/`otool -L` on the binary it built
  and type-checks a program with it, publishes that output to the job summary, and fails the
  release if an LLVM — or on macOS anything outside the system libraries — is named there.
  Neither job is allowed to fail quietly either, so a release carries both assets or none.

  A manual run of the workflow builds both and stops before publishing, so the matrix can be
  exercised without spending a version number.

- **`quilon build` works on macOS, and CI covers macOS and Arch Linux
  ([#49](https://github.com/assapir/quilon/issues/49)).** The AOT link line was GNU-ld
  shaped and Apple's ld64 rejects it: `--whole-archive` is not a flag it knows, and there is
  no `-ldl` to link against (libSystem provides `dlopen`). `src/build.rs` now picks the
  right spelling per platform — `-Wl,-force_load,<archive>` on macOS, the
  `--whole-archive`/`--no-whole-archive` bracket elsewhere — so forcing every runtime object
  in stays deterministic on both.

  Two new jobs build and test on every run, both non-blocking until they have proven
  themselves stable: `macos-latest` (Homebrew `llvm@22`), and Arch Linux in an
  `archlinux:latest` container — the distro whose lack of a static `libgc.a` is why the
  collector is vendored at all, and the maintainer's own environment. `fmt`/`clippy` are
  platform-independent and keep gating on Ubuntu only, as do the benchmarks so the series
  compares like with like.

  Windows remains unreachable, and the blocker is the runtime rather than the GC —
  `quilon-rt` does not compile for `x86_64-pc-windows-msvc` (stdin readiness via
  `mio::unix::SourceFd`, `fcntl`/`sysconf`, and the collector's pthread threading model).
  Tracked with the full error list in
  [#183](https://github.com/assapir/quilon/issues/183) rather than shipped as a red job.

- **`quilon build` binaries are self-contained: the Boehm GC is linked statically
  ([#49](https://github.com/assapir/quilon/issues/49)).** A compiled Quilon program used to
  name `libgc.so` among its dynamic dependencies, so shipping it anywhere meant shipping —
  or installing — libgc too. The collector now comes from a pinned `quilon-rt/vendor/bdwgc`
  submodule (bdwgc 8.2.12), compiled by `quilon-rt`'s build script into a single object and
  linked statically. Because rustc bundles a static native library into a staticlib, it
  travels inside `libquilon_rt.a` — the archive the compiler already embeds and
  cache-extracts — so the AOT link needed nothing new beyond dropping `-lgc`. A produced
  binary now runs on a machine with no libgc installed, gated by
  `tests/build_command_test.rs`, which asserts the product names no shared `libgc`.

  libgc stops being a dependency anywhere: the `quilon` binary and `quilon run`'s in-process
  JIT carry the same statically linked collector, so `libgc-dev` is gone from both workflows
  and CI's green run is itself the proof that nothing needs installing. Clone with
  `--recurse-submodules` (or `git submodule update --init`) — the build stops with exactly
  that instruction when the submodule is absent, rather than a wall of compiler errors.

  `THIRD-PARTY-NOTICES.md` carries bdwgc's copyright and permission notice verbatim: the
  collector's sources arrive as a submodule, so a source archive of this repository would
  otherwise ship none of its notice text.

  The collector is built with upstream's configure defaults for a threaded POSIX build, so
  its behaviour is unchanged; `ALL_INTERIOR_POINTERS` is load bearing, since Quilon's
  `Text`/array values are `{ ptr, len }` pairs whose pointer may be interior. Costs, measured
  on x86_64: a compiled `hello_world` grows 202 KB (+3.1%) and the `quilon` binary 245 KB
  (+2.8%). Runtime is flat to faster — `gc_churn` -6.0%, `text_loop` -7.1% — with peak RSS
  unchanged.

### Fixed

- **Debugging works from a GUI-launched editor, and from any directory
  ([#200](https://github.com/assapir/quilon/issues/200)).** Two independent reasons a debug
  session could not start:

  The VS Code extension ran a bare `quilon`, so it only worked when the editor's process had
  inherited a `PATH` containing it. An editor started from a desktop launcher usually has
  not — `~/.cargo/bin` is added by a shell rc file — so an installed compiler still produced
  `debug build failed: could not run "quilon"`. When `quilon.command` is left at its default
  the extension now looks for the compiler itself: `PATH`, then the usual install directories
  (`~/.cargo/bin`, `~/.local/bin`, `/usr/local/bin`, `/opt/homebrew/bin`), then a
  `target/release`|`debug` build in an open folder, then `cargo run --quiet --` when an open
  folder is a checkout of this repository. An explicitly configured `quilon.command` is still
  used verbatim (bar a bare `"quilon"`, which is the value that fails in the first place),
  and when nothing can be spawned the notification says where it looked and
  opens the setting.

  A `--debug` build recorded the source directory exactly as typed, so
  `quilon build --debug examples/hello.qn` wrote a relative `DW_AT_comp_dir` of `examples`.
  A debugger resolves that against its own working directory, so the source only opened for a
  debugger that happened to run from the directory the build did. The `DIFile` directory is
  now absolute.

## 0.9.2 "Hegemon" — 2026-08-24

### Added

- **Fail-loud runtime failures say where they happened
  ([#153](https://github.com/assapir/quilon/issues/153)).** A checked `arr[i]` that is out
  of bounds, negative, or NaN, and a violated `Text.replace` / `replaceAll` / `repeat`
  contract, now report the *expression that broke the contract* — a `file:line:column:`
  position line, the message on the line under it, then the source line and a caret run —
  instead of a bare stderr line:

  ```text
  demo.qn:4:11:
  index 7 out of bounds for an array of size 3
     |
   4 |   value = items[wanted]
     |           ^^^^^^^^^^^^^
  ```

  Same frame as a compile error and a failing assertion — one shape now covers every located
  failure a program can produce, and all three shorten a path too long for the position line
  from its START (`…/deep/module.qn:4:11:`) so the file name stays visible and the line does
  not wrap — and a program with several array reads no longer leaves you
  hunting for which one it was. Exit codes are unchanged (`1` for a bad index, `101` for a
  `Text` contract), and a redirected report stays plain. The location is compiled in — each
  fallible intrinsic takes the read's `Site` constant — so a native build reports exactly
  what `quilon run` does, with no debug info and no unwinder. `tests/fail_loud_location_test.rs`
  pins compile errors, assertions, and runtime checks to the same framing, since the
  assertion renderer lives in Quilon (`corelib/test.qn`) and the runtime one in Rust
  (`quilon-rt`'s `report`). `examples/array_methods.qn` shows the non-aborting alternative:
  `at(n)` hands an out-of-range index back as `NotOk` instead of stopping the program.

- **Built-in `Map` and `Set` collections ([#72](https://github.com/assapir/quilon/issues/72)).**
  Keyed lookup arrives as two built-in parametric collection primitives — like `[]T`, not
  user-defined generics. A **pipe fence** carries both the type and the literal: a map is
  `[|K => V|]` with `=>` reading "maps to" (`[|"a" => 1, "b" => 2|]`, empty `[|=>|]`); a set
  is `[|T|]` (`[|"a", "b"|]`, empty `[||]`). The fence is what keeps a set literal distinct
  from an array (`[1, 2, 3]`). Both are **immutable** — every mutating method returns a NEW
  collection — and backed by plain `std::collections::HashMap`/`HashSet` in `quilon-rt` over
  GC memory. Keys are the built-in hashable types (Num / Text / Bool; Text hashes by
  content, consistent with value `==`). A map value is read only through `m.get(k)`, the
  safe `Ok(v)`/`NotOk` form — there is no bracket indexing on a map. Map
  methods: `.get`/`.has`/`.set`/`.keys`/`.values`/`.each` plus the `.size` field; set
  methods: `.has`/`.add`/`.items`/`.each` plus `.size`. **Set algebra** is spelled with
  single-token operators: `+` union, `-` difference, `+-` (= `-+`) intersection.
  **Iteration order is unspecified** and must not be relied on — the runtime uses a
  fixed-seed hasher so a program is reproducible run-to-run, but the order is unspecified by
  contract, not insertion order. User-defined key hashing (the `%`/`==` hooks), `remove`,
  and handing `^`'s env over as a `[|Text => Text|]` map are deferred to a later slice. See
  `examples/maps.qn` and `examples/sets.qn`.
- **Deferred values — the `@readStdin` leaf IO primitive ([#120](https://github.com/assapir/quilon/issues/120)).**
  The value-returning half of the colorless implicit-futures model. `@readStdin()` (in
  `core.io`) reads one line from stdin and returns a **deferred** `Text`: calling it
  *launches* the read on a background fiber and hands back the value immediately — the
  caller does not wait. The value threads lazily through bindings, records, and calls
  (promise pipelining) and is **forced on use** — the fiber parks until the bytes are
  ready — only where a strict primitive reads them (a comparison, `print`/`write`, a
  native call, the match scrutinee, an `@`-primitive argument, the `^` exit). At
  end-of-input it yields the empty `Text` `""`. A new pre-codegen **deferred-taint pass**
  colors which expressions can be deferred and marks the exact force-frontier sites;
  codegen emits a memoized park-or-read `force` there and a hybrid representation (a
  deferred `Text` is `{ promise, -1 }`, a real byte length is never negative) — so
  **only tainted values carry the promise representation and pure code is byte-identical**
  (zero overhead). The **type checker is unchanged**: a deferred `Text` still types as
  `Text` (no `Task`/`Future` type), so overload resolution is untouched — forcing keys off
  the operation, never the type. The promise records its `@readStdin` launch site so a read
  fault reports where the IO was called; a scope runs its launched reads to completion
  before it exits (effects never vanish). Also fixes checking a corelib file directly
  (`quilon check corelib/time.qn`): the front-end now trusts a bundled corelib source to
  declare `@` primitives while still rejecting them in user code. See
  `examples/readStdin.qn` (pipe it a line to watch a real value flow); cross-source *overlap*
  is demonstrated later with a networked primitive.
- **Failing assertions say where they failed, and a general call-site facility to build
  that on ([#65](https://github.com/assapir/quilon/issues/65)).** A failing `core.test`
  assertion now reports in the shape of a compiler error — the failing call's own
  `file:line:column:`, the message on the line under it, then the source line and a caret
  run under the call:

  ```text
  demo.qn:12:3:
  assertion failed: expected 42, got 41
     |
  12 |   assertEq(answer(), 42)
     |   ^^^^^^^^^^^^^^^^^^^^^^
  ```

  The location is the **user's** call site, not an internal hop: `assertEq` fails several
  calls deep inside `core.test` and still points at the line where the program called
  `assertEq`, including inside a helper rather than `^`. The report is colored when stderr
  is a terminal, plain when redirected or under `NO_COLOR`.

  The mechanism is a new built-in record type **`Site`** (`file`/`line`/`column`/`excerpt`/
  `width`), usable in any signature with no import: a function whose **last** parameter is
  a `Site` receives the location of the call that left that argument off, and **passing one
  explicitly forwards it** — which is the whole propagation rule, and what makes a chain of
  wrappers blame the outermost caller (Rust's `#[track_caller]`, as an ordinary argument).
  It is compile-time only, and free while the program runs: every field is a constant, so
  each call site is emitted as a **read-only constant** whose address the call passes — no
  allocation and no stores, so a passing assertion costs its comparison and a pointer
  argument even in the hottest loop (1M asserted iterations: 3 ms). A `Site` is therefore
  **read-only** — a location is a value, not a variable, so writing one of its fields is a
  compile error however the value was reached, since records alias and a write through any
  binding would be a write to that constant. There is no unwinder
  and no debug info to keep, and `quilon run` (JIT) and native builds report identically. A
  `Site` parameter that nothing could fill in — before another parameter, or on a lambda, a
  nested declaration, or a record method — is a compile error rather than a silent demand
  for an explicit location. See `examples/call_site.qn`.

  Supporting surface, all documented: `core.test`'s **`failAt(message)`** (the reporting
  primitive the assertions are built from, and what a custom assertion of your own
  forwards its own `site` to), **`Text.repeat(count)`** (`count` copies; fail-loud on a
  negative or fractional count, at compile time when literal), and the **`\e`** string
  escape for the ESC byte, without which `.qn` code could not write an ANSI sequence at
  all. The terminal check behind the coloring is an INTERNAL primitive
  (`__color_enabled`, alongside `__exit`): raw file descriptors gain no user-facing API,
  since the language's IO direction is `@` leaf primitives rather than `fd`-taking
  functions, and a user-facing color story waits for that design.
  Internally, the front end now carries a `SourceMap` (every file's path and text, keyed by
  the `FileId` its spans carry) through to codegen, and compiler diagnostics and `Site`
  values resolve a span through the same code — so both report a position and caret width
  identically.
- **Concurrency runtime — the `@sleep` leaf IO primitive ([#120](https://github.com/assapir/quilon/issues/120)).**
  The first Quilon-visible surface of the colorless implicit-futures model: `@sleep`
  (in the new `core.time` module), an effect-only pause. `@sleep(secs)` takes seconds
  (a fractional `Num`, like Python's `time.sleep`) and **waits right there** on the
  current fiber, then execution continues in program order; it yields `$` (Unit). The
  program's entry runs on the single-threaded fiber scheduler so `@sleep` has a fiber to
  park on — but **only when the program uses an `@` primitive**, so pure programs are
  byte-identical (the emitted LLVM IR is unchanged; zero overhead). The `@` marker names
  a leaf IO primitive and is **corelib-only**: user code calls one but cannot declare
  one, and the type system is untouched (`@sleep` is a plain `Num -> $`, no `Task`/
  `Future` type). This lands the runtime surface; the *deferred value* story — a
  value-returning primitive whose result threads lazily and is forced at a strict
  operation, giving automatic overlap — arrives with a later primitive (`@readStdin`). See
  `examples/sleep.qn`.
- **Uniform `Result` layout — a `Result` of any payload flows through a generic
  `(r :: Result)` parameter/return.** Every `Result` now has a single canonical LLVM
  shape `{ i8 tag, {ptr,i64} slot }`: a `Text` or array payload fills the slot
  directly, and a scalar (`Num`/`Bool`/`$`) is packed into it, then unpacked back to
  its concrete type at the match site. Previously a `Result` was sized to its actual
  payload per value, so a composite-payload result (`Ok("x")`, `Ok(["a"])`) had a
  different LLVM type from the `{ i8, double }` a generic `(r :: Result)` parameter
  expected, and the call was rejected by the verifier. With one shape, `assertOk` /
  `assertNotOk` (`core.test`) now accept a `Result` of **any** payload — including the
  composite-payload results of `getEnv` / `getOpt` — so `examples/cli.qn` asserts them
  directly instead of bridging through a `match → Bool`. Matching by variant (`Ok` vs
  `NotOk`) works on any `Result` anywhere; extracting a payload still needs its concrete
  type in scope at the match site (there are no generics). Debug (`--debug`) DWARF and
  format-string rendering of a `Result` follow the new layout.
- **A functional update may name the type it builds:** `Vec { <-p, x = 9 }` alongside
  the anonymous `{ <-p, x = 9 }`. Both forms now parse through the same field list, so a
  spread is accepted wherever record fields are. Naming the target constrains the source:
  it must **already be that type**, or an **anonymous record of exactly its shape** (same
  fields and types, nothing extra). A different named type is never accepted, however
  similar — `Point` and `Other` remain distinct — and an anonymous record cannot fill a
  type that declares methods, since it carries none. Every declared field must end up
  provided, by the spread or by an override. See `examples/spread.qn`.
- **String interpolation / format strings.** A string literal may contain
  **interpolation holes** — an arbitrary expression wrapped in backticks — which
  are rendered to `Text` and spliced in: `` "hi `user.name`" ``,
  `` "sum: `a + b`" ``, `` "port `getPort()`" ``. A hole can hold a value of any
  type. A **doubled backtick** `` `` `` is one literal backtick (never a hole).
  Rendering goes through a single, overloadable **render operator `` ` ``**: every
  built-in type has a default rendering (Num without trailing zeros; `Bool` as
  `True`/`False`, capitalized; `Text` as itself; a record as its type name; a sum
  value as its variant name; an array as `[a, b, c]`, or `[first <- last]` when it
  has more than 10 elements), and **any user type may override** its rendering by
  defining its own `` ` `` operator method-style (`it` is the instance; it returns
  `Text` and may itself interpolate). `print`/`eprint` now render **any** value
  through the same `` ` `` path, so `print(user)` and `` "`user`" `` agree — and
  `assertEq`/`assertNotEq` failure messages now render records, sum types, and
  arrays too. There are no format specifiers. (See `examples/interpolation.qn`.)
  (#101)

### Changed

- **Breaking: source files are `.qn`**, and the compiler accepts nothing else
  ([#172](https://github.com/assapir/quilon/issues/172)). Rename your `.ql` sources — nothing
  else about them changes; a program or a `<<`-imported module named anything but `.qn` is
  rejected, by name, before it is read. `.ql` is CodeQL's extension, and GitHub was
  attributing ~40% of this repository to CodeQL because of it — the language bar advertised
  someone else's language for every Quilon program in the tree. Every source file here is
  renamed (`git mv`, so history follows). `.qn` is unclaimed, so the misattribution stops with
  the rename itself; a `.gitattributes` override (`*.qn linguist-language=Quilon`) asks for
  the files to be labelled Quilon, which needs Quilon in Linguist proper to take effect. The
  VS Code extension registers `.qn` only, to match.

- **CI shows benchmark deltas against the previous run on the branch
  ([#162](https://github.com/assapir/quilon/issues/162)).** Both benchmark families print a
  `Δ` column beside their measurements, so performance drift shows up in the run that
  introduced it rather than as a column someone has to diff across job summaries. The
  numbers are kept between runs in `actions/cache` and restored by prefix; a missing
  baseline (first run, evicted cache, a fork) prints the tables exactly as before. Still
  **informational** — no threshold, nothing gates on a delta — because shared runners are
  noisy in absolute terms and only interleaved runs on one machine compare credibly; the
  summary says so where a reader sees it. The same flags work locally:
  `cargo bench --bench compile_speed -- --metrics before.tsv`, then `--baseline before.tsv`
  after a change.

- **Every located report puts the message on its own line, and shortens a long path.**
  Since 0.9.1 a compile error read `path:line:col: error: <message>` on one line; it — and
  the assertion and fail-loud runtime reports added in this release — now print the position
  and the message separately:

  ```text
  demo.qn:2:7:
  error: No overload of '+' matches argument types (Num, Bool)
    |
  2 |   x = 1 + true
    |       ^^^^^^^^
  ```

  "Where" and "what" are different questions, and a long message no longer pushes the
  position off the right edge. A path wider than 60 characters is shown from its END behind a
  `…` (`…/scratch/demo/bounds.qn:7:11:`), so the file name stays visible instead of the line
  wrapping — absolute paths and temp directories made that routine. **Anything parsing
  compiler output for `: error: ` on the position line needs updating**; the position line
  now ends at the colon.

- **A function nothing can reach from `^` is no longer emitted, so importing a
  library costs only what you use from it.** `<< core.test` brought in every
  assertion the module defines and all of them were emitted — and, under
  `quilon run`, JIT-compiled — whether the program called one or none. Across the
  examples that was more than half of every function emitted (533 down to 323).
  `quilon run` is now 9–14% faster on programs that import the core library
  (`examples/hello_world.qn` 11.3 ms → 10.0 ms), and the benchmark's
  library-importing one-liner drops from 10.8 ms to 9.6 ms; emitting the code for
  such a program takes a tenth of the time it did (the `corelib` corpus's codegen
  phase, 1.1 ms → 0.1 ms). Nothing changes for a program with no unused code: the
  emitted LLVM IR is byte-identical, and the analysis itself is not measurable
  (`flat`, 4000 reachable functions, is unchanged). What a program does use is
  unaffected however indirectly it is reached — through an operator overload, a
  render override called only by interpolation, a helper called only from a method,
  or overload dispatch. A module compiled on its own, with no `^`, keeps everything.
- **The compile-speed benchmark corpora now call every function they define.** Their
  entry points reached one or two, which was fine when everything was emitted anyway
  but would have left five of the eight measuring almost nothing — `flat` emitting 4
  of its 4000 functions. This breaks numeric comparability with figures recorded
  before it, deliberately: the alternative was a codegen column that no longer
  measured codegen. `corelib` is the exception and still imports a library it barely
  uses, since that is the shape the pruning above exists for. `runtime_speed` gains a
  second latency row for a one-liner **with** an import, beside the import-free one.
- **Record-heavy programs type-check about three times faster.** A user-declared
  record type carried its whole field and method list by value, and a `Type` is
  copied constantly — into the type table for every expression, back out of it in
  codegen, and through each inference step — so every one of those copies
  reallocated the field list and a `String` for each field name. The declaration
  never changes once the checker has built it, so it is now shared rather than
  copied. On the new `records` benchmark corpus (30 record types of 20 fields)
  type checking drops from 3.8 ms to 1.3 ms and the whole compile from 28.4 ms
  to 25.2 ms. As a side effect every `Type` in the compiler shrank from 72 to 56
  bytes, which shows up as a smaller peak RSS (143.7 MB → 139.5 MB) and slightly
  faster checking even for programs with no records in them. Generated code is
  unaffected — the emitted LLVM IR is byte-identical for every example.
- **`if` and `while` are no longer reserved words.** The lexer still had tokens for
  them, left over from a design the language never took — nothing in the parser ever
  consumed one, so their only effect was to make `if = 5` fail with "Unexpected token"
  instead of binding a variable. They lex as ordinary identifiers now, the way `for`
  did when its loop was removed, which makes the no-keywords claim literally true: not
  one word is reserved.
- **Breaking: every member of an overload set must annotate its return type**, as it
  already had to annotate every parameter. A member's return type used to default to
  `Num` when omitted and was corrected only after its body was checked, so what a call
  saw depended on where it sat relative to the definition: a call above it resolved
  against the `Num` placeholder and either passed `quilon check` only to fail at
  runtime (`Overload not found: g$N`), or was rejected with a bogus complaint about a
  type nobody wrote (`expected Text, got Num`). A member's signature is now fixed at
  its definition, and the omission is reported instead — at the call that needed the
  result type, or at the definition when nothing calls it:
  `cannot call 'g': its overload member (Num) has no return type annotation — annotate
  it, since exact dispatch needs the full signature`. This also makes a recursive
  overload member expressible: annotate it and the self-call resolves. An unannotated
  comparison-operator overload (`==`, `<=`, …) now asks for the annotation rather than
  reporting that it must return `Bool`.
- **Breaking: an overload member joins its set where it is written**, so a call resolves
  only against the members above it — names resolve top to bottom, with no hoisting, the
  same rule plain functions have always followed. Members used to be registered in a
  pre-pass, so a call could resolve against a definition further down the file that
  codegen then had no symbol for: a fully annotated program passed `quilon check` and
  died with `Overload not found: odd$N`. Such a call is now a compile error
  (`cannot call 'odd' before its definition — Quilon resolves names top to bottom; move
  the definition above this call`). A definition is still in scope for its own body, so
  self-recursion is unaffected; mutual recursion between top-level functions is not
  expressible (it never worked — it only appeared to type-check). See
  `examples/overload_dispatch.qn` and docs/LANGUAGE.md's "Names resolve top to bottom".

### Fixed

- **A top-level binding that has to be computed is now a clear compile error instead
  of a broken build.** A binding outside any function becomes a global, whose
  initializer must already be a constant — nothing runs before `^` in which to
  compute one. Only literals and arrays/records were ever handled, and the rest fell
  through into codegen, which generated the value's instructions with the LLVM builder
  still pointing wherever the last emitted function had left it: `x = 1 + 2` surfaced
  as the internal `Failed to build add: UnsetPosition`, and `x = f(1)` silently
  appended a call to the previously emitted function, producing a module that failed
  verification (`Basic Block in function 'write' does not have terminator!`). Both
  passed `quilon check` first. The type checker now rejects such a binding where it is
  written, naming the supported forms and the fix, so `check`, `run` and `build` agree.
  A `Num`/`Bool`/`$` literal, a function value, and a mutable (`:=`) global all still
  work, and everything is unrestricted inside a function. Computing a global's value at
  startup remains unimplemented — see `examples/globals.qn` and
  `examples/global_computed.qn`, and docs/LANGUAGE.md's "A top-level binding must be a
  constant or a function".

## 0.9.1 "Towel" — 2026-08-14 — "Stable basics, hardened"

Everything merged since 0.9.0: the M1–M3 language-surface work (overloading, sum
types, closures, ranges, spread, array/`Text` methods, `Unit`, guaranteed TCO,
`^` args/env, `core.test`/`core.cli`), a cluster of correctness fixes, the
runtime-library licensing exception, a provenance watermark, and a distribution
fix that makes a bare `quilon` binary self-contained. No release tag stood
between 0.9.0 and this one, so this section covers the whole span.

### Added

- **Ad-hoc overloading — the only polymorphism.** Two or more top-level
  definitions that share a name and each annotate **all** their parameters form
  an overload set (no marker keyword); call sites dispatch by **exact** static
  argument type, with no implicit coercion, and an unmatched/ambiguous call is a
  compile error listing the candidates. **Operators are overload sets too**
  (`+ - * / %`, `== != < <= > >=`): the built-ins (e.g. `+` on `Num` and on
  `Text`) are visible overloads, and a user definition named with the operator
  symbol adds a member for a user type. `==`/`!=` and `<`/`<=`/`>`/`>=` over
  `Text` (equality + lexicographic order) ship as built-in overloads.
  Comparison/equality overloads must return `Bool`. (#32)
- **User-defined sum types (`/` separator).** A set of named variants, nullary
  or with built-in-typed payloads (`Color = Red / Green / Blue`,
  `Shape = Circle(Num) / Rect(Num, Num)`), constructed by name and consumed by
  exhaustive `?`/`|` matching that binds the payload. `Result` (`Ok`/`NotOk`) is
  just a predefined sum type. (#28) A pattern-bound `Ok`/`NotOk` payload now
  carries its **concrete type**, so it is usable at the match site and across a
  `-> Result` function boundary (overload dispatch sees the real `Num`/`Text`/
  `Bool`). (#53)
- **`Unit` type and value — `$`.** A type with exactly one value, both written
  `$`; `print`/`eprint` return `$`, and a `$`-bodied `^` exits 0. (#25)
- **`Text` methods.** `split`/`trim`/`trimStart`/`trimEnd`/`replaceAll`/`replace`/
  `contains`/`indexOf`/`slice`/`toUpper`/`toLower` — compiler-provided, chainable,
  grapheme-based, UTF-8-correct, and fail-loud where the request is invalid
  (`replace` count/empty-argument checks). (#52)
- **`Text` and nested arrays inside composites.** A codegen type-oracle
  side-table lets `Text` (and nested arrays) live in records and arrays and be
  carried as sum-type payloads (`Ok("done")`), reading back at their real type —
  the previous numeric-only restriction on composite contents is lifted. (#35)
- **Array methods.** `map`/`filter`/`reduce`/`each`/`find`/`at` — built-in,
  chainable, taking a lambda the compiler inlines per element; `each` returns the
  receiver, `find`/`at` return `Ok`/`NotOk`. (#40)
- **Array concatenation via `+`.** `[]T + []T` (concat), `[]T + T` (append), and
  `T + []T` (prepend), each selected by exact operand types, always building a
  new array. (#51)
- **Ranges — infix `lo <- hi`.** An inclusive `[]Num` (`1 <- 4` → `[1,2,3,4]`,
  descending when `lo > hi`); pure array sugar, so it composes with `.size`,
  indexing, and the array methods. (#34)
- **Spread — prefix `<-` in literals.** Array splice (`[<-xs, 4]`) and record
  functional-update (`{<-p, x = 9}`, preserving a named record's type + methods
  when it only overrides existing fields). Disambiguated from the range `<-`
  purely by position. (#43)
- **Closures.** A function nested in another body captures the enclosing locals
  it names; how is decided by the binding operator — `=` captures by value (a
  frozen snapshot), `:=` captures by reference (a shared, mutable cell that
  outlives the frame). Monomorphic in this milestone. (#36)
- **Guaranteed self-tail-call optimization.** A function whose result is a call
  to itself in tail position is lowered to a loop (parameters become
  loop-carried slots), so tail recursion runs in constant stack and never
  overflows, however deep. (#37)
- **In-place mutation of `:=` records.** Direct field writes (`obj.field := v`)
  and **setter** methods — a method is a setter exactly when its body writes
  `it.field := …`; there is no marker, and a setter call requires a `:=`
  receiver. (#26)
- **`^` receives `args` and `env`.** The entry point may declare
  `args :: []Text` (argv, including `argv[0]`) and `env :: [][]Text` (environment
  as `[key, value]` pairs); the generated `main()` fills them from C
  `argc`/`argv`/`envp`. Both are real Quilon arrays. (#39)
- **`core.test` module.** In-language assertions for self-verifying programs:
  `assert` (with an `AssertOpts { message }` overload), `assertEq`, `assertNotEq`,
  `assertOk`, `assertNotOk`; a failing assertion prints to stderr and exits 101.
  Pure Quilon (`corelib/test.ql`) over a single process-exit intrinsic. (#63)
- **`core.cli` module.** Pipe-friendly `getEnv` / `hasFlag` / `getOpt` over the
  entry point's `args`/`env` (both `--name value` and `--name=value`; flag names
  with or without `--`). Pure Quilon (`corelib/cli.ql`), no new intrinsics. (#66)
- **Human-readable diagnostics.** Compile errors from the lexer, parser, and
  type checker are reported rustc-style: a `path:line:col: error: <message>`
  header with the offending source line and a caret underline (1-based,
  character-counted columns). (#23)
- **Source-level debugging: `quilon build --debug` (`-g`).** Native builds can now
  emit **DWARF line-number debug info**, so a debugger (`gdb`/`lldb`) can set
  breakpoints, single-step, and print backtraces in terms of `.ql` source lines.
  Each emitted function (top-level functions, methods, closures, and the generated
  `main` wrapper) gets a `DISubprogram`, and every expression is attributed to its
  source location; the compile unit records the `.ql` file. Verify with
  `llvm-dwarfdump --debug-line ./program` / `--debug-info`. Builds are already
  unoptimized, so `--debug` only *adds* the info — the non-debug build path is
  unchanged and carries no debug info. Known limitation: debug info covers the
  program's own source file only — functions imported from other modules (`<<`)
  carry no usable line info, because the debug info holds a single compile unit and
  source text to resolve offsets against; multi-file line info is a follow-up (source
  positions do now carry the identity of the file they index into, so it has what it
  needs). (#100)
- **`--debug` also emits local-variable and type debug info.** Beyond line tables,
  `--debug` now emits **DWARF local variables, parameters, and debug types**, so a
  debugger can inspect each `.ql` value with its correct Quilon type. Every parameter
  and `=`/`:=` local gets a typed `DILocalVariable` + `#dbg_declare`, attached to its
  function's subprogram or a nested lexical block (blocks and closures get their own
  scopes). The Quilon type system maps to **distinct** DWARF entries: `Num`/`Bool` as
  base types, and `Text`, `[]T`, records, and each sum type as distinctly-named
  composites — even though they share a `{ptr, i64}`-ish LLVM shape — so a debugger
  (and a future pretty-printer) can tell a `Text` from a `[]Num` from a record from a
  `Result`. Sum types are emitted as layout-faithful tagged structs
  (`{ i8 tag, payload… }`); a self-describing DWARF *variant part* is a possible later
  refinement. Verify with `llvm-dwarfdump --debug-info ./program`. Still `--debug`-only
  — the non-debug build path is unchanged. (#100)
- **Provenance watermark in native binaries.** Every executable `quilon build`
  produces now carries a plaintext watermark —
  `Built with Quilon by Assaf Sapir - github.com/assapir/quilon` — in the ELF
  `.comment` section, next to the C toolchain's own producer string. It is
  emitted as an `!llvm.ident` module metadata entry, which LLVM lowers into
  `.comment` during object generation, so it survives linking. Inspect it with
  `readelf -p .comment ./program` or `strings`. The string is a fixed
  compile-time constant (no build date), builds stay reproducible, and there is
  no runtime effect; `strip -R .comment` (or `objcopy --remove-section=.comment`)
  removes it — plain `strip --strip-all` keeps it, since GNU `strip` preserves the
  `.comment` section. `quilon run` (JIT) produces
  no artifact and so carries no watermark. (#45)
- **Runtime-library exception (licensing).** The Quilon runtime (`quilon-rt`),
  which is statically linked and embedded into every binary `quilon build`
  produces, now carries a Classpath-style linking exception on top of GPLv2 (see
  `LICENSE-EXCEPTION.md`). The exception also covers the runtime boilerplate the
  compiler emits into its output (e.g. the generated C-compatible `main()`
  wrapper). As a result, **programs you compile with Quilon are not brought under
  the GPL by that linking and may be licensed under any terms**. The compiler and
  the runtime *source* stay GPLv2 (forking either remains copyleft); the
  exception is an additional grant that frees only the combined output. libgc
  (Boehm GC), also linked in, is separately licensed under its own permissive
  terms and needs no exception. `license` fields were added to the workspace
  crates (`GPL-2.0-only`, and `GPL-2.0-only WITH Classpath-exception-2.0` for
  `quilon-rt`) to make this machine-readable. (#93)

### Removed

- **Breaking:** removed the `for n <- collection => body` loop (and its
  `for (item, index) <- …` form). Iteration is now expressed with the built-in
  array methods — `.each` for side effects, `.map`/`.filter`/`.reduce` for
  transforms/folds — and with recursion (a self-tail-call is guaranteed to lower
  to a loop, so deep recursion runs in constant stack). `for` is no longer a
  keyword; it lexes as an ordinary identifier. This also retires the `for`-body
  `:=` accumulation bug. See `examples/iteration.ql`.

### Changed

- **Breaking (layout rule):** a `(`, `[`, or `{` that is the **first token on its
  source line** no longer continues the previous expression as a call, index, or
  record constructor — it begins a **new statement**. Call arguments, index
  brackets, and constructor braces must open on the same line as the expression
  they apply to; a continuation line may still start with `.`, `|>`, or an
  operator, and an argument list (or a constructor body) opened on its
  expression's line may still span lines. Previously, with no statement
  separator, adjacent statements fused across the newline: `x = f()` followed by
  a line `(1 + 2) |> print` parsed as the call `f()(1 + 2)` (a misleading "Not a
  function" error pointing at the wrong line), `b = a` followed by
  `[3, 4].each(…)` parsed as the index `a[3, 4]`, `b = a` followed by
  `{ x = 1 }` parsed as the constructor `a { x = 1 }`, and when arities/types
  lined up the fused program compiled silently and did the wrong thing. This is
  the grammar's second line-aware rule, alongside the line-final `>` block close.
  See `examples/statements.ql`.

- **Breaking:** removed the `mut` keyword. Mutability is now the `:=` operator,
  consistent with Quilon's no-keywords, symbol-based design. `x = 0` is an
  immutable binding; `counter := 0` declares a mutable binding; `counter := counter + 1`
  reassigns it. Reassigning an immutable binding (`x := …`) is a type error, and
  immutability is now enforced by the checker.
- All `examples/*.ql` programs are now **self-asserting**: each imports `<< core.test`
  and verifies every result it demonstrates with `assert`/`assertEq`/`assertNotEq`/
  `assertOk`/`assertNotOk`, exiting 0 on success (a failing assertion aborts with exit
  101). This replaces the previous idiom of encoding a result in the process exit code
  (e.g. `factorial(5)` → exit 120). The examples gate (`tests/examples_test.rs`) is
  simplified to match: the bespoke per-example `EXPECTED_EXIT` table is gone, and the
  uniform contract is now "every runnable example exits 0" under both the JIT
  (`quilon run`) and native AOT (`quilon build`, `clang` + `gcc`), with the JIT/AOT
  parity gate kept intact.

### Fixed

- **A host that runs Quilon programs on more than one thread no longer aborts.**
  The collector stops the world by signalling the threads it knows about, and it
  was never told about any: whichever thread first ran a program initialized it
  and nothing registered. A later collection then tried to stop a thread it could
  not signal and libgc killed the process — `Collecting from unknown thread`,
  `pthread_kill failed at suspend`, `Signals delivery fails constantly`, all the
  same fault at different moments. `quilon run` now registers its thread with the
  collector for the duration of the run and unregisters it afterwards, including
  the thread that did the initializing, whose entry otherwise outlives it and
  becomes the corpse the next collection trips over. A compiled binary is
  unaffected — it has one thread — so this is a fix for embedders and for the
  test suite, which no longer aborts sporadically when run with the default
  thread count.
- An expression in the file you compile can no longer retype an expression inside
  a module it imports. Every module is lexed on its own, so byte offsets restart
  at 0 in each one; the per-expression types codegen reads back were keyed on the
  byte range alone, so a range used in two files collided and the last one checked
  won. A program whose own code happened to sit on a corelib expression's offsets
  therefore passed `quilon check` and then failed to compile (`Function not
  found`, or a leaked LLVM verifier dump) or, worse, dispatched a library call to
  the wrong overload member and read a `Text` as a number. Source positions now
  carry the identity of the file they index into, which keeps each module's types
  its own; the offsets themselves are 32-bit, so a position stays smaller than
  before and deep nesting keeps its stack headroom.
- A self-tail-call whose argument type does not match the parameter slot it would
  be stored into now compiles to an ordinary call instead of taking the loop
  back-edge. Storing anyway wrote a wrong-sized value into the frame — silent
  corruption if the call resolution that got there ever disagreed with the
  declared parameter type. Arguments are still evaluated exactly once either way.
  A new self-asserting `examples/overload_dispatch.ql` pins down dispatch on
  argument types recovered from an array element, a match, a call, or a lambda —
  including a self-recursive overload member that must reach itself, not its
  sibling.
- Array and range literals used in a self-tail-recursive loop no longer overflow
  the stack. Two codegen paths materialized an array's `{ptr, size}` struct
  through a raw `alloca` at the current insert point — array indexing (`arr[i]`,
  to read the fields) and range lowering (`lo <- hi`, to build the result). A
  self-tail-call is lowered to a loop, so such an `alloca` re-ran every iteration
  and grew the stack without bound; a loop that built and indexed an array or
  range literal crashed at depth, violating the constant-stack guarantee. Index
  field reads now use `extractvalue` (purely in registers) and range results go
  through the shared entry-block `array_struct` helper, so no `alloca` lands in
  the loop body. (The companion escape hazard — an array literal returned from a
  function reading garbage after its frame died — was fixed earlier by
  heap-allocating literal backing stores; all sub-cases now have regression
  coverage, including a self-asserting `examples/array_literal_lifetime.ql`.)
  (#67)
- The recursive-descent parser had no recursion-depth limit, so deeply nested
  input (e.g. ~2000 nested `(`, `[`, or `{`) overflowed the native stack and
  aborted the compiler with a SIGABRT/core dump (exit 134) instead of producing
  a diagnostic — any hostile or machine-generated file could crash `quilon`.
  The parser now bounds nesting depth (max 128 levels) across every construct
  that can nest unboundedly — parenthesized expressions, array/record literals,
  block statements, `[]T` element types, nested constructor patterns
  (`Ok(Ok(…))`), chained prefix operators (`---…x`), and `:=` chains — and
  reports a clean, source-located
  `path:line:col: error: expression nesting too deep …` diagnostic (exit 1) once
  the limit is exceeded. Ordinary code nests only a handful of levels, so the
  limit affects only pathological input. (#76)
- `quilon build` was unusable from a distributed binary (a GitHub-release
  download or any machine other than the one that compiled the compiler): it
  looked for `libquilon_rt.a` at a path baked in at compile time and next to
  the binary, and releases ship the bare binary only. The runtime archive is
  now **embedded, gzip-compressed, in the `quilon` binary itself** and
  decompressed on first use into the per-user cache (`$XDG_CACHE_HOME/quilon`,
  default `~/.cache/quilon`), keyed by content hash so a new compiler never
  links a stale archive and later builds reuse the cached copy without
  re-decompressing. A system-provided archive always wins over the embedded
  one — lookup order: a `QUILON_RT_LIB` environment variable set at run time
  (developer override) → `libquilon_rt.a` next to the binary (dev loop) → the
  cached extraction → decompress the embedded copy. `quilon build` is now
  fully self-contained; the system libgc requirement remains and is tracked
  separately. (#78)
- `%` (modulo) had no codegen: it was documented and type-checked, then failed
  at `run`/`build` with an internal "Unsupported binary operation" error. It now
  lowers to the f64 remainder (LLVM `frem`, i.e. C `fmod`): works on fractional
  operands, and the result takes the dividend's sign. (#73)
- `&&` and `||` now actually short-circuit, as documented — the right operand
  is evaluated only when the left does not already decide the result. They were
  lowered as eager bitwise and/or, so `false && f(x)` ran `f`'s side effects and
  the guard idiom `i < a.size && a[i] == k` always performed the (unchecked)
  index. (#71)
- A literal or nested constructor inside a constructor pattern (`Ok(1)`,
  `Ok(Ok(x))`) was accepted by the checker but silently ignored by codegen —
  the arm matched *any* payload of the variant, so the wrong arm could win with
  no diagnostic. Such refutable sub-patterns are now a compile error (payload
  sub-patterns must be a binding or `_`); bind the payload and compare it in the
  arm body instead. (#70)
- Codegen kept per-function state (`record_types`, `var_named_types`, and — on the
  method path — `var_types`) across function emissions, so a later function reusing
  a variable name could be silently miscompiled (e.g. an array parameter `p`'s
  `p.size` mis-routed to a record-field read after an earlier function bound a
  record to `p`). Every function/method/lambda emission now starts from an empty
  per-function frame, and closures carry their captured variables' type metadata
  into the lifted frame explicitly. (#68)
- Type checking deeply nested call chains was exponential in nesting depth —
  every call site inferred its first argument twice, so ~22 nested calls took
  seconds and ~26 hung the checker outright. Since `|>` desugars to first-arg
  nesting, long pipelines hit the same wall. Each argument is now inferred
  exactly once per call site; 60-deep chains check instantly.
- Array indexing `arr[i]` is now **checked**: an out-of-bounds, negative, or NaN
  index reports a clear runtime error to stderr and exits 1. Previously it was a
  raw unchecked read — garbage values for OOB/negative indices and LLVM poison
  (undefined behavior) for NaN. A fractional in-range index still truncates
  toward zero (documented), and `.at(n)` remains the non-aborting `Ok`/`NotOk`
  form — its bounds check now also runs before the float conversion, so
  `at(0/0)` returns `NotOk` instead of branching on poison. (#74)
- `quilon run` (JIT) and a native build now agree on `args`: the `quilon run`
  CLI prefix is stripped and the `.ql` path becomes `argv[0]`, so
  `quilon run f.ql a b c` gives the program the same `args.size` and trailing
  arguments as `./f a b c`. Previously the JIT leaked the CLI prefix into `argv`.
  (#44)
- `quilon build` places `libquilon_rt.a` deterministically for the local
  dev loop (next to the binary), superseded for distributed binaries by the
  embedded, gzip-compressed runtime archive above. (#38)

## 0.9.0 — "Stable basics"

The first stabilized release: a small but **verified, runnable** core of the
language. Programs run end-to-end and every example is checked in CI through both
the JIT and native-AOT paths. This is a stable *core*, **not** feature-complete —
see "Known limitations".

### Language

- **Entry point `^`** — `^ = () -> Num => …`. If the body isn't a `Num`, the
  program exits `0` (C `main`-style success), so side-effecting mains need no
  trailing `0`.
- **Modules** — `<< core.io` imports; a `>>` prefix exports a top-level item.
  Built-in `core.io` plus relative/absolute file-path imports.
- **Pipe `|>`** with first-argument injection: `x |> f` ⇒ `f(x)`,
  `x |> f(a)` ⇒ `f(x, a)`.
- **Loops** — `for n <- collection => body` (and `for (n, i) <- collection => body`).
- **`Text`** — a built-in string type (no import): `+` concatenation, `.size`
  (byte length), `.length` (grapheme-cluster count, full UTF-8).
- **`core.io`** — `print(x)` / `eprint(x)` (newline-terminated, over Num/Text/Bool),
  `write(content, fd)`, and `stdout` / `stderr`.
- Numbers (`Num`, f64), `Bool`, arrays (`.size`, indexing), records and named
  record types with methods (implicit `it`), sum types `Ok` / `NotOk` with
  pattern matching, ternary `? :`, blocks `< … >`, recursion, type inference.
- **Memory** — conservative garbage collection (Boehm GC).

### Tooling

- **`quilon run`** — compile and execute in-process via the LLVM JIT.
- **`quilon build [--linker clang|gcc] [-o out]`** — emit a native executable
  (object generated in-process via LLVM `TargetMachine`, linked against the
  bundled `libquilon_rt`).
- **`quilon compile` / `quilon check`** — emit LLVM IR / type-check only.
- **Strict CI** — deny-warnings build, blocking `clippy -D warnings`, `fmt`
  check, and a gate that runs every example through JIT **and** native AOT under
  **both** clang and gcc, asserting matching exit codes.
- Built against **LLVM 22** (inkwell).

### Known limitations (planned for a later release)

- **No generics yet** — `Text` (and other non-numeric values) inside records,
  arrays, or `Ok`/`NotOk` payloads are not supported; numeric payloads work.
- No closures, no `while` loops, no user-defined sum types, no `Unit` type.
- `Text` `.size` works only on identifier receivers in some positions.
- Boehm GC is linked dynamically (`-lgc`); self-contained static GC is planned.
- `argv` is a placeholder.
