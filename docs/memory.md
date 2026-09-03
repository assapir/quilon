---
title: "Memory"
---

# Memory

Memory is **garbage-collected**: heap values (`Text`, arrays, records) are freed
automatically. A compiled program runs on a machine with the operating system alone.

Allocation is **checked**. An allocation the collector fails to satisfy, and an array whose
element count times its element size is too large to represent, both stop the program with
a message on stderr and exit status 1 — the contract of a bad `array[i]` (see
[error messages](tooling/errors.md)).
