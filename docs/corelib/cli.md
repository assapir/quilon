# `core.cli` — CLI helpers

Import with `<< core.cli`. See the [corelib index](../LANGUAGE.md#corelib) and `examples/cli.qn`.

Thin, pipe-friendly helpers over the [entry point](../LANGUAGE.md#entry-point)'s
`args :: []Text` and `env :: [][]Text`. The data is always the **first** parameter, so
`env |> getEnv("PATH")` and `args |> hasFlag("-v")` read naturally.

| Function | Result |
|----------|--------|
| `getEnv(env :: [][]Text, key :: Text) -> Result` | Find the pair whose `[0]` equals `key`; `Ok(value)` (its `[1]`) if present, else `NotOk`. |
| `hasFlag(args :: []Text, flag :: Text) -> Bool` | `true` when the bare flag appears in `args`. The name works **with or without** a leading `--` (so `"verbose"` and `"--verbose"` both match an arg `"--verbose"`). |
| `getOpt(args :: []Text, name :: Text) -> Result` | Collect the option's values (argv[0] skipped), recognising both `--name value` and `--name=value`; the name works with or without `--`. Returns `Ok([]Text)` of the values in argv order (an option may repeat), or `NotOk(name)` when no value is found — the name never appears, or appears only as a trailing `--name` with nothing after it. (The `--name=value` form always supplies a value, even the empty one in `--name=`.) |

```quilon
<< core.cli
^ = (args :: []Text, env :: [][]Text) -> Num => <
  home :: Text = env |> getEnv("HOME") ? | Ok(v) => v | NotOk(_) => "?"
  verbose :: Bool = args |> hasFlag("-v")
  outputs :: []Text = args |> getOpt("--out") ? | Ok(vs) => vs | NotOk(_) => args.filter(x => false)
  verbose ? 0 : outputs.size
>
```

(See `examples/cli.qn`.)
