//! A module the scanner wrote for a protocol outside this crate, compiled
//! as a user's would be and driven over a socket pair: every argument type
//! goes out as the wire format says and comes back decoded.

#![cfg(target_os = "linux")]

#[allow(dead_code)]
#[rustfmt::skip]
#[path = "gen/demo.rs"]
mod demo;

use demo::{demo_hub, demo_item, DemoHub, DemoItem, DemoMarker};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use wayland::wire::{Builder, Fixed};
use wayland::{Connection, Error, Proxy};

fn message(object: u32, opcode: u16, f: impl FnOnce(&mut Builder<'_>)) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut b = Builder::new(&mut buf, object, opcode);
    f(&mut b);
    b.finish().unwrap();
    buf
}

fn temp_file(text: &str) -> File {
    let path = std::env::temp_dir().join(format!("nl-wayland-demo-{}-{text}", std::process::id()));
    let mut f = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .unwrap();
    std::fs::remove_file(&path).unwrap();
    f.write_all(text.as_bytes()).unwrap();
    f
}

#[test]
fn generated_requests_and_events_cross_the_wire_intact() {
    let (a, b) = UnixStream::pair().unwrap();
    let mut conn = Connection::from_stream(a);
    // The compositor's side: a second connection is used only to write
    // messages (with a descriptor) into the socket; requests are read raw.
    let mut reader = b.try_clone().unwrap();
    reader.set_nonblocking(true).unwrap();
    let mut server = Connection::from_stream(b);

    let registry = conn.display().get_registry(&mut conn).unwrap();
    let hub: DemoHub = registry.bind(&mut conn, 1, 3).unwrap();
    assert_eq!(conn.object(hub.id()).unwrap().0.name, "demo_hub");
    conn.flush().unwrap();
    let mut skipped = [0u8; 256];
    let _ = reader.read(&mut skipped).unwrap();

    // Requests.
    let item = hub.make_item(&mut conn, "first", None).unwrap();
    let child = hub.make_item(&mut conn, "", Some(item)).unwrap();
    let file = temp_file("request");
    hub.attach(
        &mut conn,
        file.as_fd(),
        -3,
        Fixed::from_f64(1.5),
        &[1, 2, 3],
        None,
        77,
    )
    .unwrap();
    hub.attach(
        &mut conn,
        file.as_fd(),
        0,
        Fixed::default(),
        &[],
        Some("n"),
        0,
    )
    .unwrap();
    hub.move_(
        &mut conn,
        demo_hub::turn::_90 | demo_hub::turn::FLIPPED_180,
        -1,
    )
    .unwrap();
    let marker: DemoMarker = hub.bind(&mut conn, 9, 1).unwrap();
    item.rename(&mut conn, "renamed").unwrap();
    child.destroy(&mut conn).unwrap();
    hub.destroy(&mut conn).unwrap();
    assert!(conn.object(child.id()).is_none() && conn.object(hub.id()).is_none());
    assert_eq!(conn.object(marker.id()).unwrap().0.name, "demo_marker");
    assert!(matches!(
        item.rename(&mut conn, "nul\0inside"),
        Err(Error::Wire(wayland::wire::WireError::InteriorNul))
    ));
    conn.flush().unwrap();

    let mut want = Vec::new();
    want.extend(message(hub.id(), 0, |b| {
        b.uint(item.id());
        b.string(Some("first")).unwrap();
        b.uint(0);
    }));
    want.extend(message(hub.id(), 0, |b| {
        b.uint(child.id());
        b.string(Some("")).unwrap();
        b.uint(item.id());
    }));
    want.extend(message(hub.id(), 1, |b| {
        b.int(-3);
        b.fixed(Fixed::from_raw(384));
        b.array(&[1, 2, 3]).unwrap();
        b.string(None).unwrap();
        b.uint(77);
    }));
    want.extend(message(hub.id(), 1, |b| {
        b.int(0);
        b.fixed(Fixed::from_raw(0));
        b.array(&[]).unwrap();
        b.string(Some("n")).unwrap();
        b.uint(0);
    }));
    want.extend(message(hub.id(), 3, |b| {
        b.uint(3);
        b.int(-1);
    }));
    want.extend(message(hub.id(), 2, |b| {
        b.uint(9);
        b.string(Some("demo_marker")).unwrap();
        b.uint(1);
        b.uint(marker.id());
    }));
    want.extend(message(item.id(), 0, |b| {
        b.string(Some("renamed")).unwrap()
    }));
    want.extend(message(child.id(), 1, |_| ()));
    want.extend(message(hub.id(), 4, |_| ()));
    let mut got = vec![0u8; 4096];
    let n = reader.read(&mut got).unwrap();
    assert_eq!(&got[..n], &want[..]);

    // Events. The hub is destroyed on the client now, so events to it
    // would be dropped; use a second hub.
    let hub: DemoHub = registry.bind(&mut conn, 1, 3).unwrap();
    let made = 0xff00_0000u32;
    let file = temp_file("event");
    let mut m = server.request(hub.id(), 0);
    m.uint(made);
    m.string(Some("made"));
    m.send().unwrap();
    let mut m = server.request(hub.id(), 1);
    m.fd(file.as_fd());
    m.int(-5);
    m.fixed(Fixed::from_int(-2));
    m.array(&[9, 8]);
    m.string(None);
    m.uint(made);
    m.uint(12);
    m.send().unwrap();
    let mut m = server.request(hub.id(), 1);
    m.fd(file.as_fd());
    m.int(1);
    m.fixed(Fixed::from_raw(1));
    m.array(&[]);
    m.string(Some("note"));
    m.uint(0);
    m.uint(3);
    m.send().unwrap();
    server.request(hub.id(), 2).send().unwrap();
    let mut m = server.request(made, 0);
    m.uint(5);
    m.send().unwrap();
    server.request(made, 1).send().unwrap();
    server.flush().unwrap();

    while conn.read().unwrap() > 0 {}
    let mut next = || conn.next_event().unwrap().expect("an event");
    let added: DemoItem = match demo_hub::Event::decode(next()).unwrap() {
        demo_hub::Event::ItemAdded { item, label } => {
            assert_eq!(label, "made");
            item
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(added.id(), made);
    match demo_hub::Event::decode(next()).unwrap() {
        demo_hub::Event::Status {
            fd,
            amount,
            scale,
            payload,
            note,
            item,
            anything,
        } => {
            let mut f = File::from(fd);
            f.seek(SeekFrom::Start(0)).unwrap();
            let mut text = String::new();
            f.read_to_string(&mut text).unwrap();
            assert_eq!(text, "event");
            assert_eq!((amount, scale.to_int(), payload), (-5, -2, vec![9, 8]));
            assert_eq!((note, item, anything), (None, Some(added), 12));
        }
        other => panic!("{other:?}"),
    }
    match demo_hub::Event::decode(next()).unwrap() {
        demo_hub::Event::Status {
            amount,
            scale,
            payload,
            note,
            item,
            anything,
            ..
        } => {
            assert_eq!((amount, scale.raw(), payload), (1, 1, vec![]));
            assert_eq!((note.as_deref(), item, anything), (Some("note"), None, 3));
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        demo_hub::Event::decode(next()).unwrap(),
        demo_hub::Event::Quiet
    ));
    assert!(matches!(
        demo_item::Event::decode(next()).unwrap(),
        demo_item::Event::_Self { serial: 5 }
    ));
    let gone = next();
    assert_eq!(gone.name(), "gone");
    assert!(matches!(
        demo_item::Event::decode(gone).unwrap(),
        demo_item::Event::Gone
    ));
    assert!(conn.next_event().unwrap().is_none());

    // The tables the module carries say what the XML said.
    let hub = &demo_hub::INTERFACE;
    assert_eq!((hub.name, hub.version), ("demo_hub", 3));
    assert_eq!(hub.requests.len(), 5);
    assert_eq!(hub.requests[3].name, "move");
    assert_eq!(hub.requests[3].since, 2);
    assert!(hub.requests[4].destructor && !hub.requests[0].destructor);
    assert_eq!(hub.events[1].args.len(), 7);
    assert_eq!(hub.requests[0].args[0].interface.unwrap().name, "demo_item");
    assert!(hub.requests[0].args[2].nullable);
    assert!(demo_item::INTERFACE.events[1].destructor);
    assert_eq!(DemoMarker::interface().name, "demo_marker");
    assert_eq!(demo_hub::error::WORSE_THING, 1);
}
