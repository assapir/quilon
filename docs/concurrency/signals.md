---
title: "Signal trap"
sidebar:
  label: "Signal trap"
---

# Signal trap

`!>` declares the **signal trap**: one or more match arms over
[`core.process`](../corelib/process.md)'s `Signal`, each arm's body a fiber of its own.
Only the file that defines `^` may declare one, and a program declares at most one. A
library exposes what to do on shutdown as a function, and the program calls it from its arm.

```quilon ignore
<< core.net
<< core.process

@shop := net.Server { handle = 0 }

handler = (sender :: process.Sender) -> $ => < shop.kill(5) >

!> | Interrupt(s) => handler(s)
   | Terminate(_) => shop.kill(0)

^ = () -> Num => <
  shop := net.@tcpServe("127.0.0.1:8080", connection => echo(connection))
  0
>
```

An arm's pattern names one of `Signal`'s variants by its bare spelling — `Interrupt`,
`Terminate`, and so on — the way `Ok`/`NotOk` resolve for a `Result`, so a trap needs `<<
core.process` (declaring one without it is an error) but never writes the qualified
`process.Interrupt` form. A signal with no written arm keeps the OS default for it; the
trap does not need to cover every variant.

Each arm's body runs on a fresh fiber when its signal arrives — the fiber-sharing check
applies to it exactly like a `net.@tcpServe`/`http.@serve` handler's body (see
[Sharing state across fibers](README.md#sharing-state-across-fibers)): a plain `:=` global
it reaches is
[QN350](../tooling/errors/semantics.md#qn350---value-shared-across-fibers), and an `@`
global is fine. Its body is its own
[launch scope](README.md#implemented-primitives), joined before that fiber's own run ends.

A signal arriving while its own arm is still running is delivered once that arm returns —
at most one pending per signal, so a burst delivers the arm again exactly once, however
many further arrivals piled up while it ran. Returning from an arm's body ignores the
signal; ending the process from inside one is the arm's own doing (`kill`, then let `^`
return, or fall through to the OS default for an untrapped signal). The trap stays
installed for the rest of the process's life; `^` returning ends the program whether or
not the trap fired.
