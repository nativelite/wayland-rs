//! The Wayland wire format: message headers, arguments, and the tables that
//! describe an interface's requests and events.
//!
//! A message is a sequence of 32-bit words in the host's byte order: the
//! object id, then a word holding the message's size in bytes (upper 16
//! bits) and its opcode (lower 16), then the arguments. File descriptors
//! take no bytes here; they travel beside the stream. Nothing in this module
//! touches a socket, so it builds and is tested on every platform.

use std::fmt;

/// The length of a message header in bytes.
pub const HEADER_LEN: usize = 8;

/// The longest message the format can express: the size field is 16 bits
/// and a message is a whole number of words.
pub const MAX_MESSAGE_LEN: usize = 65532;

/// What can be wrong with bytes on the wire, or with a message being built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// A header's size is under eight bytes or not a multiple of four.
    BadLength(usize),
    /// An argument runs past the end of its message.
    Truncated,
    /// A string's last byte is not NUL.
    UnterminatedString,
    /// A null string, object or new id where the protocol allows none.
    Null,
    /// The object has no message with this opcode.
    UnknownOpcode(u16),
    /// A message needs a file descriptor and none arrived with it.
    MissingFd,
    /// The compositor announced a new object with an id it may not use.
    BadNewId(u32),
    /// A built message is longer than [`MAX_MESSAGE_LEN`].
    TooLong,
    /// A string to send has a NUL byte inside it.
    InteriorNul,
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WireError::BadLength(n) => write!(f, "message size {n} is not a valid length"),
            WireError::Truncated => f.write_str("argument runs past the end of its message"),
            WireError::UnterminatedString => f.write_str("string is not NUL-terminated"),
            WireError::Null => f.write_str("null value where the protocol allows none"),
            WireError::UnknownOpcode(op) => write!(f, "no message with opcode {op}"),
            WireError::MissingFd => f.write_str("message arrived without its file descriptor"),
            WireError::BadNewId(id) => write!(f, "compositor created an object with id {id:#x}"),
            WireError::TooLong => f.write_str("message is longer than the format allows"),
            WireError::InteriorNul => f.write_str("string has a NUL byte inside it"),
        }
    }
}

impl std::error::Error for WireError {}

/// A signed 24.8 fixed-point number, the format's fractional type.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Fixed(i32);

impl Fixed {
    /// The number whose wire representation is `raw` (the value times 256).
    pub const fn from_raw(raw: i32) -> Fixed {
        Fixed(raw)
    }

    /// The wire representation: the value times 256.
    pub const fn raw(self) -> i32 {
        self.0
    }

    /// The whole number `v`, saturating at the type's range.
    pub const fn from_int(v: i32) -> Fixed {
        Fixed(v.saturating_mul(256))
    }

    /// The value with its fraction dropped, rounding toward zero.
    pub const fn to_int(self) -> i32 {
        self.0 / 256
    }

    /// The nearest representable number to `v`, saturating at the range.
    pub fn from_f64(v: f64) -> Fixed {
        Fixed((v * 256.0).round() as i32)
    }

    /// The value as a float; every `Fixed` is exact in an `f64`.
    pub fn to_f64(self) -> f64 {
        f64::from(self.0) / 256.0
    }
}

impl fmt::Debug for Fixed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_f64())
    }
}

/// The type of one argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A signed 32-bit integer.
    Int,
    /// An unsigned 32-bit integer.
    Uint,
    /// A [`Fixed`].
    Fixed,
    /// A length-prefixed, NUL-terminated string, padded to a word.
    String,
    /// The id of an existing object.
    Object,
    /// The id of an object this message creates. With no interface named in
    /// the protocol it is preceded by the interface name and version.
    NewId,
    /// A length-prefixed byte array, padded to a word.
    Array,
    /// A file descriptor, sent beside the stream.
    Fd,
}

/// One argument of a message.
#[derive(Debug, Clone, Copy)]
pub struct ArgDesc {
    /// Its type.
    pub kind: Kind,
    /// Whether a string or object may be null.
    pub nullable: bool,
    /// For an object or new id, the interface the protocol names, if any.
    pub interface: Option<&'static Interface>,
}

/// One request or event of an interface.
#[derive(Debug, Clone, Copy)]
pub struct Message {
    /// Its name in the protocol.
    pub name: &'static str,
    /// The interface version that introduced it.
    pub since: u32,
    /// Whether the object is gone once it is sent (a request) or has
    /// arrived (an event).
    pub destructor: bool,
    /// Its arguments, in order.
    pub args: &'static [ArgDesc],
}

