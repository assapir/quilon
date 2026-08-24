~ The entry point `^` may receive the command-line `args` and the `env`ironment.
~   args :: []Text     — the program arguments (argv), including argv[0] (the program
~                        name), so `args.size` is always at least 1.
~   env  :: [][]Text   — the environment as [key, value] pairs (each inner array is two
~                        Texts: the variable name and its value, split on the first `=`).
~ The one fact that holds however the program is invoked is that argv[0] is present, so
~ `<< core.test` asserts exactly that; the program always runs to completion and exits 0.
<< core.io
<< core.test

^ = (args :: []Text, env :: [][]Text) -> $ => <
  assert(args.size >= 1)        ~ argv[0] is always present -> args is non-empty

  ~ Read argv[0] back as a Text and count the env's [key, value] pairs, exercising the
  ~ []Text and [][]Text entry-point paths. Both are invocation-dependent, so they are
  ~ shown (printed) rather than asserted on.
  first :: Text = args[0]
  pairCount :: Num = env.size
  print(first)
  print(pairCount)
>
