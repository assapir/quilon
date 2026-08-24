~ core.cli — helpers over the `^` entry point's `args :: []Text` and
~ `env :: [][]Text` (env is an array of `[key, value]` pairs). Import with
~ `<< core.cli`. Exposes:
~   getEnv(env, key)    look up an environment variable by name
~   hasFlag(args, flag) is a bare flag present in argv?
~   getOpt(args, name)  read a `--name value` / `--name=value` option's values
~
~ Everything is pipe-friendly: the data is the FIRST parameter, so
~ `env |> getEnv("PATH")` and `args |> hasFlag("-v")` read naturally.
~
~ Example:
~   << core.cli
~   ^ = (args :: []Text, env :: [][]Text) -> Num => <
~     home :: Text = env |> getEnv("HOME") ? | Ok(v) => v | NotOk(_) => "?"
~     args |> hasFlag("-v") ? 0 : 1
~     args |> getOpt("--out") ? | Ok(_) => 0 | NotOk(_) => 1
~   >

~ Look up `key` in the `[][]Text` env (each inner array is a `[name, value]`
~ pair). Returns `Ok(value)` (the pair's `[1]`) when a pair's `[0]` equals `key`,
~ else `NotOk`.
>> getEnv = (env :: [][]Text, key :: Text) -> Result => <
  env.find(pair => pair[0] == key) ?
    | Ok(pair) => Ok(pair[1])
    | NotOk(_) => NotOk($)
>

~ True when the bare flag `flag` appears in `args`. The flag name works WITH or
~ WITHOUT a leading `--`, so `hasFlag(args, "verbose")` and
~ `hasFlag(args, "--verbose")` both match an arg `"--verbose"`.
>> hasFlag = (args :: []Text, flag :: Text) -> Bool => <
  wanted :: Text = flag.slice(0, 2) == "--" ? flag : "--" + flag
  args.find(arg => arg == flag || arg == wanted) ?
    | Ok(_)    => true
    | NotOk(_) => false
>

~ Read the values of the option `name` from `args`, skipping argv[0]. Both forms
~ are recognised: `--name value` (the following argument) and `--name=value` (the
~ text after the first `=`). The name works with OR without a leading `--`. Since
~ an option may be repeated, the collected values are returned as `Ok([]Text)` (in
~ argv order). `NotOk(name)` is returned when no value is found — either the name
~ never appears, or it appears only as a trailing `--name` with nothing after it
~ (the `--name=value` form always supplies a value, even the empty one in `--name=`).
~ Parsing is positional (no flag registry): the token right after a space-form
~ `--name` is taken as its value even if that token itself looks like an option.
>> getOpt = (args :: []Text, name :: Text) -> Result => <
  wanted :: Text = name.slice(0, 2) == "--" ? name : "--" + name
  ~ The option name of a token: the text before the first `=`, else the whole token.
  optKey = (tok :: Text) -> Text => tok.indexOf("=") ? | Ok(p) => tok.slice(0, p) | NotOk(_) => tok
  ~ The `--name=value` value of a token: the text after the first `=`, else empty.
  optVal = (tok :: Text) -> Text => tok.indexOf("=") ? | Ok(p) => tok.slice(p + 1, tok.size) | NotOk(_) => ""
  ~ Does a token name this option (in either form, with or without `--`)?
  matches = (tok :: Text) -> Bool => optKey(tok) == name || optKey(tok) == wanted
  ~ Indices into args, skipping argv[0].
  idx :: []Num = (0 <- (args.size - 1)).filter(i => i > 0)
  empty :: []Text = args.filter(x => false)
  vals :: []Text = idx.reduce(empty, (acc, i) =>
    matches(args[i])
      ? (args[i].contains("=")
          ? acc + [optVal(args[i])]
          : (i + 1 < args.size ? acc + [args[i + 1]] : acc))
      : acc)
  vals.size > 0 ? Ok(vals) : NotOk(name)
>