/// An interface: its name, version, and messages indexed by opcode.
#[derive(Clone, Copy)]
pub struct Interface {
    /// Its name in the protocol, e.g. `wl_surface`.
    pub name: &'static str,
    /// The highest version these tables describe.
    pub version: u32,
    /// Requests, client to compositor.
    pub requests: &'static [Message],
    /// Events, compositor to client.
    pub events: &'static [Message],
}

// Interfaces refer to each other in cycles, so this prints the name only.
impl fmt::Debug for Interface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} v{}", self.name, self.version)
    }
}

/// A message header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// The object the message is for.
    pub object: u32,
    /// Which of the object's messages it is.
    pub opcode: u16,
    /// The whole message's length in bytes, header included.
    pub len: usize,
}

/// The header at the start of `buf`: `None` until eight bytes are there,
/// an error if the size it gives cannot be a message's.
pub fn header(buf: &[u8]) -> Result<Option<Header>, WireError> {
    if buf.len() < HEADER_LEN {
        return Ok(None);
    }
    let object = u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let word = u32::from_ne_bytes([buf[4], buf[5], buf[6], buf[7]]);
    let len = (word >> 16) as usize;
    if len < HEADER_LEN || len % 4 != 0 {
        return Err(WireError::BadLength(len));
    }
    Ok(Some(Header {
        object,
        opcode: (word & 0xffff) as u16,
        len,
    }))
}

/// `n` rounded up to a whole number of words, or `None` on overflow.
fn padded(n: usize) -> Option<usize> {
    n.checked_add(3).map(|v| v & !3)
}

