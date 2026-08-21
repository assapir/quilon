~ Call-site locations — the `Site` parameter.
~
~ A function whose LAST parameter is a `Site` receives the location of the call that left
~ that argument off: file, line, column, the text of the line, and how wide the call is.
~ Forwarding a received `site` on to another such function reports the ORIGINAL caller
~ instead of the hop — which is how `core.test`'s assertions blame your `assertEq` call
~ rather than their own internals. See `examples/assert_location.ql` for what a failure
~ built on this looks like.
~
~ Run: cargo run -- run examples/call_site.ql    ~ exits 0 — it checks itself
<< core.test

~ The line the call to this function is on.
callerLine = (site :: Site) -> Num => site.line

~ The column the call starts at, and how many characters it spans.
callerColumn = (site :: Site) -> Num => site.column
callerWidth = (site :: Site) -> Num => site.width

~ The file and source line the call sits in.
callerFile = (site :: Site) -> Text => site.file
callerSource = (site :: Site) -> Text => site.excerpt

~ A hop that FORWARDS its own site: the location stays the outermost caller's.
throughAWrapper = (site :: Site) -> Num => callerLine(site)

~ An assertion of your own, reporting the caller's location through `core.test`'s
~ `failAt` — the same primitive `assert` and `assertEq` are built on.
assertEven = (n :: Num, site :: Site) -> $ =>
  n % 2 == 0 ? $ : failAt("assertion failed: `n` is odd", site)

^ = () -> $ => <
  ~ Three calls on consecutive lines report consecutive line numbers — and the third
  ~ goes through a forwarding wrapper, so it reports THIS line rather than the wrapper's.
  first = callerLine()
  second = callerLine()
  wrapped = throughAWrapper()
  assertEq(second - first, 1)
  assertEq(wrapped - second, 1)

  ~ Every call reports its OWN column, so of two on one line the left one starts earlier.
  assert(callerColumn() < callerColumn())

  ~ The width is the call's own text — the same length as that text written as a literal.
  assertEq(callerWidth(), "callerWidth()".length)

  ~ The file is this example, and the source line is the one the call is written on.
  assert(callerFile().contains("call_site.ql"))
  assert(callerSource().contains("callerSource()"))

  ~ A custom assertion holds without a peep, and would report right here if it failed.
  assertEven(4)
>
