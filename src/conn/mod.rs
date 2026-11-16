//! The connection to a compositor: the socket, the objects alive on it, the
//! requests waiting to go out and the events that have come in.

mod sys;
#[cfg(test)]
mod tests;

use crate::protocol::wayland::{wl_callback, wl_display, WlDisplay};
use crate::wire::{self, Builder, Fixed, Interface, Kind, Reader, WireError};
use crate::{Error, Proxy};
use std::collections::VecDeque;
use std::env;
use std::fmt;
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Ids from here up are the compositor's to hand out.
const SERVER_ID_BASE: u32 = 0xff00_0000;

/// One object on the connection.
#[derive(Debug, Clone, Copy)]
struct Slot {
    interface: &'static Interface,
    version: u32,
    /// Destroyed by this side, its id not yet released by the compositor:
    /// events still on their way to it are dropped.
    dead: bool,
}

/// The objects alive on a connection, by id.
#[derive(Debug, Default)]
struct Objects {
    /// Ids this side allocated; index is the id, 0 is never used.
    client: Vec<Option<Slot>>,
    /// Client ids the compositor has released, reused last-freed first.
    free: Vec<u32>,
    /// Ids the compositor allocated; index is the id less the base.
    server: Vec<Option<Slot>>,
}

impl Objects {
    fn get(&self, id: u32) -> Option<&Slot> {
        if id >= SERVER_ID_BASE {
            self.server.get((id - SERVER_ID_BASE) as usize)?.as_ref()
        } else {
            self.client.get(id as usize)?.as_ref()
        }
    }

    /// A fresh client id for an object of `interface`.
    fn alloc(&mut self, interface: &'static Interface, version: u32) -> Result<u32, Error> {
        let slot = Some(Slot {
            interface,
            version,
            dead: false,
        });
        if let Some(id) = self.free.pop() {
            self.client[id as usize] = slot;
            return Ok(id);
        }
        if self.client.is_empty() {
            self.client.push(None);
        }
        let id = self.client.len() as u64;
        if id >= u64::from(SERVER_ID_BASE) {
            return Err(Error::TooManyObjects);
        }
        self.client.push(slot);
        Ok(id as u32)
    }

    /// Takes back an id handed out by `alloc` that was never sent.
    fn unalloc(&mut self, id: u32) {
        if let Some(slot) = self.client.get_mut(id as usize) {
            if slot.take().is_some() {
                self.free.push(id);
            }
        }
    }

    /// Records an object the compositor created. Like libwayland, accepts
    /// only an id in the compositor's range that is at most one past the
    /// highest seen, so a hostile id cannot size the table.
    fn insert_server(
        &mut self,
        id: u32,
        interface: &'static Interface,
        version: u32,
    ) -> Result<(), WireError> {
        let index = id
            .checked_sub(SERVER_ID_BASE)
            .ok_or(WireError::BadNewId(id))? as usize;
        let slot = Some(Slot {
            interface,
            version,
            dead: false,
        });
        match index.cmp(&self.server.len()) {
            std::cmp::Ordering::Less => self.server[index] = slot,
            std::cmp::Ordering::Equal => self.server.push(slot),
            std::cmp::Ordering::Greater => return Err(WireError::BadNewId(id)),
        }
        Ok(())
    }

    /// This side destroyed `id`. A client id stays reserved until the
    /// compositor releases it; a compositor id is free at once.
    fn destroyed(&mut self, id: u32) {
        if id >= SERVER_ID_BASE {
            if let Some(slot) = self.server.get_mut((id - SERVER_ID_BASE) as usize) {
                *slot = None;
            }
        } else if let Some(Some(slot)) = self.client.get_mut(id as usize) {
            slot.dead = true;
        }
    }

    /// The compositor released client id `id` (`wl_display.delete_id`).
    fn released(&mut self, id: u32) {
        if id < SERVER_ID_BASE && id > 1 {
            self.unalloc(id);
        }
    }
}

/// Why a connection can no longer be used.
#[derive(Debug, Clone)]
enum Broken {
    Protocol {
        object: u32,
        interface: &'static str,
        code: u32,
        message: String,
    },
    Wire(WireError),
    Closed,
}

