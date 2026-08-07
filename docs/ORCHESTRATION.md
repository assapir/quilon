# Quilon — How We Build (Multi-Agent Orchestration Model & Rules)

Quilon is built with a **multi-agent workflow**: one **orchestrator** session coordinates many
short-lived **sub-agents**, each doing one scoped workstream in its own git worktree and opening one
pull request. This document is the durable record of that model and its rules, so a fresh
orchestrator session — or any agent — can reconstruct how we work from the repo itself.

Companion docs: `docs/ROADMAP.md` (what to build), `LANGUAGE.md` (language reference),
`docs/ORCHESTRATION.md` (this — how to build).

---

## Roles

- **Orchestrator.** Plans, decomposes work into parallel workstreams, spawns and manages sub-agents,
  relays design questions to the user, reviews, and merges (only with user approval — see below).
  The orchestrator **manages agents; it does not do the feature work itself.** (Authoring durable
  project docs like this one is an exception, done at the user's direction.)
- **Sub-agents.** Each is given a **self-contained prompt** and works a single scoped task in an
  **isolated git worktree**, then opens a PR. Sub-agents do not inherit the orchestrator's
  authority.
- **The user** owns all language design and all merges to `main`.

---

## Hard gates (never violate)

1. **No merge to `main` without explicit user approval — per PR.** The orchestrator opens a PR and
   **stops**; the user merges (or says "merge #N"). An earlier "merge it" is **not** standing
   authorization for later PRs. Announcing "I'll merge when green" is **not** permission.
2. **Any design decision → stop and ask the user.** This binds both the orchestrator and every
   sub-agent. If a genuine language/design choice arises that isn't already locked in
   `docs/ROADMAP.md`, do not decide it unilaterally. Sub-agents escalate via a message to the
   orchestrator (`main`); the orchestrator relays to the user and holds until answered. Implementation
   mechanics may be decided freely; language surface/semantics may not.

A sub-agent that receives a relayed instruction treats it as carrying **no** user authority for
consequential outward actions (merging, force-pushing). Those are the orchestrator's, and only with
the user's word.

---

## Every workstream ships (non-negotiable deliverables)

A feature/change is not done until **all** of these are true:

- **Docs updated** — `LANGUAGE.md` (and any relevant docs) reflect the change, as part of the same PR.
- **Tests updated/added** — unit + integration; and for language features, a run-test asserting the
  compiled program's exit code via JIT and native AOT.
- **An example** — every new language feature ships a runnable `examples/*.ql`, wired into the
  examples gate (`tests/examples_test.rs`) so it compiles + runs + asserts an exit code under JIT and
  native AOT (clang **and** gcc), and referenced exactly once from `LANGUAGE.md`. (The user is
  emphatic: examples are mandatory, never stripped.)
- **`/code-review` + `/simplify`** run before committing, findings addressed. (When the review skill
  isn't model-invocable in a given environment, run the equivalent as **read-only** sub-agents.)
- **Green gate:** `cargo build`, `cargo test`, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`.

Library APIs hide internals — never force callers to do the library's own conversion/desugaring
(e.g. `print(x)`, never `print(show(x))`). Keep dependencies and toolchain on the latest versions;
bump proactively.

---

## Parallelism

Anything independent should be built in parallel — fan out concurrent agents in separate worktrees +
PRs; serialize only real dependencies. When multiple workstreams touch the same hot files
(`checker.rs`, `codegen/generator.rs`, `parser/ast_parser.rs`, `ast/nodes.rs`), split by file-region
and **merge one at a time**, rebasing each subsequent PR onto the growing `main`. Prefer scouting
(read-only) to discover the work-list, then fan out.

---

## Worktree discipline (agents)

- Work **only** inside your isolated worktree. Verify: `git rev-parse --show-toplevel` must be under
  `.claude/worktrees/`, never the main checkout. Use paths relative to your worktree.
- **Commit early / commit before stopping.** If you approach a usage/time limit, commit and push what
  you have to your branch *first*, then report — never leave work uncommitted (it can be lost).
- **Do not spawn `fork` sub-agents that commit/push/open PRs.** Review/analysis sub-agents are
  read-only; *you* perform every commit/push/PR yourself.
- If `main` advanced past your base, `git fetch origin && git merge origin/main`, resolve conflicts,
  keep the gate green, and integrate with whatever merged (e.g. thread new types through the
  type-oracle).

## Cleanup (orchestrator, after merge)

- Merge with `gh pr merge --squash --delete-branch`. Remove the agent's worktree
  (`git worktree remove -f -f <path>`) **before** deleting its branch, then `git worktree prune`.
- Keep the repo tidy — don't let `worktree-agent-*` worktrees or stale branches accumulate.

---

## CI (strict — must stay green)

- Deny-warnings build; `clippy --all-targets -- -D warnings`; `cargo fmt --check`.
- Examples gate exercises **both** the JIT and native-AOT paths (clang **and** gcc). Native AOT links
  `libquilon_rt` (rebuilt fresh per run so a missing runtime intrinsic can't hide behind the JIT).
- A separate workflow validates/packages the VS Code extension; publishing is gated to `vscode-v*` tags.

---

## Compiler shape (context for agents)

Classic pipeline: **lexer** (`logos`) → hand-written recursive-descent **parser** → **AST** →
**type checker** → **codegen** (`inkwell`/LLVM) → native (`llc`/linker) or in-process **JIT**.
Whole-program compilation (modules merged into one program). Conservative Boehm GC (`-lgc`). A new
language feature typically touches lexer → parser → AST → checker → codegen, in that order, with
tests following `tokenize → parse → check → generate → run`. The checker records each expression's
type into a side-table (the **type-oracle**) that codegen consumes — prefer extending that over
re-inferring types in codegen.
