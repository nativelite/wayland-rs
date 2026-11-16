//! The connection against a compositor played by the test itself, over a
//! socket pair in one thread: the test writes the bytes a compositor would
//! and reads the bytes the client sent.

use super::*;
use crate::protocol::wayland::{
    wl_data_device, wl_data_offer, wl_keyboard, wl_registry, WlCompositor, WlDataDeviceManager,
    WlSeat, WlShm,
};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::IntoRawFd;

/// The compositor's end of the socket.
struct Server {
    sock: UnixStream,
    buf: Vec<u8>,
    fds: VecDeque<OwnedFd>,
}

/// One request as the server read it.
#[derive(Debug, PartialEq, Eq)]
struct Got {
    object: u32,
    opcode: u16,
    body: Vec<u8>,
}

fn pair() -> (Connection, Server) {
    let (a, b) = UnixStream::pair().unwrap();
    let server = Server {
        sock: b,
        buf: Vec::new(),
        fds: VecDeque::new(),
    };
    (Connection::from_stream(a), server)
}

/// The bytes of one message.
fn message(object: u32, opcode: u16, f: impl FnOnce(&mut Builder<'_>)) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut b = Builder::new(&mut buf, object, opcode);
    f(&mut b);
    b.finish().unwrap();
    buf
}

fn words(ws: &[u32]) -> Vec<u8> {
    ws.iter().flat_map(|w| w.to_ne_bytes()).collect()
}

impl Server {
    /// Writes `bytes` with `fds` beside them.
    fn send_fds(&self, bytes: &[u8], fds: &[RawFd]) {
        let mut sent = sys::send(self.sock.as_raw_fd(), bytes, fds).unwrap();
        while sent < bytes.len() {
            sent += sys::send(self.sock.as_raw_fd(), &bytes[sent..], &[]).unwrap();
        }
    }

    fn send(&self, bytes: &[u8]) {
        self.send_fds(bytes, &[]);
    }

    /// Reads everything the client has written so far.
    fn fill(&mut self) {
        let mut chunk = [0u8; 4096];
        loop {
            match sys::recv(self.sock.as_raw_fd(), &mut chunk, &mut self.fds) {
                Ok(0) => break,
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => panic!("server read: {e}"),
            }
        }
    }

    /// The next request the client sent, if a whole one has arrived.
    fn request(&mut self) -> Option<Got> {
        self.fill();
        let h = wire::header(&self.buf).unwrap()?;
        if self.buf.len() < h.len {
            return None;
        }
        let msg: Vec<u8> = self.buf.drain(..h.len).collect();
        Some(Got {
            object: h.object,
            opcode: h.opcode,
            body: msg[wire::HEADER_LEN..].to_vec(),
        })
    }

    fn expect(&mut self, object: u32, opcode: u16, body: &[u8]) {
        let got = self.request().expect("a request");
        assert_eq!(
            got,
            Got {
                object,
                opcode,
                body: body.to_vec()
            }
        );
    }
}

/// What a file descriptor reads as from its start.
fn contents(fd: OwnedFd) -> String {
    let mut f = File::from(fd);
    f.seek(SeekFrom::Start(0)).unwrap();
    let mut s = String::new();
    f.read_to_string(&mut s).unwrap();
    s
}

/// A file holding `text`, deleted on disk already.
fn file_with(text: &str) -> File {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = env::temp_dir().join(format!("nl-wayland-{}-{n}", std::process::id()));
    let mut f = File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    std::fs::remove_file(&path).unwrap();
    f.write_all(text.as_bytes()).unwrap();
    f
}

/// Reads what the server sent and returns every event it makes.
fn drain(conn: &mut Connection) -> Result<Vec<Event>, Error> {
    while conn.read()? > 0 {}
    let mut out = Vec::new();
    while let Some(e) = conn.next_event()? {
        out.push(e);
    }
    Ok(out)
}

fn global(registry: u32, name: u32, interface: &str, version: u32) -> Vec<u8> {
    message(registry, 0, |b| {
        b.uint(name);
        b.string(Some(interface)).unwrap();
        b.uint(version);
    })
}

#[test]
fn the_registry_lists_globals_and_binds_them() {
    let (mut conn, mut server) = pair();
    let registry = conn.display().get_registry(&mut conn).unwrap();
    assert_eq!(registry.id(), 2);
    assert_eq!(server.request(), None, "nothing goes out before a flush");
    conn.flush().unwrap();
    server.expect(1, 1, &words(&[2]));

    server.send(&global(2, 10, "wl_compositor", 6));
    server.send(&global(2, 11, "wl_shm", 1));
    let events = drain(&mut conn).unwrap();
    assert_eq!(events.len(), 2);
    let mut seen = Vec::new();
    for e in events {
        assert!(e.is_for(&registry));
        assert_eq!(e.name(), "global");
        match wl_registry::Event::decode(e).unwrap() {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => seen.push((name, interface, version)),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(
        seen,
        [
            (10, "wl_compositor".to_string(), 6),
            (11, "wl_shm".to_string(), 1)
        ]
    );

    let compositor: WlCompositor = registry.bind(&mut conn, 10, 4).unwrap();
    assert_eq!(compositor.id(), 3);
    let (interface, version) = conn.object(3).unwrap();
    assert_eq!((interface.name, version), ("wl_compositor", 4));
    let surface = compositor.create_surface(&mut conn).unwrap();
    assert_eq!(
        conn.object(surface.id()).unwrap().1,
        4,
        "a child has its parent's version"
    );
    conn.flush().unwrap();
    let mut body = words(&[10, 14]);
    body.extend_from_slice(b"wl_compositor\0\0\0");
    body.extend_from_slice(&words(&[4, 3]));
    server.expect(2, 0, &body);
    server.expect(3, 0, &words(&[4]));
    assert_eq!(server.request(), None);
}

#[test]
fn a_roundtrip_keeps_the_events_that_came_first_and_frees_the_callback_id() {
    let (mut conn, mut server) = pair();
    let registry = conn.display().get_registry(&mut conn).unwrap();
    // The compositor's whole answer is already in the socket: a global,
    // the sync callback's `done`, and the release of its id (3).
    server.send(&global(2, 1, "wl_seat", 9));
    server.send(&message(3, 0, |b| b.uint(77)));
    server.send(&message(1, 1, |b| b.uint(3)));
    conn.roundtrip().unwrap();
    server.expect(1, 1, &words(&[2]));
    server.expect(1, 0, &words(&[3]));

    let e = conn.next_event().unwrap().expect("the global");
    assert!(e.is_for(&registry));
    assert!(conn.next_event().unwrap().is_none());
    // Id 3 was released, so the next object gets it; then 4.
    let seat: WlSeat = registry.bind(&mut conn, 1, 5).unwrap();
    assert_eq!(seat.id(), 3);
    assert_eq!(seat.get_pointer(&mut conn).unwrap().id(), 4);
}

#[test]
fn a_file_descriptor_goes_out_with_its_request_and_comes_in_with_its_event() {
    let (mut conn, mut server) = pair();
    let registry = conn.display().get_registry(&mut conn).unwrap();
    let shm: WlShm = registry.bind(&mut conn, 1, 1).unwrap();
    let file = file_with("from the client");
    let pool = shm.create_pool(&mut conn, file.as_fd(), 4096).unwrap();
    drop(file); // The request holds its own copy.
    conn.flush().unwrap();
    server.request().unwrap();
    server.request().unwrap();
    server.expect(shm.id(), 0, &words(&[pool.id(), 4096]));
    assert_eq!(server.fds.len(), 1);
    assert_eq!(contents(server.fds.pop_front().unwrap()), "from the client");

    let seat: WlSeat = registry.bind(&mut conn, 2, 5).unwrap();
    let keyboard = seat.get_keyboard(&mut conn).unwrap();
    let keymap = file_with("from the compositor");
    server.send_fds(
        &message(keyboard.id(), 0, |b| {
            b.uint(1);
            b.uint(19);
        }),
        &[keymap.as_raw_fd()],
    );
    drop(keymap);
    let mut events = drain(&mut conn).unwrap();
    assert_eq!(events.len(), 1);
    match wl_keyboard::Event::decode(events.remove(0)).unwrap() {
        wl_keyboard::Event::Keymap { format, fd, size } => {
            assert_eq!((format, size), (wl_keyboard::keymap_format::XKB_V1, 19));
            assert_eq!(contents(fd), "from the compositor");
        }
        other => panic!("{other:?}"),
    }
}

/// Whether the other end of `sock` has been closed by everyone holding it.
fn hung_up(mut sock: &UnixStream) -> bool {
    sock.set_nonblocking(true).unwrap();
    let mut b = [0u8; 1];
    match sock.read(&mut b) {
        Ok(0) => true,
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => false,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_descriptor_nobody_takes_is_closed() {
    let (mut conn, server) = pair();
    let registry = conn.display().get_registry(&mut conn).unwrap();
    let seat: WlSeat = registry.bind(&mut conn, 2, 5).unwrap();
    let keyboard = seat.get_keyboard(&mut conn).unwrap();
    let keymap = |b: &mut Builder<'_>| {
        b.uint(1);
        b.uint(0);
    };

    // An event that is dropped undecoded.
    let (ours, theirs) = UnixStream::pair().unwrap();
    server.send_fds(&message(keyboard.id(), 0, keymap), &[theirs.as_raw_fd()]);
    drop(theirs);
    let events = drain(&mut conn).unwrap();
    assert_eq!(events.len(), 1);
    assert!(!hung_up(&ours), "the event holds it");
    drop(events);
    assert!(hung_up(&ours));

    // An event for an object this side has destroyed never surfaces, and
    // its descriptor is closed too.
    keyboard.release(&mut conn).unwrap();
    let (ours, theirs) = UnixStream::pair().unwrap();
    server.send_fds(&message(keyboard.id(), 0, keymap), &[theirs.as_raw_fd()]);
    drop(theirs);
    assert!(drain(&mut conn).unwrap().is_empty());
    assert!(hung_up(&ours));

    // A descriptor still waiting in the connection goes with it.
    let (ours, theirs) = UnixStream::pair().unwrap();
    server.send_fds(&[0, 0], &[theirs.as_raw_fd()]);
    drop(theirs);
    assert_eq!(conn.read().unwrap(), 2);
    assert!(!hung_up(&ours));
    drop(conn);
    assert!(hung_up(&ours));
}

#[test]
fn more_descriptors_than_one_write_carries_arrive_in_order() {
    let (mut conn, mut server) = pair();
    let registry = conn.display().get_registry(&mut conn).unwrap();
    let shm: WlShm = registry.bind(&mut conn, 1, 1).unwrap();
    let count = 3 * sys::MAX_FDS_OUT + 5;
    for i in 0..count {
        let file = file_with(&format!("file {i}"));
        shm.create_pool(&mut conn, file.as_fd(), i as i32).unwrap();
    }
    conn.flush().unwrap();
    server.request().unwrap();
    server.request().unwrap();
    for i in 0..count {
        let got = server.request().unwrap();
        assert_eq!((got.object, got.opcode), (shm.id(), 0));
        assert_eq!(got.body[4..], (i as u32).to_ne_bytes());
        // The descriptor is there by the time its message is.
        assert_eq!(
            contents(server.fds.pop_front().unwrap()),
            format!("file {i}")
        );
    }
    assert_eq!(server.request(), None);
    assert!(server.fds.is_empty());
}

#[test]
fn objects_the_compositor_creates_are_known_and_decode() {
    let (mut conn, server) = pair();
    let registry = conn.display().get_registry(&mut conn).unwrap();
    let manager: WlDataDeviceManager = registry.bind(&mut conn, 1, 3).unwrap();
    let seat: WlSeat = registry.bind(&mut conn, 2, 5).unwrap();
    let device = manager.get_data_device(&mut conn, seat).unwrap();

    server.send(&message(device.id(), 0, |b| b.uint(0xff00_0000)));
    server.send(&message(0xff00_0000, 0, |b| {
        b.string(Some("text/plain")).unwrap()
    }));
    let mut events = drain(&mut conn).unwrap();
    assert_eq!(events.len(), 2);
    let offer = match wl_data_device::Event::decode(events.remove(0)).unwrap() {
        wl_data_device::Event::DataOffer { id } => id,
        other => panic!("{other:?}"),
    };
    assert_eq!(offer.id(), 0xff00_0000);
    let (interface, version) = conn.object(offer.id()).unwrap();
    assert_eq!((interface.name, version), ("wl_data_offer", 3));
    match wl_data_offer::Event::decode(events.remove(0)).unwrap() {
        wl_data_offer::Event::Offer { mime_type } => assert_eq!(mime_type, "text/plain"),
        other => panic!("{other:?}"),
    }
    // Decoding it as another interface is refused.
    server.send(&message(0xff00_0000, 0, |b| b.string(Some("x")).unwrap()));
    let e = drain(&mut conn).unwrap().remove(0);
    match wl_registry::Event::decode(e) {
        Err(Error::WrongInterface { expected, found }) => {
            assert_eq!((expected, found), ("wl_registry", "wl_data_offer"))
        }
        other => panic!("{other:?}"),
    }

    // Destroying a compositor-made object frees its id at once (no
    // delete_id comes for those), and the compositor may use it again.
    offer.destroy(&mut conn).unwrap();
    assert!(conn.object(offer.id()).is_none());
    server.send(&message(0xff00_0000, 0, |b| {
        b.string(Some("stale")).unwrap()
    }));
    assert!(
        drain(&mut conn).unwrap().is_empty(),
        "an event for no object is dropped"
    );
    server.send(&message(device.id(), 0, |b| b.uint(0xff00_0000)));
    server.send(&message(device.id(), 0, |b| b.uint(0xff00_0001)));
    assert_eq!(drain(&mut conn).unwrap().len(), 2);
    assert!(conn.object(0xff00_0001).is_some());
}

#[test]
fn an_object_id_the_compositor_may_not_use_ends_the_connection() {
    for bad in [5u32, 0xff00_0001, 0xffff_ffff] {
        let (mut conn, server) = pair();
        let registry = conn.display().get_registry(&mut conn).unwrap();
        let manager: WlDataDeviceManager = registry.bind(&mut conn, 1, 3).unwrap();
        let seat: WlSeat = registry.bind(&mut conn, 2, 5).unwrap();
        let device = manager.get_data_device(&mut conn, seat).unwrap();
        server.send(&message(device.id(), 0, |b| b.uint(bad)));
        match drain(&mut conn) {
            Err(Error::Wire(WireError::BadNewId(id))) => assert_eq!(id, bad),
            other => panic!("{bad:#x}: {other:?}"),
        }
        // And it stays ended.
        assert!(matches!(
            conn.read(),
            Err(Error::Wire(WireError::BadNewId(_)))
        ));
        assert!(matches!(
            conn.display().sync(&mut conn),
            Err(Error::Wire(WireError::BadNewId(_)))
        ));
    }
}

#[test]
fn a_protocol_error_is_returned_and_ends_the_connection() {
    let (mut conn, server) = pair();
    let registry = conn.display().get_registry(&mut conn).unwrap();
    server.send(&global(2, 1, "wl_seat", 9));
    server.send(&message(1, 0, |b| {
        b.uint(registry.id());
        b.uint(7);
        b.string(Some("invalid global")).unwrap();
    }));
    drop(server);
    assert!(conn.read().unwrap() > 0);
    // The event before the error still comes out.
    assert!(conn.next_event().unwrap().is_some());
    let check = |e: Error| match e {
        Error::Protocol {
            object,
            interface,
            code,
            message,
        } => assert_eq!(
            (object, interface, code, message.as_str()),
            (2, "wl_registry", 7, "invalid global")
        ),
        other => panic!("{other:?}"),
    };
    check(conn.next_event().unwrap_err());
    check(conn.next_event().unwrap_err());
    check(conn.read().unwrap_err());
    check(conn.roundtrip().unwrap_err());
    check(conn.display().sync(&mut conn).unwrap_err());
    check(conn.wait_event().unwrap_err());
    assert!(format!("{conn:?}").contains("invalid global"));
}

#[test]
fn the_error_the_compositor_sent_before_hanging_up_wins_over_the_broken_pipe() {
    let (mut conn, server) = pair();
    server.send(&message(1, 0, |b| {
        b.uint(1);
        b.uint(1);
        b.string(Some("no such request")).unwrap();
    }));
    drop(server);
    // Writing into the closed socket fails; the reason is still read.
    conn.display().sync(&mut conn).unwrap();
    match conn.wait_event() {
        Err(Error::Protocol {
            interface, message, ..
        }) => {
            assert_eq!(
                (interface, message.as_str()),
                ("wl_display", "no such request")
            )
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_compositor_that_goes_away_is_reported_as_closed() {
    let (mut conn, server) = pair();
    drop(server);
    assert!(matches!(conn.wait_event(), Err(Error::Closed)));
    assert!(matches!(conn.next_event(), Err(Error::Closed)));
    let (mut conn, server) = pair();
    drop(server);
    assert!(matches!(conn.roundtrip(), Err(Error::Closed)));
    let (mut conn, server) = pair();
    drop(server);
    conn.display().sync(&mut conn).unwrap();
    assert!(
        matches!(conn.flush(), Ok(())),
        "nothing to tell from a write alone"
    );
    // The read says nothing arrived; the next event says why.
    assert_eq!(conn.read().unwrap(), 0);
    assert!(matches!(conn.next_event(), Err(Error::Closed)));
    assert!(matches!(conn.read(), Err(Error::Closed)));
}

#[test]
fn events_split_at_every_byte_come_out_whole() {
    let (mut conn, server) = pair();
    let registry = conn.display().get_registry(&mut conn).unwrap();
    let mut stream = Vec::new();
    let mut ends = Vec::new();
    for (i, name) in ["wl_compositor", "wl_shm", "xdg_wm_base", "a"]
        .iter()
        .enumerate()
    {
        stream.extend_from_slice(&global(registry.id(), i as u32, name, 1));
        ends.push(stream.len());
    }
    let mut seen = 0;
    for (i, byte) in stream.iter().enumerate() {
        server.send(&[*byte]);
        assert_eq!(conn.read().unwrap(), 1);
        let got = conn.next_event().unwrap();
        if ends.contains(&(i + 1)) {
            match wl_registry::Event::decode(got.expect("a whole event")).unwrap() {
                wl_registry::Event::Global { name, .. } => assert_eq!(name, seen),
                other => panic!("{other:?}"),
            }
            seen += 1;
        } else {
            assert!(got.is_none(), "byte {i}");
        }
    }
    assert_eq!(seen, 4);
    assert_eq!(conn.read().unwrap(), 0, "nothing waiting reads as zero");
}

#[test]
fn a_long_stream_of_events_is_read_through_buffer_compaction() {
    let (mut conn, server) = pair();
    let registry = conn.display().get_registry(&mut conn).unwrap();
    let mut next = 0;
    for round in 0..40 {
        for k in 0..25 {
            server.send(&global(
                registry.id(),
                round * 25 + k,
                "some_interface_name",
                3,
            ));
        }
        // Leave part of the batch unparsed some rounds, so the buffer is
        // compacted with bytes still in it.
        while conn.read().unwrap() > 0 {}
        let take = if round % 3 == 0 { 20 } else { usize::MAX };
        for _ in 0..take {
            let Some(e) = conn.next_event().unwrap() else {
                break;
            };
            match wl_registry::Event::decode(e).unwrap() {
                wl_registry::Event::Global { name, .. } => assert_eq!(name, next),
                other => panic!("{other:?}"),
            }
            next += 1;
        }
    }
    while let Some(_e) = conn.next_event().unwrap() {
        next += 1;
    }
    assert_eq!(next, 1000);
}

#[test]
fn events_for_destroyed_and_unknown_objects_are_dropped() {
    let (mut conn, mut server) = pair();
    let registry = conn.display().get_registry(&mut conn).unwrap();
    let seat: WlSeat = registry.bind(&mut conn, 2, 5).unwrap();
    let pointer = seat.get_pointer(&mut conn).unwrap();
    pointer.release(&mut conn).unwrap();
    assert!(
        conn.object(pointer.id()).is_none(),
        "destroyed for the caller"
    );
    // The id is not reused until the compositor lets go of it.
    let keyboard = seat.get_keyboard(&mut conn).unwrap();
    assert_eq!((pointer.id(), keyboard.id()), (4, 5));

    // A frame event still on its way to the pointer, one for an id never
    // made, then a real one.
    server.send(&message(pointer.id(), 5, |_| ()));
    server.send(&message(900, 3, |b| b.uint(1)));
    server.send(&message(seat.id(), 1, |b| b.string(Some("seat0")).unwrap()));
    let events = drain(&mut conn).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!((events[0].object, events[0].name()), (seat.id(), "name"));

    // delete_id frees the pointer's id for the next object.
    server.send(&message(1, 1, |b| b.uint(pointer.id())));
    assert!(drain(&mut conn).unwrap().is_empty());
    assert_eq!(seat.get_touch(&mut conn).unwrap().id(), pointer.id());
    // A delete_id for the display or for an id never made changes nothing.
    server.send(&message(1, 1, |b| b.uint(1)));
    server.send(&message(1, 1, |b| b.uint(4000)));
    server.send(&message(1, 1, |b| b.uint(0xff00_0000)));
    assert!(drain(&mut conn).unwrap().is_empty());
    assert_eq!(seat.get_pointer(&mut conn).unwrap().id(), 6);
    conn.flush().unwrap();
    while server.request().is_some() {}
}

#[test]
fn bytes_that_are_not_messages_end_the_connection_with_the_reason() {
    let cases: Vec<(Vec<u8>, WireError)> = vec![
        // A size under the header's own.
        (words(&[1, 4 << 16]), WireError::BadLength(4)),
        (words(&[1, 9 << 16, 0]), WireError::BadLength(9)),
        // wl_display has two events.
        (words(&[1, (8 << 16) | 2]), WireError::UnknownOpcode(2)),
        // wl_registry.global cut short, and with a string that overruns.
        (words(&[2, 12 << 16, 1]), WireError::Truncated),
        (words(&[2, 16 << 16, 1, 400]), WireError::Truncated),
        // A string without its NUL.
        (
            {
                let mut m = words(&[2, 24 << 16, 1, 4]);
                m.extend_from_slice(b"abcd");
                m.extend_from_slice(&words(&[1]));
                m
            },
            WireError::UnterminatedString,
        ),
        // A null string where there must be one.
        (words(&[2, 20 << 16, 1, 0, 1]), WireError::Null),
        // wl_keyboard.keymap (object 4) without its descriptor.
        (words(&[4, 16 << 16, 1, 0]), WireError::MissingFd),
    ];
    for (bytes, want) in cases {
        let (mut conn, server) = pair();
        let registry = conn.display().get_registry(&mut conn).unwrap();
        let seat: WlSeat = registry.bind(&mut conn, 2, 5).unwrap();
        assert_eq!(seat.get_keyboard(&mut conn).unwrap().id(), 4);
        server.send(&bytes);
        match drain(&mut conn) {
            Err(Error::Wire(e)) => assert_eq!(e, want),
            other => panic!("{want:?}: {other:?}"),
        }
        assert!(matches!(conn.next_event(), Err(Error::Wire(_))));
    }
}

#[test]
fn random_bytes_from_the_compositor_never_panic() {
    // xorshift, fixed seed: the same bytes every run.
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut events_seen = 0;
    let mut errors_seen = 0;
    for round in 0..400 {
        let (mut conn, server) = pair();
        let registry = conn.display().get_registry(&mut conn).unwrap();
        let seat: WlSeat = registry.bind(&mut conn, 2, 5).unwrap();
        seat.get_keyboard(&mut conn).unwrap();
        let manager: WlDataDeviceManager = registry.bind(&mut conn, 1, 3).unwrap();
        manager.get_data_device(&mut conn, seat).unwrap();
        let mut bytes = Vec::new();
        for _ in 0..(next() % 12) {
            // Now and then a real event, so the stream is not all errors.
            if next() % 2 == 0 {
                bytes.extend_from_slice(&message(3, 1, |b| b.string(Some("seat")).unwrap()));
            }
            // A plausible header for a live object, then noise.
            let object = (next() % 8) as u32;
            let opcode = (next() % 12) as u32;
            let len = 8 + 4 * (next() % 10) as u32;
            bytes.extend_from_slice(&words(&[object, (len << 16) | opcode]));
            for _ in 0..(len - 8) {
                let b = next();
                // Mostly small values, so lengths are often believable.
                bytes.push(if b % 3 == 0 {
                    (b >> 8) as u8
                } else {
                    (b >> 8) as u8 % 8
                });
            }
        }
        if round % 5 == 0 {
            bytes.truncate(bytes.len() / 2);
        }
        if bytes.is_empty() {
            continue;
        }
        server.send(&bytes);
        // Whatever it is, it is events or an error, and never a panic.
        while matches!(conn.read(), Ok(n) if n > 0) {}
        loop {
            match conn.next_event() {
                Ok(Some(mut e)) => {
                    events_seen += 1;
                    let _ = e.args();
                }
                Ok(None) => break,
                Err(_) => {
                    errors_seen += 1;
                    break;
                }
            }
        }
    }
    // The noise reached both the decoder and the error paths.
    assert!(
        events_seen > 20 && errors_seen > 20,
        "{events_seen} {errors_seen}"
    );
}

#[test]
fn a_request_that_cannot_be_sent_leaves_nothing_behind() {
    let (mut conn, mut server) = pair();
    let registry = conn.display().get_registry(&mut conn).unwrap();
    let shm: WlShm = registry.bind(&mut conn, 1, 1).unwrap();
    conn.flush().unwrap();
    while server.request().is_some() {}

    // A string with a NUL inside: the request creates an object and so
    // would have taken an id.
    let mut req = conn.request(registry.id(), 0);
    req.uint(1);
    req.string(Some("bad\0name"));
    let id = req.new_id(&wl_data_offer::INTERFACE);
    assert_eq!(id, 4);
    match req.send() {
        Err(Error::Wire(WireError::InteriorNul)) => {}
        other => panic!("{other:?}"),
    }
    // A request with a descriptor, dropped unsent.
    let file = file_with("x");
    let mut req = conn.request(shm.id(), 0);
    let id = req.new_id(&wl_data_offer::INTERFACE);
    assert_eq!(id, 4, "the id came back");
    req.fd(file.as_fd());
    req.int(1);
    drop(req);
    // A message longer than the format allows.
    let mut req = conn.request(shm.id(), 0);
    req.array(&vec![0; 70_000]);
    assert!(matches!(req.send(), Err(Error::Wire(WireError::TooLong))));
    // More new objects than one request may make.
    let mut req = conn.request(shm.id(), 0);
    for _ in 0..5 {
        req.new_id(&wl_data_offer::INTERFACE);
    }
    assert!(matches!(req.send(), Err(Error::TooManyObjects)));

    assert!(conn.out.is_empty() && conn.out_fds.is_empty());
    assert!(conn.try_flush().unwrap());
    assert_eq!(server.request(), None);
    assert!(server.fds.is_empty());
    assert_eq!(shm.create_pool(&mut conn, file.as_fd(), 1).unwrap().id(), 4);
}

#[test]
fn a_full_socket_holds_requests_until_the_compositor_reads() {
    let (mut conn, mut server) = pair();
    let registry = conn.display().get_registry(&mut conn).unwrap();
    let shm: WlShm = registry.bind(&mut conn, 1, 1).unwrap();
    let payload = vec![7u8; 4000];
    let mut total = conn.out.len();
    for _ in 0..600 {
        // Not a real request of wl_shm; the server here only counts bytes.
        let mut req = conn.request(shm.id(), 9);
        req.array(&payload);
        req.send().unwrap();
        total += 8 + 4 + 4000;
    }
    assert!(
        !conn.try_flush().unwrap(),
        "2.4 MB does not fit a socket buffer"
    );
    let mut rounds = 0;
    loop {
        server.fill();
        if conn.try_flush().unwrap() {
            break;
        }
        rounds += 1;
        assert!(rounds < 100_000);
    }
    server.fill();
    assert_eq!(server.buf.len(), total);
}

#[test]
fn waiting_with_a_timeout_gives_up_and_otherwise_returns_the_event() {
    let (mut conn, mut server) = pair();
    let registry = conn.display().get_registry(&mut conn).unwrap();
    let started = Instant::now();
    assert!(conn
        .wait_event_timeout(Duration::from_millis(30))
        .unwrap()
        .is_none());
    assert!(started.elapsed() >= Duration::from_millis(30));
    // Waiting flushed the request.
    server.expect(1, 1, &words(&[registry.id()]));
    assert!(conn.wait_event_timeout(Duration::ZERO).unwrap().is_none());
    server.send(&global(registry.id(), 1, "wl_output", 4));
    let e = conn
        .wait_event_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert_eq!(e.name(), "global");
    server.send(&global(registry.id(), 2, "wl_output", 4));
    assert_eq!(conn.wait_event().unwrap().name(), "global");
}

#[test]
fn arguments_can_be_read_without_the_generated_types() {
    let (mut conn, server) = pair();
    let registry = conn.display().get_registry(&mut conn).unwrap();
    server.send(&global(registry.id(), 3, "wl_output", 4));
    let mut e = drain(&mut conn).unwrap().remove(0);
    assert_eq!(format!("{e:?}"), "wl_registry#2.global (24 bytes, 0 fds)");
    match e.args().unwrap().as_slice() {
        [Arg::Uint(3), Arg::Str(Some(s)), Arg::Uint(4)] => assert_eq!(s, "wl_output"),
        other => panic!("{other:?}"),
    }
    // Reading past the last argument is an error, not a panic.
    assert!(matches!(e.uint(), Err(Error::Wire(WireError::Truncated))));
    assert!(matches!(e.fd(), Err(Error::Wire(WireError::MissingFd))));
}

#[test]
fn the_environment_names_the_socket_as_libwayland_reads_it() {
    let s = |v: &str| Some(v.to_string());
    let path = |t: Result<Target, Error>| match t {
        Ok(Target::Path(p)) => p.to_string_lossy().into_owned(),
        Ok(Target::Inherited(fd)) => format!("fd {fd}"),
        Err(e) => format!("error: {e}"),
    };
    let run = "/run/user/1000";
    assert_eq!(path(target(None, None, s(run))), "/run/user/1000/wayland-0");
    assert_eq!(
        path(target(None, s(""), s(run))),
        "/run/user/1000/wayland-0"
    );
    assert_eq!(
        path(target(None, s("wayland-7"), s(run))),
        "/run/user/1000/wayland-7"
    );
    assert_eq!(path(target(None, s("/tmp/sock"), None)), "/tmp/sock");
    assert_eq!(path(target(s("5"), s("wayland-7"), None)), "fd 5");
    assert!(path(target(s("five"), None, s(run))).contains("not a file descriptor"));
    assert!(path(target(s("-1"), None, s(run))).contains("not a file descriptor"));
    assert!(path(target(None, None, None)).contains("XDG_RUNTIME_DIR"));
    assert!(path(target(None, s("wayland-1"), s("relative/dir"))).contains("XDG_RUNTIME_DIR"));

    match Connection::connect_to("/nonexistent/nl-wayland-socket") {
        Err(Error::NoCompositor(why)) => assert!(why.contains("/nonexistent/nl-wayland-socket")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_listening_socket_is_connected_to_by_path() {
    let dir = env::temp_dir().join(format!("nl-wayland-sock-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("wayland-test");
    let _ = std::fs::remove_file(&path);
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    let mut conn = Connection::connect_to(&path).unwrap();
    let (sock, _) = listener.accept().unwrap();
    let mut server = Server {
        sock,
        buf: Vec::new(),
        fds: VecDeque::new(),
    };
    conn.display().sync(&mut conn).unwrap();
    conn.flush().unwrap();
    server.expect(1, 0, &words(&[2]));
    assert!(conn.as_raw_fd() >= 0 && conn.as_fd().as_raw_fd() == conn.as_raw_fd());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn binding_a_version_the_tables_do_not_describe_is_refused() {
    let (mut conn, mut server) = pair();
    let registry = conn.display().get_registry(&mut conn).unwrap();
    conn.flush().unwrap();
    server.request().unwrap();
    let known = WlCompositor::interface().version;
    for bad in [0, known + 1, u32::MAX] {
        match registry.bind::<WlCompositor>(&mut conn, 1, bad) {
            Err(Error::UnsupportedVersion {
                interface,
                version,
                supported,
            }) => assert_eq!(
                (interface, version, supported),
                ("wl_compositor", bad, known)
            ),
            other => panic!("{bad}: {other:?}"),
        }
    }
    conn.flush().unwrap();
    assert_eq!(server.request(), None, "nothing was sent");
    let compositor: WlCompositor = registry.bind(&mut conn, 1, known).unwrap();
    assert_eq!(compositor.id(), 3, "and no id was used up");
}

#[test]
fn an_object_made_for_a_destroyed_object_keeps_its_place_and_stays_silent() {
    let (mut conn, server) = pair();
    let registry = conn.display().get_registry(&mut conn).unwrap();
    let manager: WlDataDeviceManager = registry.bind(&mut conn, 1, 3).unwrap();
    let seat: WlSeat = registry.bind(&mut conn, 2, 5).unwrap();
    let old = manager.get_data_device(&mut conn, seat).unwrap();
    let device = manager.get_data_device(&mut conn, seat).unwrap();
    old.release(&mut conn).unwrap();

    // The compositor had not seen the release yet: it makes an offer for
    // the old device, describes it, then makes one for the live device.
    server.send(&message(old.id(), 0, |b| b.uint(0xff00_0000)));
    server.send(&message(0xff00_0000, 0, |b| {
        b.string(Some("text/plain")).unwrap()
    }));
    server.send(&message(device.id(), 0, |b| b.uint(0xff00_0001)));
    let events = drain(&mut conn).unwrap();
    assert_eq!(events.len(), 1, "only the live device's offer");
    assert!(events[0].is_for(&device));
    assert!(conn.object(0xff00_0000).is_none());
    assert_eq!(conn.object(0xff00_0001).unwrap().0.name, "wl_data_offer");
}

#[test]
fn unparsed_bytes_survive_the_buffer_being_compacted() {
    let (mut conn, server) = pair();
    let registry = conn.display().get_registry(&mut conn).unwrap();
    // One write per batch: many small ones would fill the socket's buffer
    // with bookkeeping.
    let send = |from: u32, to: u32| {
        let mut bytes = Vec::new();
        for n in from..to {
            bytes.extend_from_slice(&global(registry.id(), n, "some_interface_name", 3));
        }
        server.send(&bytes);
    };
    let expect = |conn: &mut Connection, n: u32| match wl_registry::Event::decode(
        conn.next_event().unwrap().unwrap(),
    )
    .unwrap()
    {
        wl_registry::Event::Global { name, .. } => assert_eq!(name, n),
        other => panic!("{other:?}"),
    };
    // 300 events of 36 bytes; parse 200, leaving 3600 bytes behind 7200
    // already consumed, then read more: the consumed part is dropped.
    send(0, 300);
    while conn.read().unwrap() > 0 {}
    for n in 0..200 {
        expect(&mut conn, n);
    }
    assert!(conn.in_pos >= 4096);
    send(300, 310);
    assert!(conn.read().unwrap() > 0);
    assert_eq!(conn.in_pos, 0);
    for n in 200..310 {
        expect(&mut conn, n);
    }
    assert!(conn.next_event().unwrap().is_none());
}

#[test]
fn a_socket_error_while_reading_ends_the_connection() {
    // A descriptor that is not a socket: every read fails the same way.
    let file = file_with("not a socket");
    let fd = file.as_fd().try_clone_to_owned().unwrap();
    // SAFETY: an owned, open descriptor; that it is no socket is the test.
    let stream = unsafe { UnixStream::from_raw_fd(fd.into_raw_fd()) };
    let mut conn = Connection::from_stream(stream);
    // The read itself says nothing arrived; the failure is the next event.
    assert_eq!(conn.read().unwrap(), 0);
    assert!(matches!(conn.next_event(), Err(Error::Io(_))));
    assert!(matches!(conn.read(), Err(Error::Io(_))));
    assert!(matches!(conn.display().sync(&mut conn), Err(Error::Io(_))));
}

#[test]
fn events_read_before_the_compositor_hung_up_still_come_out() {
    let (mut conn, server) = pair();
    let registry = conn.display().get_registry(&mut conn).unwrap();
    server.send(&global(registry.id(), 1, "wl_seat", 9));
    server.send(&message(1, 0, |b| {
        b.uint(registry.id());
        b.uint(7);
        b.string(Some("the reason")).unwrap();
    }));
    drop(server);
    // Read until there is no more, as a loop woken by an edge must: the end
    // of the stream is not an error until what came before it is handed out.
    while conn.read().unwrap() > 0 {}
    assert_eq!(conn.read().unwrap(), 0);
    assert_eq!(conn.next_event().unwrap().unwrap().name(), "global");
    match conn.next_event() {
        Err(Error::Protocol { code, message, .. }) => {
            assert_eq!((code, message.as_str()), (7, "the reason"))
        }
        other => panic!("{other:?}"),
    }

    // With no error sent, the events come out and then the connection is
    // closed.
    let (mut conn, server) = pair();
    let registry = conn.display().get_registry(&mut conn).unwrap();
    server.send(&global(registry.id(), 1, "wl_seat", 9));
    drop(server);
    while conn.read().unwrap() > 0 {}
    assert_eq!(conn.next_event().unwrap().unwrap().name(), "global");
    assert!(matches!(conn.next_event(), Err(Error::Closed)));
    assert!(matches!(conn.read(), Err(Error::Closed)));
    assert!(matches!(conn.wait_event(), Err(Error::Closed)));
}

#[test]
fn a_zero_timeout_still_takes_what_is_waiting_in_the_socket() {
    let (mut conn, server) = pair();
    let registry = conn.display().get_registry(&mut conn).unwrap();
    server.send(&global(registry.id(), 1, "wl_output", 4));
    let e = conn.wait_event_timeout(Duration::ZERO).unwrap().unwrap();
    assert_eq!(e.name(), "global");
    assert!(conn.wait_event_timeout(Duration::ZERO).unwrap().is_none());
}

#[test]
fn a_delete_id_for_a_live_object_does_not_hand_its_id_to_another() {
    let (mut conn, server) = pair();
    let registry = conn.display().get_registry(&mut conn).unwrap();
    let seat: WlSeat = registry.bind(&mut conn, 2, 5).unwrap();
    // The compositor releases an id this side never destroyed.
    server.send(&message(1, 1, |b| b.uint(seat.id())));
    assert!(drain(&mut conn).unwrap().is_empty());
    let compositor: WlCompositor = registry.bind(&mut conn, 1, 4).unwrap();
    assert_ne!(compositor.id(), seat.id());
    assert_eq!(conn.object(seat.id()).unwrap().0.name, "wl_seat");
    // Once this side lets go of it too, the id is free.
    seat.release(&mut conn).unwrap();
    let shm: WlShm = registry.bind(&mut conn, 3, 1).unwrap();
    assert_eq!(shm.id(), seat.id());
}

#[test]
fn an_object_ended_by_its_own_event_is_gone_and_its_id_comes_back() {
    let (mut conn, server) = pair();
    let callback = conn.display().sync(&mut conn).unwrap();
    server.send(&message(callback.id(), 0, |b| b.uint(0)));
    let events = drain(&mut conn).unwrap();
    assert_eq!(events[0].name(), "done");
    assert!(conn.object(callback.id()).is_none(), "done ends a callback");
    // Not reused before the compositor says so.
    let second = conn.display().sync(&mut conn).unwrap();
    assert_ne!(second.id(), callback.id());
    server.send(&message(1, 1, |b| b.uint(callback.id())));
    assert!(drain(&mut conn).unwrap().is_empty());
    assert_eq!(conn.display().sync(&mut conn).unwrap().id(), callback.id());
}

#[test]
fn an_event_in_flight_to_a_destroyed_compositor_object_is_dropped_and_its_id_used_again() {
    let (mut conn, server) = pair();
    let registry = conn.display().get_registry(&mut conn).unwrap();
    let manager: WlDataDeviceManager = registry.bind(&mut conn, 1, 3).unwrap();
    let seat: WlSeat = registry.bind(&mut conn, 2, 5).unwrap();
    let device = manager.get_data_device(&mut conn, seat).unwrap();
    let keyboard = seat.get_keyboard(&mut conn).unwrap();
    server.send(&message(device.id(), 0, |b| b.uint(0xff00_0000)));
    let offer = match wl_data_device::Event::decode(drain(&mut conn).unwrap().remove(0)).unwrap() {
        wl_data_device::Event::DataOffer { id } => id,
        other => panic!("{other:?}"),
    };
    offer.destroy(&mut conn).unwrap();
    assert!(conn.object(offer.id()).is_none());
    // The compositor had not seen that yet: an event to the offer, then a
    // keymap with its descriptor. The keymap must get its own descriptor.
    server.send(&message(offer.id(), 0, |b| {
        b.string(Some("text/plain")).unwrap()
    }));
    let file = file_with("the keymap");
    server.send_fds(
        &message(keyboard.id(), 0, |b| {
            b.uint(1);
            b.uint(10);
        }),
        &[file.as_raw_fd()],
    );
    let mut events = drain(&mut conn).unwrap();
    assert_eq!(events.len(), 1);
    match wl_keyboard::Event::decode(events.remove(0)).unwrap() {
        wl_keyboard::Event::Keymap { fd, size, .. } => {
            assert_eq!(size, 10);
            let mut text = String::new();
            let mut f = File::from(fd);
            f.seek(SeekFrom::Start(0)).unwrap();
            f.read_to_string(&mut text).unwrap();
            assert_eq!(text, "the keymap");
        }
        other => panic!("{other:?}"),
    }
    // The compositor uses the id again for a new offer.
    server.send(&message(device.id(), 0, |b| b.uint(0xff00_0000)));
    assert_eq!(drain(&mut conn).unwrap().len(), 1);
    assert_eq!(conn.object(0xff00_0000).unwrap().0.name, "wl_data_offer");
}