impl Broken {
    fn error(&self) -> Error {
        match self {
            Broken::Protocol {
                object,
                interface,
                code,
                message,
            } => Error::Protocol {
                object: *object,
                interface,
                code: *code,
                message: message.clone(),
            },
            Broken::Wire(e) => Error::Wire(e.clone()),
            Broken::Closed => Error::Closed,
        }
    }
}

/// One argument of an event, for code that reads events without the
/// generated types.
#[derive(Debug)]
pub enum Arg {
    /// A signed integer.
    Int(i32),
    /// An unsigned integer.
    Uint(u32),
    /// A fixed-point number.
    Fixed(Fixed),
    /// A string, `None` when null.
    Str(Option<String>),
    /// An object's id, 0 when null.
    Object(u32),
    /// The id of an object the event created.
    NewId(u32),
    /// A byte array.
    Array(Vec<u8>),
    /// A file descriptor.
    Fd(OwnedFd),
}

/// An event as it came off the wire: which object, which of its events,
/// and the arguments still encoded. The generated `Event::decode` of the
/// object's interface turns it into a typed value; the reading methods here
/// are what that decoder calls, each taking the next argument.
pub struct Event {
    /// The id of the object the event is for.
    pub object: u32,
    /// That object's interface.
    pub interface: &'static Interface,
    /// Which of the interface's events this is.
    pub opcode: u16,
    body: Vec<u8>,
    pos: usize,
    fds: VecDeque<OwnedFd>,
}

impl fmt::Debug for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}#{}.{} ({} bytes, {} fds)",
            self.interface.name,
            self.object,
            self.name(),
            self.body.len(),
            self.fds.len()
        )
    }
}

impl Event {
    /// The event's name in the protocol.
    pub fn name(&self) -> &'static str {
        self.interface
            .events
            .get(usize::from(self.opcode))
            .map_or("?", |m| m.name)
    }

    /// Whether the event is for `object`.
    pub fn is_for<T: Proxy>(&self, object: &T) -> bool {
        self.object == object.id()
    }

    /// Fails unless the event's object is of `interface` (by name).
    pub fn expect(&self, interface: &'static Interface) -> Result<(), Error> {
        if self.interface.name == interface.name {
            Ok(())
        } else {
            Err(Error::WrongInterface {
                expected: interface.name,
                found: self.interface.name,
            })
        }
    }

    fn read<T>(
        &mut self,
        f: impl FnOnce(&mut Reader<'_>) -> Result<T, WireError>,
    ) -> Result<T, Error> {
        let mut r = Reader::at(&self.body, self.pos);
        let v = f(&mut r)?;
        self.pos = r.position();
        Ok(v)
    }

    /// The next argument, a signed integer.
    pub fn int(&mut self) -> Result<i32, Error> {
        self.read(|r| r.int())
    }

    /// The next argument, an unsigned integer or an untyped object id.
    pub fn uint(&mut self) -> Result<u32, Error> {
        self.read(|r| r.uint())
    }

    /// The next argument, a fixed-point number.
    pub fn fixed(&mut self) -> Result<Fixed, Error> {
        self.read(|r| r.fixed())
    }

    /// The next argument, a string that may be null.
    pub fn string_opt(&mut self) -> Result<Option<String>, Error> {
        self.read(|r| r.string())
    }

    /// The next argument, a string.
    pub fn string(&mut self) -> Result<String, Error> {
        self.string_opt()?.ok_or(Error::Wire(WireError::Null))
    }

    /// The next argument, an object that may be null.
    pub fn object_opt<T: Proxy>(&mut self) -> Result<Option<T>, Error> {
        let id = self.uint()?;
        Ok((id != 0).then(|| T::from_id(id)))
    }

    /// The next argument, an object or one the event created.
    pub fn object<T: Proxy>(&mut self) -> Result<T, Error> {
        self.object_opt()?.ok_or(Error::Wire(WireError::Null))
    }

    /// The next argument, a byte array.
    pub fn array(&mut self) -> Result<Vec<u8>, Error> {
        self.read(|r| r.array().map(<[u8]>::to_vec))
    }

    /// The next file descriptor argument. It is closed when dropped, as
    /// are any the event carries that are never taken.
    pub fn fd(&mut self) -> Result<OwnedFd, Error> {
        self.fds
            .pop_front()
            .ok_or(Error::Wire(WireError::MissingFd))
    }

    /// The arguments not yet read, each decoded by its type in the
    /// interface's table.
    pub fn args(&mut self) -> Result<Vec<Arg>, Error> {
        let Some(desc) = self.interface.events.get(usize::from(self.opcode)) else {
            return Err(Error::Wire(WireError::UnknownOpcode(self.opcode)));
        };
        let mut out = Vec::with_capacity(desc.args.len());
        for a in desc.args {
            out.push(match a.kind {
                Kind::Int => Arg::Int(self.int()?),
                Kind::Uint => Arg::Uint(self.uint()?),
                Kind::Fixed => Arg::Fixed(self.fixed()?),
                Kind::String => Arg::Str(self.string_opt()?),
                Kind::Object => Arg::Object(self.uint()?),
                Kind::NewId => Arg::NewId(self.uint()?),
                Kind::Array => Arg::Array(self.array()?),
                Kind::Fd => Arg::Fd(self.fd()?),
            });
        }
        Ok(out)
    }
}

