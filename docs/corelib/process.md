---
title: "core.process — Signals"
sidebar:
  label: "core.process"
  order: 7
---

# `core.process` — Signals

Import with `<< core.process`. See the [corelib index](README.md). This module is the
[signal trap](../concurrency/README.md#signal-trap)'s own vocabulary: a `Sender` record and
a `Signal` sum, nothing else.

`Sender`, what the OS reports about a signal's sender:

| Member | Effect |
|--------|--------|
| `sender.pid :: Num` | The sending process's id. A signal with no sending process — a terminal hangup, an alarm — carries `0`. |
| `sender.uid :: Num` | The real user id the sending process ran as. |

`Signal`, one variant per signal a program can trap, each carrying the `Sender` that sent it:

| Variant | OS signal |
|---------|-----------|
| `Hangup(Sender)` | `SIGHUP` |
| `Interrupt(Sender)` | `SIGINT` |
| `Quit(Sender)` | `SIGQUIT` |
| `Terminate(Sender)` | `SIGTERM` |
| `Alarm(Sender)` | `SIGALRM` |
| `UserDefined1(Sender)` | `SIGUSR1` |
| `UserDefined2(Sender)` | `SIGUSR2` |

A trap's arms match `Signal` by these bare names — `Interrupt`, `Terminate`, and so on —
the way `Ok`/`NotOk` resolve for a `Result`; an ordinary match on a `Signal` value takes
the qualified `process.Interrupt` spelling instead. See the
[signal trap](../concurrency/README.md#signal-trap) section for the trap itself.

`SIGKILL` and `SIGSTOP` end a process without ever reaching it, on every OS that has them,
so neither is a `Signal` variant. `SIGPIPE` — the default a write to a peer that has closed
its end raises — is left ignored by the runtime, so a socket write past a closed peer is a
`Result`/`NotOk` a program matches, not a signal it would otherwise need to catch. A fault
(`SIGSEGV`, `SIGBUS`, `SIGFPE`, `SIGILL`) reports a runtime diagnostic (see
[`docs/tooling/errors/runtime.md`](../tooling/errors/runtime.md)) directly, its own report
the only thing there is for it: a program's own logic has no state left to meaningfully
resume from one.
