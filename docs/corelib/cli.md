---
title: "core.cli — CLI helpers"
sidebar:
  label: "core.cli"
  order: 3
---

# `core.cli` — CLI helpers

Import with `<< core.cli`. See the [corelib index](README.md) and `examples/cli.qn`.

Thin helpers over the [entry point](../modules/entry-point.md)'s
`args :: []Text` and `env :: [|Text => Text|]`. The data is always the **first** parameter.

| Function | Result |
|----------|--------|
| `cli.getEnv(env :: [\|Text => Text\|], key :: Text) -> Result` | Look `key` up in the env Map; `Ok(value)` if the variable is set, else `NotOk`. The ergonomic name for the Map's own `env.get(key)`. |
| `cli.hasFlag(args :: []Text, flag :: Text) -> Bool` | `true` when the bare flag appears in `args`. The name works **with or without** a leading `--` (so `"verbose"` and `"--verbose"` both match an argument `"--verbose"`). |
| `cli.getOpt(args :: []Text, name :: Text) -> Result` | Collect the option's values (argv[0] skipped), recognising both `--name value` and `--name=value`; the name works with or without `--`. Returns `Ok([]Text)` of the values in argv order (an option may repeat), or `NotOk(name)` when no value is found — the name never appears, or appears only as a trailing `--name` with nothing after it. (The `--name=value` form always supplies a value, even the empty one in `--name=`.) |

```quilon
<< core.cli
^ = (args :: []Text, env :: [|Text => Text|]) -> Num => <
  home :: Text = cli.getEnv(env, "HOME") ? | Ok(v) => v | NotOk(_) => "?"
  verbose :: Bool = cli.hasFlag(args, "-v")
  outputs :: []Text = cli.getOpt(args, "--out") ? | Ok(vs) => vs | NotOk(_) => args.filter(x => false)
  verbose ? 0 : outputs.size
>
```

(See `examples/cli.qn`.)