/// A connection to a Wayland compositor.
///
/// Requests are written to a buffer and go out on [`flush`](Self::flush)
/// or when the connection waits for events. Events are pulled, one value
/// at a time, with [`next_event`](Self::next_event) (what has already
/// arrived) or [`wait_event`](Self::wait_event) (blocking); there are no
/// callbacks and no threads. The socket is never switched to non-blocking
/// mode; each call says whether it blocks.
pub struct Connection {
    stream: UnixStream,
    /// Requests not yet written.
    out: Vec<u8>,
    /// Descriptors to send, each with the offset in `out` of the message it
    /// belongs to.
    out_fds: VecDeque<(usize, OwnedFd)>,
    /// The compositor stopped reading; nothing more is written.
    write_closed: bool,
    /// Bytes read and not yet parsed, from `in_pos`.
    inbuf: Vec<u8>,
    in_pos: usize,
    /// Descriptors read and not yet claimed by an event.
    in_fds: VecDeque<OwnedFd>,
    /// Events parsed while waiting for something else (a roundtrip).
    queue: VecDeque<Event>,
    objects: Objects,
    broken: Option<Broken>,
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection")
            .field("fd", &self.stream.as_raw_fd())
            .field("unsent", &self.out.len())
            .field("unparsed", &(self.inbuf.len() - self.in_pos))
            .field("broken", &self.broken)
            .finish()
    }
}

impl AsRawFd for Connection {
    fn as_raw_fd(&self) -> RawFd {
        self.stream.as_raw_fd()
    }
}

impl AsFd for Connection {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.stream.as_fd()
    }
}

/// Where the environment says the compositor's socket is.
enum Target {
    /// An already-connected socket inherited as this descriptor.
    Inherited(RawFd),
    Path(PathBuf),
}

/// Reads `WAYLAND_SOCKET`, then `WAYLAND_DISPLAY` (default `wayland-0`)
/// under `XDG_RUNTIME_DIR`, as libwayland does.
fn target(
    socket: Option<String>,
    display: Option<String>,
    runtime_dir: Option<String>,
) -> Result<Target, Error> {
    if let Some(s) = socket {
        return match s.parse::<RawFd>() {
            Ok(fd) if fd >= 0 => Ok(Target::Inherited(fd)),
            _ => Err(Error::NoCompositor(format!(
                "WAYLAND_SOCKET is {s:?}, not a file descriptor"
            ))),
        };
    }
    let name = match display {
        Some(d) if !d.is_empty() => d,
        _ => "wayland-0".to_string(),
    };
    let path = PathBuf::from(&name);
    if path.is_absolute() {
        return Ok(Target::Path(path));
    }
    match runtime_dir {
        Some(dir) if PathBuf::from(&dir).is_absolute() => {
            Ok(Target::Path(PathBuf::from(dir).join(path)))
        }
        _ => Err(Error::NoCompositor(
            "XDG_RUNTIME_DIR is not set to an absolute path".to_string(),
        )),
    }
}

