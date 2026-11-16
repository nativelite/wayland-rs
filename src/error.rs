//! The crate's error type.

use crate::wire::WireError;
use std::fmt;
use std::io;

/// What can go wrong talking to a compositor.
#[derive(Debug)]
pub enum Error {
    /// The socket failed.
    Io(io::Error),
    /// Bytes from the compositor were not a valid message, or a request
    /// could not be encoded.
    Wire(WireError),
    /// The compositor reported a protocol error (`wl_display.error`) and
    /// will close the connection.
    Protocol {
        /// The id of the object the error is about.
        object: u32,
        /// That object's interface, or `"unknown"`.
        interface: &'static str,
        /// The error code, from that interface's `error` enum.
        code: u32,
        /// The compositor's description.
        message: String,
    },
    /// The compositor closed the connection.
    Closed,
    /// No compositor could be found: the reason.
    NoCompositor(String),
    /// An event was decoded as an interface it does not belong to.
    WrongInterface {
        /// The interface the decoder is for.
        expected: &'static str,
        /// The interface of the event's object.
        found: &'static str,
    },
    /// No object ids are left, or a request creates more objects than this
    /// crate supports in one message.
    TooManyObjects,
    /// A bind asked for a version of an interface that this crate's tables
    /// do not describe.
    UnsupportedVersion {
        /// The interface.
        interface: &'static str,
        /// The version asked for.
        version: u32,
        /// The highest version the tables describe.
        supported: u32,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "wayland socket: {e}"),
            Error::Wire(e) => write!(f, "wayland message: {e}"),
            Error::Protocol {
                object,
                interface,
                code,
                message,
            } => write!(
                f,
                "wayland protocol error {code} on {interface}#{object}: {message}"
            ),
            Error::Closed => f.write_str("the compositor closed the connection"),
            Error::NoCompositor(why) => write!(f, "no wayland compositor: {why}"),
            Error::WrongInterface { expected, found } => {
                write!(f, "event of {found} decoded as {expected}")
            }
            Error::TooManyObjects => f.write_str("out of wayland object ids"),
            Error::UnsupportedVersion {
                interface,
                version,
                supported,
            } => write!(
                f,
                "cannot bind {interface} at version {version}: versions 1 to {supported} are known"
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            Error::Wire(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Error {
        Error::Io(e)
    }
}

impl From<WireError> for Error {
    fn from(e: WireError) -> Error {
        Error::Wire(e)
    }
}
