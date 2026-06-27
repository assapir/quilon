~ The entry point `^` may receive the command-line `args` and the `env`ironment.
~   args :: []Text     — the program arguments (argv), including argv[0] (the program
~                        name), so `args.size` is always at least 1.
~   env  :: [][]Text   — the environment as [key, value] pairs (each inner array is two
~                        Texts: the variable name and its value, split on the first `=`).
~ This example is deterministic regardless of how it is invoked: argv[0] is always
~ present and the runtime always builds a non-null (possibly empty) env, so it exits 7.
^ = (args :: []Text, env :: [][]Text) -> Num => <
  hasProgramName = args.size >= 1   ~ argv[0] is always there -> true
  first = args[0]                   ~ argv[0] as a Text (the program/name)
  ~ The env is an array of [key, value] pairs; touch it so the [][]Text path runs.
  envOk = env.size >= 0             ~ always true (size is non-negative)
  hasProgramName && envOk ? 7 : 0
>