impl Connection {
    /// Connects to the compositor the environment names: the inherited
    /// socket in `WAYLAND_SOCKET` if set (the variable is then removed, so
    /// a child process does not try to use it too), otherwise the socket
    /// `WAYLAND_DISPLAY` names (default `wayland-0`), relative to
    /// `XDG_RUNTIME_DIR` unless it is an absolute path.
    pub fn connect() -> Result<Connection, Error> {
        let var = |k: &str| env::var_os(k).map(|v| v.to_string_lossy().into_owned());
        match target(
            var("WAYLAND_SOCKET"),
            var("WAYLAND_DISPLAY"),
            var("XDG_RUNTIME_DIR"),
        )? {
            Target::Inherited(fd) => {
                env::remove_var("WAYLAND_SOCKET");
                sys::set_cloexec(fd)
                    .map_err(|e| Error::NoCompositor(format!("WAYLAND_SOCKET={fd}: {e}")))?;
                // SAFETY: the variable is the parent's promise that this
                // descriptor is a connected socket handed to this process;
                // fcntl above showed it is open.
                Ok(Connection::from_stream(unsafe {
                    UnixStream::from_raw_fd(fd)
                }))
            }
            Target::Path(path) => Connection::connect_to(path),
        }
    }

    /// Connects to the compositor socket at `path`.
    pub fn connect_to(path: impl AsRef<std::path::Path>) -> Result<Connection, Error> {
        let path = path.as_ref();
        match UnixStream::connect(path) {
            Ok(stream) => Ok(Connection::from_stream(stream)),
            Err(e) => Err(Error::NoCompositor(format!("{}: {e}", path.display()))),
        }
    }

    /// A connection over a socket that is already connected.
    pub fn from_stream(stream: UnixStream) -> Connection {
        let mut objects = Objects::default();
        // Id 1 is the display, on every connection.
        objects.client.push(None);
        objects.client.push(Some(Slot {
            interface: &wl_display::INTERFACE,
            version: 1,
            dead: false,
        }));
        Connection {
            stream,
            out: Vec::new(),
            out_fds: VecDeque::new(),
            write_closed: false,
            inbuf: Vec::new(),
            in_pos: 0,
            in_fds: VecDeque::new(),
            queue: VecDeque::new(),
            objects,
            broken: None,
        }
    }

    /// The display, the one object every connection starts with.
    pub fn display(&self) -> WlDisplay {
        WlDisplay::from_id(1)
    }

    /// The interface and version of the object with `id`, if it is alive.
    pub fn object(&self, id: u32) -> Option<(&'static Interface, u32)> {
        self.objects
            .get(id)
            .filter(|s| !s.dead)
            .map(|s| (s.interface, s.version))
    }

    fn check(&self) -> Result<(), Error> {
        match &self.broken {
            Some(b) => Err(b.error()),
            None => Ok(()),
        }
    }

    fn fail<T>(&mut self, broken: Broken) -> Result<T, Error> {
        let e = broken.error();
        self.broken = Some(broken);
        Err(e)
    }

