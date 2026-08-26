# Third-party notices

Quilon distributes the third-party components below in binary form. Their sources arrive as
git submodules, so a source archive of this repository would carry none of their notice
text — this file makes each notice travel with everything we ship.

## Boehm-Demers-Weiser conservative garbage collector (bdwgc)

Upstream: <https://github.com/ivmai/bdwgc>, carried at `quilon-rt/vendor/bdwgc` and pinned
to `v8.2.12` (commit `4fab5386df64466b2b61fc7209bef033cad1e6cc`). Compiled statically into
`libquilon_rt.a`, into the `quilon` binary, and into every executable `quilon build`
produces.

Reproduced verbatim from that commit's `README.md`, section "Copyright & Warranty":

```text
 * Copyright (c) 1988, 1989 Hans-J. Boehm, Alan J. Demers
 * Copyright (c) 1991-1996 by Xerox Corporation.  All rights reserved.
 * Copyright (c) 1996-1999 by Silicon Graphics.  All rights reserved.
 * Copyright (c) 1999-2011 by Hewlett-Packard Development Company.
 * Copyright (c) 2008-2025 Ivan Maidanski

The files pthread_stop_world.c, pthread_support.c and some others are also

 * Copyright (c) 1998 by Fergus Henderson.  All rights reserved.

The file include/gc.h is also

 * Copyright (c) 2007 Free Software Foundation, Inc

The files Makefile.am and configure.ac are

 * Copyright (c) 2001 by Red Hat Inc. All rights reserved.

The files extra/msvc_dbg.c and include/private/msvc_dbg.h are

 * Copyright (c) 2004-2005 Andrei Polushin

The file tests/initsecondarythread.c is

 * Copyright (c) 2011 Ludovic Courtes

The file tests/disclaim_weakmap_test.c is

 * Copyright (c) 2018 Petter A. Urkedal

Several files supporting GNU-style builds are copyrighted by the Free
Software Foundation, and carry a different license from that given
below.

THIS MATERIAL IS PROVIDED AS IS, WITH ABSOLUTELY NO WARRANTY EXPRESSED
OR IMPLIED.  ANY USE IS AT YOUR OWN RISK.

Permission is hereby granted to use or copy this program
for any purpose,  provided the above notices are retained on all copies.
Permission to modify the code and to distribute modified code is granted,
provided the above notices are retained, and a notice that the code was
modified is included with the above copyright notice.

A few of the files needed to use the GNU-style build procedure come with
slightly different licenses, though they are all similar in spirit.  A few
are GPL'ed, but with an exception that should cover all uses in the
collector. (If you are concerned about such things, I recommend you look
at the notice in config.guess or ltmain.sh.)
```
