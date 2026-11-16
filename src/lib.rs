//! A Wayland client on the Rust standard library alone: what
//! `libwayland-client` does for a C program, without linking it.
//!
//! - [`wire`]: the wire format, message by message.
//! - [`Connection`]: the socket to the compositor, with file descriptors
//!   passed beside the bytes, the table of live objects, requests buffered
//!   until flushed, and events pulled one at a time. No callbacks, no
//!   threads, no global state.
//! - [`scan`]: the protocol scanner. It turns a protocol's XML into a Rust
//!   module of typed requests and events, as `wayland-scanner` does for C.
//! - [`protocol`]: the scanner's output for the core protocol and for
//!   xdg-shell, checked in.
//!
//! [`wire`] and [`scan`] build everywhere. [`Connection`] and [`protocol`]
//! are Linux only.
//!
//! ```no_run
//! # #[cfg(target_os = "linux")]
//! # fn main() -> Result<(), wayland::Error> {
//! use wayland::protocol::wayland::wl_registry;
//! use wayland::Connection;
//!
//! let mut conn = Connection::connect()?;
//! let registry = conn.display().get_registry(&mut conn)?;
//! conn.roundtrip()?;
//! while let Some(event) = conn.next_event()? {
//!     if event.is_for(&registry) {
//!         if let wl_registry::Event::Global { name, interface, version } =
//!             wl_registry::Event::decode(event)?
//!         {
//!             println!("{name}: {interface} v{version}");
//!         }
//!     }
//! }
//! # Ok(())
//! # }
//! # #[cfg(not(target_os = "linux"))]
//! # fn main() {}
//! ```

#![warn(missing_docs)]

#[cfg(target_os = "linux")]
mod conn;
mod error;
#[cfg(target_os = "linux")]
pub mod protocol;
pub mod scan;
pub mod wire;

#[cfg(target_os = "linux")]
pub use conn::{Arg, Connection, Event, Request};
pub use error::Error;

/// A typed handle to an object on a connection: its id, with the interface
/// known at compile time. The generated object types implement it.
///
/// A handle is only a number. It does not keep the object alive, and using
/// one after its object is destroyed is a protocol error the compositor
/// reports, as it is in C.
pub trait Proxy: Copy {
    /// The interface's message tables.
    fn interface() -> &'static wire::Interface;
    /// The object's id.
    fn id(&self) -> u32;
    /// The handle for the object with `id`. Nothing checks that the object
    /// exists or has this interface.
    fn from_id(id: u32) -> Self;
}