    /// Starts a request to `object`. This is what generated code calls;
    /// see [`Request`].
    pub fn request(&mut self, object: u32, opcode: u16) -> Request<'_> {
        let version = self.objects.get(object).map_or(1, |s| s.version);
        let fds_start = self.out_fds.len();
        Request {
            object,
            version,
            builder: Some(Builder::new(&mut self.out, object, opcode)),
            fds: &mut self.out_fds,
            fds_start,
            objects: &mut self.objects,
            broken: &self.broken,
            created: [0; MAX_NEW_IDS],
            created_len: 0,
            error: None,
            sent: false,
        }
    }

    /// Writes as much of the waiting requests as the socket takes without
    /// blocking. `true` means everything has gone.
    pub fn try_flush(&mut self) -> Result<bool, Error> {
        while !self.out.is_empty() {
            if self.write_closed {
                self.out.clear();
                self.out_fds.clear();
                break;
            }
            // At most MAX_FDS_OUT descriptors go with one write; if more
            // are waiting, stop the bytes short of the first message whose
            // descriptor would be left behind.
            let nfds = self.out_fds.len().min(sys::MAX_FDS_OUT);
            let mut limit = self.out.len();
            if let Some((offset, _)) = self.out_fds.get(nfds) {
                if *offset > 0 {
                    limit = limit.min(*offset);
                }
            }
            let mut raw = [0 as RawFd; sys::MAX_FDS_OUT];
            for (slot, (_, fd)) in raw.iter_mut().zip(self.out_fds.iter()) {
                *slot = fd.as_raw_fd();
            }
            match sys::send(self.stream.as_raw_fd(), &self.out[..limit], &raw[..nfds]) {
                Ok(n) => {
                    // The descriptors went with the first byte; ours can
                    // close.
                    self.out_fds.drain(..nfds);
                    self.out.drain(..n);
                    for (offset, _) in &mut self.out_fds {
                        *offset = offset.saturating_sub(n);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
                    ) =>
                {
                    // The compositor has gone, most likely after sending
                    // the error that says why: leave that to be read.
                    self.write_closed = true;
                }
                Err(e) => return Err(Error::Io(e)),
            }
        }
        Ok(true)
    }

    /// Writes all waiting requests, blocking until the socket has taken
    /// them. Events that arrive meanwhile are read and kept, so the
    /// compositor is never left blocked writing to a client that is blocked
    /// writing to it.
    pub fn flush(&mut self) -> Result<(), Error> {
        while !self.try_flush()? {
            let ready = sys::wait(self.stream.as_raw_fd(), true, None)?;
            if ready.read {
                self.read()?;
            }
        }
        Ok(())
    }

    /// Reads what the compositor has sent, without blocking, into the
    /// connection's buffer. Returns the number of bytes; 0 means nothing
    /// was waiting. Call it when the descriptor polls readable, then drain
    /// [`next_event`](Self::next_event).
    pub fn read(&mut self) -> Result<usize, Error> {
        self.check()?;
        if self.in_pos == self.inbuf.len() {
            self.inbuf.clear();
            self.in_pos = 0;
        } else if self.in_pos >= 4096 {
            self.inbuf.drain(..self.in_pos);
            self.in_pos = 0;
        }
        let mut chunk = [0u8; 4096];
        match sys::recv(self.stream.as_raw_fd(), &mut chunk, &mut self.in_fds) {
            Ok(0) => self.fail(Broken::Closed),
            Ok(n) => {
                self.inbuf.extend_from_slice(&chunk[..n]);
                Ok(n)
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(0),
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => self.fail(Broken::Closed),
            Err(e) => Err(Error::Io(e)),
        }
    }

    /// The next whole event in the buffer, skipping what is not the
    /// caller's business: events for objects already destroyed, the
    /// display's id bookkeeping. A protocol error from the compositor ends
    /// the connection and is returned as the error.
    fn parse(&mut self) -> Result<Option<Event>, Error> {
        loop {
            let avail = &self.inbuf[self.in_pos..];
            let header = match wire::header(avail) {
                Ok(Some(h)) if avail.len() >= h.len => h,
                Ok(_) => return Ok(None),
                Err(e) => return self.fail(Broken::Wire(e)),
            };
            let body = &avail[wire::HEADER_LEN..header.len];
            // libwayland too drops an event for an id it does not know.
            let Some(slot) = self.objects.get(header.object).copied() else {
                self.in_pos += header.len;
                continue;
            };
            let Some(desc) = slot.interface.events.get(usize::from(header.opcode)) else {
                return self.fail(Broken::Wire(WireError::UnknownOpcode(header.opcode)));
            };
            let mut created = Vec::new();
            let nfds = match wire::validate(body, desc.args, |id, i| created.push((id, i))) {
                Ok(n) => n,
                Err(e) => return self.fail(Broken::Wire(e)),
            };
            if self.in_fds.len() < nfds {
                return self.fail(Broken::Wire(WireError::MissingFd));
            }
            let fds: VecDeque<OwnedFd> = self.in_fds.drain(..nfds).collect();
            let body = body.to_vec();
            self.in_pos += header.len;
            if slot.dead {
                // Its descriptors close here; objects it would have created
                // are never known, and their events are dropped in turn.
                continue;
            }
            for (id, interface) in created {
                let Some(interface) = interface else {
                    return self.fail(Broken::Wire(WireError::BadNewId(id)));
                };
                if let Err(e) = self.objects.insert_server(id, interface, slot.version) {
                    return self.fail(Broken::Wire(e));
                }
            }
            let event = Event {
                object: header.object,
                interface: slot.interface,
                opcode: header.opcode,
                body,
                pos: 0,
                fds,
            };
            if header.object != 1 {
                return Ok(Some(event));
            }
            match wl_display::Event::decode(event)? {
                wl_display::Event::Error {
                    object_id,
                    code,
                    message,
                } => {
                    let interface = self
                        .objects
                        .get(object_id)
                        .map_or("unknown", |s| s.interface.name);
                    return self.fail(Broken::Protocol {
                        object: object_id,
                        interface,
                        code,
                        message,
                    });
                }
                wl_display::Event::DeleteId { id } => self.objects.released(id),
            }
        }
    }

    /// The next event that has already been read, or `None` when the
    /// buffer holds no whole one. Does no I/O.
    pub fn next_event(&mut self) -> Result<Option<Event>, Error> {
        if let Some(event) = self.queue.pop_front() {
            return Ok(Some(event));
        }
        self.check()?;
        self.parse()
    }

    /// Sends what is waiting and blocks until `done` says so, reading as
    /// events arrive. `deadline` bounds the wait; `Ok(false)` is a timeout.
    fn pump(
        &mut self,
        deadline: Option<Instant>,
        mut done: impl FnMut(&mut Connection) -> Result<bool, Error>,
    ) -> Result<bool, Error> {
        loop {
            if done(self)? {
                return Ok(true);
            }
            let flushed = self.try_flush()?;
            let timeout = match deadline {
                None => None,
                Some(d) => match d.checked_duration_since(Instant::now()) {
                    Some(left) if !left.is_zero() => Some(left),
                    _ => return Ok(false),
                },
            };
            let ready = sys::wait(self.stream.as_raw_fd(), !flushed, timeout)?;
            if ready.read {
                self.read()?;
            }
        }
    }

    /// Sends the waiting requests and blocks until an event arrives.
    pub fn wait_event(&mut self) -> Result<Event, Error> {
        let mut found = None;
        self.pump(None, |c| {
            found = c.next_event()?;
            Ok(found.is_some())
        })?;
        found.ok_or(Error::Closed)
    }

    /// Like [`wait_event`](Self::wait_event), giving up after `timeout`
    /// with `None`.
    pub fn wait_event_timeout(&mut self, timeout: Duration) -> Result<Option<Event>, Error> {
        let mut found = None;
        self.pump(Instant::now().checked_add(timeout), |c| {
            found = c.next_event()?;
            Ok(found.is_some())
        })?;
        Ok(found)
    }

    /// Sends the waiting requests and blocks until the compositor has
    /// handled every one of them (`wl_display.sync`). Events that arrive
    /// meanwhile are kept, in order, for [`next_event`](Self::next_event).
    pub fn roundtrip(&mut self) -> Result<(), Error> {
        self.check()?;
        let callback = self.display().sync(self)?;
        let mut held = VecDeque::new();
        let result = self.pump(None, |c| {
            while let Some(event) = c.parse()? {
                if event.object == callback.id()
                    && event.interface.name == wl_callback::INTERFACE.name
                {
                    return Ok(true);
                }
                held.push_back(event);
            }
            Ok(false)
        });
        self.queue.append(&mut held);
        result.map(|_| ())
    }
}

/// How many objects one request can create.
const MAX_NEW_IDS: usize = 4;

/// A request being written. The methods append its arguments in order;
/// [`send`](Self::send) completes it. A problem with an argument (a string
/// with a NUL in it, a descriptor that cannot be duplicated) is kept and
/// returned by `send`, and nothing is sent. Dropping the request without
/// sending it leaves no trace.
pub struct Request<'a> {
    object: u32,
    version: u32,
    /// Taken by `finish`; there until then.
    builder: Option<Builder<'a>>,
    fds: &'a mut VecDeque<(usize, OwnedFd)>,
    fds_start: usize,
    objects: &'a mut Objects,
    broken: &'a Option<Broken>,
    created: [u32; MAX_NEW_IDS],
    created_len: usize,
    error: Option<Error>,
    sent: bool,
}

