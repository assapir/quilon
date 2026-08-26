# Quilon Runtime Library Exception

This file states the special exception that applies to the Quilon runtime
library (the `quilon-rt` crate) and to any runtime code that the Quilon
compiler incorporates into the programs it compiles.

It is an ADDITIONAL PERMISSION granted on top of version 2 of the GNU General
Public License (the "GPL", see LICENSE.md). It adds a permission; it removes
nothing. Wherever this exception is silent, the GPL governs.

This exception is modeled on the GNU Classpath exception used by the GNU
Compiler Collection and OpenJDK, adapted to name the Quilon runtime and the
runtime code the Quilon compiler emits into its output.


0. Definitions
--------------

"The Library" means the Quilon runtime library — the `quilon-rt` crate — in
source or compiled form (including the static library `libquilon_rt.a` and the
copy embedded in the `quilon` compiler binary), together with any runtime code
that the Quilon compiler emits into, or links with, the programs it compiles.
This includes, without limitation, the auto-generated C-compatible `main()`
wrapper and any other runtime boilerplate produced by the code generator.

"An independent module" is a module which is not derived from or based on the
Library — for example, a program you write in Quilon and compile with the
Quilon compiler.


1. The exception
----------------

Linking the Library statically or dynamically with other modules, or embedding
the Library (or compiler-emitted runtime code) into an executable, is making a
combined work based on the Library. Thus, the terms and conditions of the GPL
cover the whole combination.

As a special exception, the copyright holders of the Library give you
permission to combine the Library with independent modules to produce an
executable, regardless of the license terms of these independent modules, and
to copy and distribute the resulting executable under terms of your choice,
provided that you also meet, for each linked or embedded independent module,
the terms and conditions of the license of that module. An independent module
is a module which is not derived from or based on the Library. If you modify
the Library, you may extend this exception to your version of the Library, but
you are not obligated to do so. If you do not wish to do so, delete this
exception statement from your version.


2. What this does, and does not, do
-----------------------------------

This exception frees only the *combined output* — the executables you compile
with Quilon. A program you compile with Quilon is NOT brought under the GPL
merely because the GPL-licensed Quilon runtime, or compiler-emitted runtime
boilerplate, is linked or embedded into it. You may license and distribute your
compiled programs under any terms you choose.

This exception does NOT change the license of the Library itself, nor of the
Quilon compiler. The source code of `quilon-rt` and of the Quilon compiler
remains licensed under the GPL, version 2. If you fork, modify, or redistribute
the runtime or the compiler *as such*, that work remains subject to the GPL,
version 2 — the copyleft is intact.


3. Third-party runtime dependencies
------------------------------------

Compiled Quilon programs also carry the Boehm-Demers-Weiser conservative garbage
collector (libgc), which is a separate third-party work distributed under its
own permissive, MIT-style license. Its sources come from the
`quilon-rt/vendor/bdwgc` submodule (upstream terms in that repository's
`README.md`, under "Copyright & Warranty"); its compiled object is linked
statically into `libquilon_rt.a`, into the `quilon` binary, and into every
executable `quilon build` produces, so those artifacts distribute libgc in
binary form. libgc is not covered by, and does not
need, this exception; its own license applies to it.
