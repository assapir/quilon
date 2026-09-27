---
title: "core.process — Signals"
sidebar:
  label: "core.process"
  order: 7
---

# `core.process` — Signals

Import with `<< core.process`. See the [corelib index](README.md). This module is the
[signal trap](../concurrency/signals.md)'s own vocabulary: a `Sender` record and
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
[signal trap](../concurrency/signals.md) page for the trap itself.

`SIGKILL` and `SIGSTOP` end a process before it can react, so they have no variant. The
runtime ignores `SIGPIPE`; a write to a closed peer is a `NotOk`. A fault (`SIGSEGV`,
`SIGBUS`, `SIGFPE`, `SIGILL`) is a runtime diagnostic, see
[runtime.md](../tooling/errors/runtime.md).