impl Drop for Request<'_> {
    fn drop(&mut self) {
        if !self.sent {
            // Dropping the builder takes the bytes back out; the
            // descriptors and ids go the same way.
            self.fds.truncate(self.fds_start);
            // Last made, first returned, so the ids come out again in
            // the order they were first given.
            for id in self.created[..self.created_len].iter().rev() {
                self.objects.unalloc(*id);
            }
        }
    }
}

impl Request<'_> {
    fn with<T>(&mut self, f: impl FnOnce(&mut Builder<'_>) -> T) -> Option<T> {
        self.builder.as_mut().map(f)
    }

    /// A signed integer argument.
    pub fn int(&mut self, v: i32) {
        self.with(|b| b.int(v));
    }

    /// An unsigned integer or object id argument (0 is the null object).
    pub fn uint(&mut self, v: u32) {
        self.with(|b| b.uint(v));
    }

    /// A fixed-point argument.
    pub fn fixed(&mut self, v: Fixed) {
        self.with(|b| b.fixed(v));
    }

    /// A string argument, `None` for null.
    pub fn string(&mut self, s: Option<&str>) {
        if let Some(Err(e)) = self.with(|b| b.string(s)) {
            self.error.get_or_insert(Error::Wire(e));
        }
    }

    /// A byte array argument.
    pub fn array(&mut self, bytes: &[u8]) {
        if let Some(Err(e)) = self.with(|b| b.array(bytes)) {
            self.error.get_or_insert(Error::Wire(e));
        }
    }

    /// A file descriptor argument. It is duplicated, so the caller's may
    /// be closed as soon as this returns.
    pub fn fd(&mut self, fd: BorrowedFd<'_>) {
        match fd.try_clone_to_owned() {
            Ok(owned) => {
                let start = self.with(|b| b.start()).unwrap_or(0);
                self.fds.push_back((start, owned));
            }
            Err(e) => {
                self.error.get_or_insert(Error::Io(e));
            }
        }
    }

    fn create(&mut self, interface: &'static Interface, version: u32) -> u32 {
        if self.created_len == MAX_NEW_IDS {
            self.error.get_or_insert(Error::TooManyObjects);
            return 0;
        }
        match self.objects.alloc(interface, version) {
            Ok(id) => {
                self.created[self.created_len] = id;
                self.created_len += 1;
                id
            }
            Err(e) => {
                self.error.get_or_insert(e);
                0
            }
        }
    }

    /// A new-id argument: allocates the id of the object this request
    /// creates, at the version of the object the request is sent to.
    pub fn new_id(&mut self, interface: &'static Interface) -> u32 {
        let id = self.create(interface, self.version);
        self.uint(id);
        id
    }

    /// An untyped new-id argument, as `wl_registry.bind` has: the
    /// interface's name, the version to bind, and the new id.
    pub fn new_id_bound(&mut self, interface: &'static Interface, version: u32) -> u32 {
        let id = self.create(interface, version);
        self.string(Some(interface.name));
        self.uint(version);
        self.uint(id);
        id
    }

    fn finish(mut self, destructor: bool) -> Result<(), Error> {
        if let Some(e) = self.error.take() {
            return Err(e);
        }
        if let Some(b) = self.broken {
            return Err(b.error());
        }
        let Some(builder) = self.builder.take() else {
            return Err(Error::Wire(WireError::TooLong));
        };
        builder.finish()?;
        self.sent = true;
        if destructor {
            self.objects.destroyed(self.object);
        }
        Ok(())
    }

    /// Completes the request and queues it to be written.
    pub fn send(self) -> Result<(), Error> {
        self.finish(false)
    }

    /// Completes a request that destroys its object.
    pub fn send_destructor(self) -> Result<(), Error> {
        self.finish(true)
    }
}
