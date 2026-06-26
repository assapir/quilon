~ core.text — the Text string type.
~
~ Imported with `<< core.text`. Text is a built-in value type represented as
~ { ptr data, i64 byte_len }; its operations are *compiler-lowered* (like
~ core.io's print), so they are available on any Text value:
~
~   "héllo".size      ~ byte length        (Num)
~   "héllo".length    ~ grapheme clusters  (Num) — full UTF-8, user-perceived chars
~   "a" + "b"         ~ concatenation      (Text), GC-allocated
~
~ This module documents that surface and reserves the `core.text` name. The
~ operations do not require the import today (they are intrinsic, like an
~ array's `.size`); the import exists for discoverability and forward
~ compatibility as the Text API grows.