/// Reads arguments from a message body (the bytes after the header).
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// A reader at the start of `body`.
    pub fn new(body: &'a [u8]) -> Reader<'a> {
        Reader { buf: body, pos: 0 }
    }

    /// A reader over `body` that has already read `pos` bytes of it.
    pub fn at(body: &'a [u8], pos: usize) -> Reader<'a> {
        Reader {
            buf: body,
            pos: pos.min(body.len()),
        }
    }

    /// How many bytes have been read.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// How many bytes are left.
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        if n > self.remaining() {
            return Err(WireError::Truncated);
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    /// An unsigned integer (also an object id or a new id).
    pub fn uint(&mut self) -> Result<u32, WireError> {
        let b = self.take(4)?;
        Ok(u32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// A signed integer.
    pub fn int(&mut self) -> Result<i32, WireError> {
        Ok(self.uint()? as i32)
    }

    /// A fixed-point number.
    pub fn fixed(&mut self) -> Result<Fixed, WireError> {
        Ok(Fixed(self.int()?))
    }

    /// The bytes of a length-prefixed, padded block.
    fn block(&mut self) -> Result<&'a [u8], WireError> {
        let len = self.uint()? as usize;
        let whole = padded(len).ok_or(WireError::Truncated)?;
        if whole > self.remaining() {
            return Err(WireError::Truncated);
        }
        let s = &self.buf[self.pos..self.pos + len];
        self.pos += whole;
        Ok(s)
    }

    /// A string's bytes without the terminating NUL; `None` for a null
    /// string.
    pub fn string_bytes(&mut self) -> Result<Option<&'a [u8]>, WireError> {
        let s = self.block()?;
        match s.split_last() {
            None => Ok(None),
            Some((0, text)) => Ok(Some(text)),
            Some(_) => Err(WireError::UnterminatedString),
        }
    }

    /// A string, `None` for a null one. Bytes that are not UTF-8 become
    /// U+FFFD rather than an error: the protocol asks for UTF-8 but nothing
    /// enforces it.
    pub fn string(&mut self) -> Result<Option<String>, WireError> {
        Ok(self
            .string_bytes()?
            .map(|b| String::from_utf8_lossy(b).into_owned()))
    }

    /// A byte array.
    pub fn array(&mut self) -> Result<&'a [u8], WireError> {
        self.block()
    }
}

/// Checks that `body` holds the arguments `args` describe, the way
/// libwayland does before it hands a message on: every argument inside the
/// message, strings terminated, nothing null that may not be. `new_id` is
/// called for each new-id argument with the id and the interface the
/// protocol names. Returns how many file descriptors the message carries.
/// Bytes after the last argument are ignored.
pub fn validate(
    body: &[u8],
    args: &[ArgDesc],
    mut new_id: impl FnMut(u32, Option<&'static Interface>),
) -> Result<usize, WireError> {
    let mut r = Reader::new(body);
    let mut fds = 0;
    for a in args {
        match a.kind {
            Kind::Int | Kind::Uint | Kind::Fixed => {
                r.uint()?;
            }
            Kind::String => {
                if r.string_bytes()?.is_none() && !a.nullable {
                    return Err(WireError::Null);
                }
            }
            Kind::Object => {
                if r.uint()? == 0 && !a.nullable {
                    return Err(WireError::Null);
                }
            }
            Kind::NewId => {
                if a.interface.is_none() {
                    // An untyped new id is written as name, version, id.
                    if r.string_bytes()?.is_none() {
                        return Err(WireError::Null);
                    }
                    r.uint()?;
                }
                let id = r.uint()?;
                if id == 0 {
                    return Err(WireError::Null);
                }
                new_id(id, a.interface);
            }
            Kind::Array => {
                r.array()?;
            }
            Kind::Fd => fds += 1,
        }
    }
    Ok(fds)
}

/// Appends one message to a buffer. Dropping the builder without calling
/// [`finish`](Builder::finish) takes the partial message back out.
#[derive(Debug)]
pub struct Builder<'a> {
    buf: &'a mut Vec<u8>,
    start: usize,
    opcode: u16,
    done: bool,
}

impl<'a> Builder<'a> {
    /// Starts a message for `object` with `opcode` at the end of `buf`.
    pub fn new(buf: &'a mut Vec<u8>, object: u32, opcode: u16) -> Builder<'a> {
        let start = buf.len();
        buf.extend_from_slice(&object.to_ne_bytes());
        buf.extend_from_slice(&[0; 4]);
        Builder {
            buf,
            start,
            opcode,
            done: false,
        }
    }

    /// Where in the buffer this message starts.
    pub fn start(&self) -> usize {
        self.start
    }

    /// An unsigned integer (also an object id or a new id; 0 is null).
    pub fn uint(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_ne_bytes());
    }

    /// A signed integer.
    pub fn int(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_ne_bytes());
    }

    /// A fixed-point number.
    pub fn fixed(&mut self, v: Fixed) {
        self.int(v.raw());
    }

    fn pad(&mut self) {
        while (self.buf.len() - self.start) % 4 != 0 {
            self.buf.push(0);
        }
    }

    /// A string, or a null one for `None`. A string with a NUL byte inside
    /// cannot be sent and writes nothing.
    pub fn string(&mut self, s: Option<&str>) -> Result<(), WireError> {
        let Some(s) = s else {
            self.uint(0);
            return Ok(());
        };
        if s.as_bytes().contains(&0) {
            return Err(WireError::InteriorNul);
        }
        let len = u32::try_from(s.len() + 1).map_err(|_| WireError::TooLong)?;
        self.uint(len);
        self.buf.extend_from_slice(s.as_bytes());
        self.buf.push(0);
        self.pad();
        Ok(())
    }

    /// A byte array.
    pub fn array(&mut self, bytes: &[u8]) -> Result<(), WireError> {
        let len = u32::try_from(bytes.len()).map_err(|_| WireError::TooLong)?;
        self.uint(len);
        self.buf.extend_from_slice(bytes);
        self.pad();
        Ok(())
    }

    /// Completes the message and returns its length; if it is longer than
    /// the format allows, takes it back out of the buffer.
    pub fn finish(mut self) -> Result<usize, WireError> {
        let len = self.buf.len() - self.start;
        if len > MAX_MESSAGE_LEN {
            return Err(WireError::TooLong);
        }
        let word = ((len as u32) << 16) | u32::from(self.opcode);
        self.buf[self.start + 4..self.start + 8].copy_from_slice(&word.to_ne_bytes());
        self.done = true;
        Ok(len)
    }
}

