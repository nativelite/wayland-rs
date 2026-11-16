# Third-party notices

No third-party code is linked or vendored. Two files in this crate,
`src/protocol/wayland.rs` and `src/protocol/xdg_shell.rs`, are written by this
crate's own scanner from protocol descriptions published by the projects
below. What was taken from those descriptions is the interface, request,
event, argument and enum names, their order (the opcodes), the argument types,
and the enum values: the facts a client must match to speak the protocol.
None of the descriptive text was taken, and the XML files themselves are not
in this repository.

The descriptions carry the notices reproduced here.

## Wayland

- Project: Wayland, <https://gitlab.freedesktop.org/wayland/wayland>
- File: `protocol/wayland.xml`, release 1.24.0
- Licence: MIT (Expat)

```
Copyright © 2008-2012 Kristian Høgsberg
Copyright © 2010-2012 Intel Corporation
Copyright © 2011 Benjamin Franzke
Copyright © 2012 Collabora, Ltd.

Permission is hereby granted, free of charge, to any person obtaining a
copy of this software and associated documentation files (the "Software"),
to deal in the Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, sublicense,
and/or sell copies of the Software, and to permit persons to whom the
Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice (including the next
paragraph) shall be included in all copies or substantial portions of the
Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
```

`wayland.xml` itself names: Copyright © 2008-2011 Kristian Høgsberg,
Copyright © 2010-2011 Intel Corporation, Copyright © 2012-2013 Collabora, Ltd.

## wayland-protocols

- Project: wayland-protocols,
  <https://gitlab.freedesktop.org/wayland/wayland-protocols>
- File: `stable/xdg-shell/xdg-shell.xml`, release 1.45
- Licence: MIT (Expat)

```
Copyright © 2008-2013 Kristian Høgsberg
Copyright © 2010-2013 Intel Corporation
Copyright © 2013      Rafael Antognolli
Copyright © 2013      Jasper St. Pierre
Copyright © 2014      Jonas Ådahl
Copyright © 2014      Jason Ekstrand
Copyright © 2014-2015 Collabora, Ltd.
Copyright © 2015      Red Hat Inc.

Permission is hereby granted, free of charge, to any person obtaining a
copy of this software and associated documentation files (the "Software"),
to deal in the Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, sublicense,
and/or sell copies of the Software, and to permit persons to whom the
Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice (including the next
paragraph) shall be included in all copies or substantial portions of the
Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
```

`xdg-shell.xml` itself names: Copyright © 2008-2013 Kristian Høgsberg,
Copyright © 2013 Rafael Antognolli, Copyright © 2013 Jasper St. Pierre,
Copyright © 2010-2013 Intel Corporation, Copyright © 2015-2017 Samsung
Electronics Co., Ltd, Copyright © 2015-2017 Red Hat Inc.
