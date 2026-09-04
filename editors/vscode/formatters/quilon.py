# Quilon value formatters for LLDB / CodeLLDB, imported by the VS Code extension.
#
# A `--debug` build (`src/codegen/generator/di.rs::emit_render_thunk`) emits one exported
# C-ABI thunk per Quilon type that gets a DWARF variable: `const char*
# __qn_render$<name>(const void* slot)`. It loads the value at `slot` (the variable's own
# storage) and renders it through the SAME `` ` `` path `io.print`/interpolation use, so the
# debugger shows exactly what `print`ing the value would — a user override, a record's type
# name, an array, or a Map/Set's entries. `render_summary` below evaluates that call IN THE
# STOPPED PROGRAM for every Quilon-shaped DWARF type; `sanitize_debug_type_name` derives the
# thunk's symbol from `DW_AT_name`, the only thing a debugger has to go on — it is the exact
# same transform `src/codegen/debug.rs::sanitize_debug_type_name` applies on the compiler
# side (see the shared example table both test suites check), and the two MUST stay in
# lockstep, or a value would call the wrong type's thunk.
#
# Also handles the DWARF composite shapes directly (no thunk call needed) for child
# expansion:
#   Text -> struct { char* data; i64 byte_len }   (NUL-terminated UTF-8)
#   []T  -> struct { T*    data; i64 size }        (name is "[]Num", "[][]Text", …)

try:
    import lldb
except ImportError:  # pragma: no cover - only importable inside an lldb session
    lldb = None

# Element children beyond this cap are hidden from an array's default expansion (still
# reachable by explicit index); the overflow count shows in the summary.
ELEMENT_CAP = 200
# Upper bound on the C string a render thunk hands back, guarding a corrupt/huge value from
# driving an unbounded read.
RENDER_BYTE_CAP = 1 << 20
# How long a render-thunk evaluation may run in the debuggee before giving up.
RENDER_TIMEOUT_MICROS = 2 * 1000 * 1000


def sanitize_debug_type_name(name):
    """The suffix a render-thunk symbol carries for the DWARF display name `name` (e.g.
    `"Map[Text, Num]"`, `"[]Num"`, `"Point"`, `"$"`). Alphanumerics, `_`, and `$` pass
    through unchanged; `[` and `,` each become their own `$` separator (so a nested
    `[][]Text`'s two `[`s each contribute a separator, rather than collapsing into one and
    colliding with a single-level `[]Text`); every other character (`]`, spaces, the `.` in
    a qualified name) is dropped. Mirrors `sanitize_debug_type_name` in
    `src/codegen/debug.rs` exactly — see that function's doc comment."""
    out = []
    for c in name:
        if c.isalnum() or c in ("_", "$"):
            out.append(c)
        elif c in ("[", ","):
            out.append("$")
    return "".join(out)


def render_thunk_symbol(debug_name):
    """The C-ABI render-thunk symbol for a type whose DWARF display name is `debug_name`."""
    return "__qn_render$" + sanitize_debug_type_name(debug_name)


