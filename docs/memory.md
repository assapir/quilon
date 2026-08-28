---
title: "Memory"
---

# Memory

Memory is **garbage-collected**: heap values (`Text`, arrays, records) are freed
automatically — there is no manual free. A compiled program needs nothing installed to run.

Allocation is **checked**, so a program never runs on memory it did not get. An allocation
the collector cannot satisfy, and an array whose element count times its element size is too
large to represent, both stop the program with a message on stderr and exit status 1 — the
same fail-loud contract a bad `array[i]` has (see
[error messages](tooling/errors.md)).
