# wayland-rs

A Wayland client in Rust: what `libwayland-client` does for a C program,
written on the Rust standard library and nothing else. It does not link
libwayland.

- **Zero runtime dependencies.** `[dependencies]` is empty and stays empty.
  The socket calls the standard library lacks (`sendmsg`, `recvmsg`, `poll`)
  are declared against the C library the standard library already links.
- **One concern:** speaking the Wayland protocol as a client.

Crate `nativelite-wayland`, library name `wayland`. Version 0.1.0, not yet
published to crates.io.

## What is in it

| Part | What it does |
| --- | --- |
| `wayland::wire` | The wire format: headers, arguments, 24.8 fixed point, building and reading messages, validation against a signature. Every platform. |
| `wayland::Connection` | The socket to the compositor (`WAYLAND_SOCKET`, `WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR`), file descriptors passed both ways with `SCM_RIGHTS`, the table of live objects with id reuse, buffered requests, events pulled one at a time, `roundtrip`. Linux. |
| `wayland::scan` | The protocol scanner: protocol XML in, a Rust module of typed requests and events out. A library function and `examples/scan.rs`. Every platform. |
| `wayland::protocol` | The scanner's output, checked in, for the core protocol (wayland 1.24.0) and xdg-shell (wayland-protocols 1.45). Linux. |

Events are pulled, not delivered: there are no listener callbacks, no event
queues per thread and no global state. The caller owns the loop.

## Example

List what the running compositor offers (`cargo run --example globals`):

```rust
use wayland::protocol::wayland::wl_registry;
use wayland::Connection;

fn main() -> Result<(), wayland::Error> {
    let mut conn = Connection::connect()?;
    let registry = conn.display().get_registry(&mut conn)?;
    // One trip to the compositor and back: every global it has is announced
    // before the reply.
    conn.roundtrip()?;
    while let Some(event) = conn.next_event()? {
        if !event.is_for(&registry) {
            continue;
        }
        if let wl_registry::Event::Global { name, interface, version } =
            wl_registry::Event::decode(event)?
        {
            println!("{name:>3}  {interface} v{version}");
        }
    }
    Ok(())
}
```

In a program with its own loop, poll the connection's descriptor
(`Connection` implements `AsRawFd`), call `read()` when it is readable, drain
`next_event()`, and `flush()` after sending requests. `tests/live.rs` takes an
`xdg_toplevel` to its first configure and is the shortest complete session.

## Other protocols

Anything in wayland-protocols, or a compositor's own, is one run of the
scanner:

```text
cargo run --example scan -- linux-dmabuf-v1.xml \
    --import ::wayland::protocol::wayland --out src/linux_dmabuf.rs
```

and in the crate that uses it:

```rust,ignore
#[rustfmt::skip]
mod linux_dmabuf;
```

The generated module carries the protocol's names, opcodes, argument types
and enum values, and none of its descriptive text.

## What it does not do

- **It cannot give Vulkan a `wl_display*`.** Vulkan's Wayland surface
  extension needs pointers from the real libwayland. A program using this
  crate presents through `wl_shm` or `linux-dmabuf` buffers instead. See
  `docs/decision.md`.
- **Linux only**, on x86-64, x86, AArch64, ARM and RISC-V 64. The socket
  constants are per platform and only these are filled in; the BSDs are not
  supported. `wire` and `scan` build anywhere.
- No check that a request exists at the version an object was bound with; the
  generated `REQ_*_SINCE` / `EVT_*_SINCE` constants say when each arrived.
- A bind is refused (`Error::UnsupportedVersion`) above the version the
  generated tables describe: bind the lower of `T::interface().version` and
  what the compositor advertises.
- No 4096-byte cap on a request. A compositor built on libwayland older than
  1.23 drops a client that sends a larger message; this crate sends anything
  the format allows (65532 bytes).
- Handles are plain ids. Using one after destroying its object is a protocol
  error the compositor reports, as in C.
- With `WAYLAND_SOCKET` set, `Connection::connect` removes the variable from
  the environment, as libwayland does. Call it before starting threads that
  read the environment.

## Development

Python 3.11+ and a Rust toolchain (1.70 or newer); nothing else.

```text
python dev.py check    # dependency guard + cargo fmt --check + cargo test
python dev.py test     # cargo test
```

`check` is the gate before every push. On Windows the wire format and the
scanner are all that builds, so `check` there goes on to run itself inside
WSL (and says so loudly if there is no WSL); on Linux it covers the connection, against a compositor
played by the tests and, when one is running, against the real one. The live
test never attaches a buffer, so no window appears; with no compositor it
prints that it was skipped and passes.

## Licence

MIT. Notices for the protocol descriptions the checked-in modules were
generated from are in `THIRD-PARTY-NOTICES.md`.