impl Drop for Builder<'_> {
    fn drop(&mut self) {
        if !self.done {
            self.buf.truncate(self.start);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(ws: &[u32]) -> Vec<u8> {
        ws.iter().flat_map(|w| w.to_ne_bytes()).collect()
    }

    #[test]
    fn a_built_message_reads_back_argument_for_argument() {
        let mut buf = Vec::new();
        let mut b = Builder::new(&mut buf, 7, 3);
        b.int(-5);
        b.uint(0xdead_beef);
        b.fixed(Fixed::from_f64(-1.5));
        b.string(Some("héllo")).unwrap();
        b.string(None).unwrap();
        b.string(Some("")).unwrap();
        b.array(&[1, 2, 3, 4, 5]).unwrap();
        b.array(&[]).unwrap();
        let len = b.finish().unwrap();
        assert_eq!(len, buf.len());
        assert_eq!(len % 4, 0);

        let h = header(&buf).unwrap().unwrap();
        assert_eq!(
            h,
            Header {
                object: 7,
                opcode: 3,
                len
            }
        );
        let mut r = Reader::new(&buf[HEADER_LEN..]);
        assert_eq!(r.int().unwrap(), -5);
        assert_eq!(r.uint().unwrap(), 0xdead_beef);
        assert_eq!(r.fixed().unwrap().to_f64(), -1.5);
        assert_eq!(r.string().unwrap().as_deref(), Some("héllo"));
        assert_eq!(r.string().unwrap(), None);
        assert_eq!(r.string().unwrap().as_deref(), Some(""));
        assert_eq!(r.array().unwrap(), [1, 2, 3, 4, 5]);
        assert_eq!(r.array().unwrap(), [0u8; 0]);
        assert_eq!(r.remaining(), 0);
        assert_eq!(r.uint(), Err(WireError::Truncated));
    }

    #[test]
    fn strings_and_arrays_are_laid_out_as_the_protocol_says() {
        let mut buf = Vec::new();
        let mut b = Builder::new(&mut buf, 1, 0);
        b.string(Some("abc")).unwrap(); // 4 with NUL: no padding
        b.string(Some("abcd")).unwrap(); // 5 with NUL: three bytes of padding
        b.array(&[9]).unwrap();
        b.finish().unwrap();
        let mut want = words(&[1, 36 << 16, 4]);
        want.extend_from_slice(b"abc\0");
        want.extend_from_slice(&5u32.to_ne_bytes());
        want.extend_from_slice(b"abcd\0\0\0\0");
        want.extend_from_slice(&1u32.to_ne_bytes());
        want.extend_from_slice(&[9, 0, 0, 0]);
        assert_eq!(buf, want);
    }

    #[test]
    fn a_header_waits_for_eight_bytes_and_rejects_impossible_sizes() {
        assert_eq!(header(&[1, 2, 3]).unwrap(), None);
        assert_eq!(header(&[]).unwrap(), None);
        for bad in [0usize, 4, 7, 9, 10, 11, 65535] {
            let buf = words(&[1, (bad as u32) << 16]);
            assert_eq!(header(&buf), Err(WireError::BadLength(bad)), "{bad}");
        }
        let buf = words(&[5, (65532 << 16) | 0xffff]);
        let h = header(&buf).unwrap().unwrap();
        assert_eq!((h.object, h.opcode, h.len), (5, 0xffff, 65532));
    }

    #[test]
    fn malformed_strings_and_arrays_are_errors_not_panics() {
        // Length says 8, three bytes follow.
        let mut body = words(&[8]);
        body.extend_from_slice(b"abc");
        assert_eq!(Reader::new(&body).string(), Err(WireError::Truncated));
        assert_eq!(Reader::new(&body).array(), Err(WireError::Truncated));
        // The content is there but its padding is not.
        let mut body = words(&[5]);
        body.extend_from_slice(b"abcd\0");
        assert_eq!(Reader::new(&body).string(), Err(WireError::Truncated));
        // No NUL at the end.
        let mut body = words(&[4]);
        body.extend_from_slice(b"abcd");
        assert_eq!(
            Reader::new(&body).string(),
            Err(WireError::UnterminatedString)
        );
        // A length near the top of the range must not overflow.
        for len in [u32::MAX, u32::MAX - 1, u32::MAX - 3, 0x8000_0000] {
            let body = words(&[len, 0]);
            assert_eq!(Reader::new(&body).string(), Err(WireError::Truncated));
            assert_eq!(Reader::new(&body).array(), Err(WireError::Truncated));
        }
        // Half a word.
        assert_eq!(Reader::new(&[1, 2]).uint(), Err(WireError::Truncated));
        // Bytes that are not UTF-8 are replaced, not refused.
        let mut body = words(&[3]);
        body.extend_from_slice(&[0xff, b'a', 0, 0]);
        assert_eq!(
            Reader::new(&body).string().unwrap().as_deref(),
            Some("\u{fffd}a")
        );
        // Padding bytes may hold anything.
        let mut body = words(&[2]);
        body.extend_from_slice(&[b'a', 0, 0x55, 0xAA]);
        assert_eq!(Reader::new(&body).string().unwrap().as_deref(), Some("a"));
    }

    #[test]
    fn a_message_too_long_for_the_format_is_taken_back_out() {
        let mut buf = vec![1, 2, 3, 4];
        let mut b = Builder::new(&mut buf, 1, 0);
        b.array(&vec![0; 70_000]).unwrap();
        assert_eq!(b.finish(), Err(WireError::TooLong));
        assert_eq!(buf, [1, 2, 3, 4]);

        // The longest message that fits: 8 header + 4 length + 65520 bytes.
        let mut b = Builder::new(&mut buf, 1, 0);
        b.array(&vec![0; 65520]).unwrap();
        assert_eq!(b.finish(), Ok(MAX_MESSAGE_LEN));
        buf.truncate(4);
        let mut b = Builder::new(&mut buf, 1, 0);
        b.array(&vec![0; 65521]).unwrap();
        assert_eq!(b.finish(), Err(WireError::TooLong));

        // Dropped unfinished, and a string that cannot be sent.
        let mut b = Builder::new(&mut buf, 1, 0);
        assert_eq!(b.string(Some("a\0b")), Err(WireError::InteriorNul));
        b.uint(1);
        drop(b);
        assert_eq!(buf, [1, 2, 3, 4]);
    }

    #[test]
    fn fixed_point_converts_both_ways() {
        assert_eq!(Fixed::from_int(3).raw(), 768);
        assert_eq!(Fixed::from_raw(-384).to_f64(), -1.5);
        assert_eq!(Fixed::from_raw(-384).to_int(), -1);
        assert_eq!(Fixed::from_raw(383).to_int(), 1);
        assert_eq!(Fixed::from_f64(0.004).raw(), 1);
        assert_eq!(Fixed::from_f64(1e12).raw(), i32::MAX);
        assert_eq!(Fixed::from_f64(f64::NAN).raw(), 0);
        assert_eq!(Fixed::from_int(i32::MAX).raw(), i32::MAX);
        assert_eq!(format!("{:?}", Fixed::from_raw(64)), "0.25");
    }

    static LEAF: Interface = Interface {
        name: "leaf",
        version: 1,
        requests: &[],
        events: &[],
    };

    fn arg(kind: Kind, nullable: bool, interface: Option<&'static Interface>) -> ArgDesc {
        ArgDesc {
            kind,
            nullable,
            interface,
        }
    }

    #[test]
    fn validation_counts_fds_reports_new_ids_and_refuses_nulls() {
        let args = [
            arg(Kind::Uint, false, None),
            arg(Kind::Fd, false, None),
            arg(Kind::String, true, None),
            arg(Kind::NewId, false, Some(&LEAF)),
            arg(Kind::Object, true, None),
            arg(Kind::Fd, false, None),
            arg(Kind::Array, false, None),
        ];
        let body = words(&[1, 0, 0xff00_0000, 0, 0]);
        let mut seen = Vec::new();
        let fds = validate(&body, &args, |id, i| seen.push((id, i.map(|i| i.name)))).unwrap();
        assert_eq!(fds, 2);
        assert_eq!(seen, [(0xff00_0000, Some("leaf"))]);
        // Every truncation of it is an error.
        for cut in 0..body.len() {
            assert!(validate(&body[..cut], &args, |_, _| ()).is_err(), "{cut}");
        }
        // Trailing bytes are ignored, as libwayland ignores them.
        let mut long = body.clone();
        long.extend_from_slice(&[1, 2, 3, 4]);
        assert_eq!(validate(&long, &args, |_, _| ()), Ok(2));

        let one = |a: ArgDesc, ws: &[u32]| validate(&words(ws), &[a], |_, _| ());
        assert_eq!(
            one(arg(Kind::String, false, None), &[0]),
            Err(WireError::Null)
        );
        assert_eq!(
            one(arg(Kind::Object, false, None), &[0]),
            Err(WireError::Null)
        );
        assert_eq!(
            one(arg(Kind::NewId, false, Some(&LEAF)), &[0]),
            Err(WireError::Null)
        );
        // An untyped new id: name, version, id.
        let mut body = words(&[5]);
        body.extend_from_slice(b"leaf\0\0\0\0");
        body.extend_from_slice(&words(&[3, 9]));
        let mut seen = Vec::new();
        validate(&body, &[arg(Kind::NewId, false, None)], |id, i| {
            seen.push((id, i.is_none()))
        })
        .unwrap();
        assert_eq!(seen, [(9, true)]);
    }

    #[test]
    fn an_interface_prints_its_name_not_its_cycle() {
        assert_eq!(format!("{LEAF:?}"), "leaf v1");
    }
}
