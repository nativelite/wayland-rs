# Decision record: a Wayland client of our own

*2026-11-15. Status: built as 0.1.0, unpublished.*

## The need

foldwave's window on Linux has to speak Wayland. The usual way is to link
`libwayland-client`, a C library. nativelite packages link nothing but the
language's standard library, so the client is written here.

## Prior art

| Project | Licence | What it is |
| --- | --- | --- |
| libwayland-client (<https://gitlab.freedesktop.org/wayland/wayland>) | MIT | The reference client library, in C. Proxies with listener callbacks, event queues, a scanner that writes C from protocol XML. |
| wayland-rs (<https://github.com/Smithay/wayland-rs>) | MIT | Rust. Either binds libwayland or uses its own Rust implementation of the wire; brings a tree of dependencies and a `Dispatch`-trait model. |

(The descriptions above are from general knowledge of the two projects, not
re-verified against their current sources for this record.)

Both are permissively licensed. Neither is used as code: this is a
reimplementation from the protocol's published wire format, with libwayland's
observable behaviour as the reference where the specification is silent
(what to do with an event for an unknown object, how file descriptors are
batched, how ids are reused).

## Is their way the best way for us?

**Callbacks.** libwayland delivers events by calling listener functions
registered per object, and wayland-rs by calling trait methods. Both exist to
fit an event model into languages and programs with many owners. foldwave's
window has one loop and one owner. Here the caller pulls events as values
(`next_event`, `wait_event`) and matches on them, so there is no registration,
no user data pointer, no queue per thread and no re-entrancy to reason about.
A callback API can be built on top of a pull API; the reverse is awkward.

**Object lifetime.** A typed handle is a `Copy` number, as an id is on the
wire. The connection keeps the one table that has to exist (id to interface
and version), which is what decodes events and what tells a destroyed object's
late events from live ones.

**Generated code.** Like both, protocols are generated from XML by a scanner,
because the alternative is typing opcodes by hand. Unlike libwayland the
scanner is a library function as well as a command, and the output is checked
in for the core protocol and xdg-shell, so building the crate runs no code
generation.

## What this cannot do: hand Vulkan a `wl_display*`

Vulkan's Wayland surface extension (`VK_KHR_wayland_surface`) takes
`struct wl_display *` and `struct wl_surface *` and the driver then calls
libwayland on them. A client that does not link libwayland has no such
pointers to give. This is a limit of the approach, not of this version.

The two ways to put pixels on a surface without it:

- **`wl_shm`**: a shared-memory buffer the client fills. Core protocol,
  works everywhere, and is the CPU-rendering fallback foldwave has anyway.
- **`linux-dmabuf`** (`zwp_linux_dmabuf_v1`, wayland-protocols): render with
  Vulkan into images whose memory is exported as dmabuf descriptors
  (`VK_KHR_external_memory_fd`, `VK_EXT_external_memory_dma_buf`,
  `VK_EXT_image_drm_format_modifier`), and attach those as buffers. This is
  the zero-copy path; it means foldwave owns the swapchain logic the
  extension would otherwise provide.

Which of the two foldwave ships first is a foldwave design decision
(`docs/design.md` §8) and is open. Neither needs anything from this crate
beyond the scanner run on the dmabuf protocol.

## Proof

- The wire format is checked byte for byte in unit tests, including every
  malformed case found (truncation, overlong lengths, missing terminator).
- The connection is driven against a compositor played by the tests over a
  socket pair: descriptors in both directions, more descriptors than one
  write carries, id reuse, compositor-created objects, protocol errors,
  events split at every byte, noise.
- The scanner's output for a test protocol is compiled and driven as a
  user's module would be.
- A live test runs against the real compositor on the machine when there is
  one (verified on WSLg's Weston), taking an `xdg_toplevel` to its first
  configure without ever mapping a window.

## Not in 0.1

- Platforms other than Linux on x86-64, x86, AArch64, ARM and RISC-V 64: the
  socket calls are declared by hand and their constants are per platform.
- Protocols other than core and xdg-shell are not checked in; run the scanner.
- No check that a request is allowed at the object's bound version; the
  `*_SINCE` constants are there for the caller.