def render_summary(valobj, _internal_dict):
    """The one-line summary for a Quilon COMPOSITE value (`Text`, an array, a record, a sum,
    a Map/Set): call its `__qn_render$<type>` thunk in the stopped debuggee and return what
    it renders. Returns `""` — an empty summary, leaving lldb's own display alone — for a
    bare scalar/builtin type (`Num`/`Bool`/Unit), whenever the type has no thunk (a
    non-Quilon type this pattern also matched, or a `--debug` build too old to carry one),
    or the evaluation fails for any reason; this function never raises.

    Deliberately `""`, not `None`: confirmed live, a registered summary function returning
    `None` makes lldb print the literal text "None" after the value, rather than showing
    nothing extra — `""` is the actual "no summary" answer here.

    Bare scalars are excluded deliberately, not just left unmatched: lldb shows a scalar's
    OWN natural value ALONGSIDE any summary a formatter returns (`true True`, `7 7`,
    confirmed live) rather than replacing it, so a rendered `Num`/`Bool` would only ever
    look redundant, never additive — unlike a composite, which lldb shows via the summary
    ALONE. `GetTypeClass()` is checked rather than the type name, since lldb canonicalizes a
    bare scalar's OWN displayed name from its (encoding, size) alone — confirmed live, an
    `f64` reads back as `"double"`, not `"Num"` — so name-matching bare scalars reliably
    would need to enumerate lldb's own names one by one; the type CLASS needs no such list.
    """
    try:
        raw = valobj.GetNonSyntheticValue()
        ty = raw.GetType()
        if ty.GetTypeClass() == lldb.eTypeClassBuiltin:
            return ""
        type_name = ty.GetName()
        if not type_name:
            return ""
        frame = raw.GetFrame()
        if frame is None or not frame.IsValid():
            return ""
        addr = raw.GetLoadAddress()
        if addr in (None, lldb.LLDB_INVALID_ADDRESS):
            return ""
        symbol = render_thunk_symbol(type_name)
        expr = "(const char*)%s((const void*)0x%xULL)" % (symbol, addr)
        options = lldb.SBExpressionOptions()
        options.SetTimeoutInMicroSeconds(RENDER_TIMEOUT_MICROS)
        options.SetTryAllThreads(False)
        result = frame.EvaluateExpression(expr, options)
        if not result.IsValid() or result.GetError().Fail():
            return ""
        ptr = result.GetValueAsUnsigned(0)
        if ptr == 0:
            return ""
        error = lldb.SBError()
        text = raw.GetProcess().ReadCStringFromMemory(ptr, RENDER_BYTE_CAP, error)
        if not error.Success():
            return ""
        return text
    except Exception:
        return ""


class TextChildrenProvider:
    """A Text is a leaf: its summary is the rendered string, not the {data, byte_len} struct."""

    def __init__(self, _valobj, _internal_dict):
        pass

    def num_children(self):
        return 0

    def get_child_index(self, _name):
        return -1

    def get_child_at_index(self, _index):
        return None

    def update(self):
        return False


class ArrayChildrenProvider:
    """Expose `data[i]` as `[i]` children, each typed as the element type so
    Text/nested-array elements format too. The default expansion is capped at
    ELEMENT_CAP, but an explicit `arr[i]` past the cap still resolves."""

    def __init__(self, valobj, _internal_dict):
        self.valobj = valobj
        self.data = None
        self.size = 0
        self.elem_type = None
        self.elem_size = 0

    def update(self):
        self.data = self.valobj.GetChildMemberWithName("data")
        size = self.valobj.GetChildMemberWithName("size")
        n = size.GetValueAsSigned(0) if size.IsValid() else 0
        if self.data.IsValid():
            self.elem_type = self.data.GetType().GetPointeeType()
            self.elem_size = self.elem_type.GetByteSize()
        else:
            self.elem_type = None
            self.elem_size = 0
        self.size = max(0, n) if self.elem_size else 0
        return False

    def num_children(self):
        return min(self.size, ELEMENT_CAP)

    def get_child_index(self, name):
        try:
            return int(name.strip("[]"))
        except ValueError:
            return -1

    def get_child_at_index(self, index):
        if index < 0 or index >= self.size or self.elem_size == 0:
            return None
        addr = self.data.GetValueAsUnsigned(0) + index * self.elem_size
        return self.valobj.CreateValueFromAddress("[{}]".format(index), addr, self.elem_type)


def __lldb_init_module(debugger, _internal_dict):
    # ONE summary provider, `render_summary`, registered against every type name (`-x ".*"`)
    # — it is the one place that decides whether a value is worth rendering (a Quilon
    # composite) or not (a bare scalar, or some unrelated type this catch-all also matches,
    # in either case falling back to lldb's own default display). A narrower set of
    # per-shape patterns was tried and dropped: it cannot single out "every Quilon composite"
    # without also either matching bare scalars (which then show a redundant double display,
    # see `render_summary`'s doc comment) or maintaining an explicit exception list, where
    # the type-CLASS check `render_summary` already needs to do this correctly makes the
    # registration pattern itself need no such precision.
    #
    # Children (expansion) providers are separate and unchanged: Text stays a leaf, and an
    # array still expands into `[i]` elements; a record's fields expand via the DWARF struct
    # with no synthetic provider needed, and Map/Set carry no children (only the summary).
    commands = [
        'type summary add -x ".*" -F quilon.render_summary',
        'type synthetic add -l quilon.TextChildrenProvider Text',
        r'type synthetic add -x "^\[\]" -l quilon.ArrayChildrenProvider',
    ]
    for command in commands:
        debugger.HandleCommand(command)
