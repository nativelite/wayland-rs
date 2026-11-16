//! The crate against the compositor actually running on this machine, when
//! there is one: connect, list globals, bind the ones a window needs, and
//! take an xdg_toplevel as far as its first configure.
//!
//! No buffer is ever attached, so the surface is never mapped and nothing
//! appears on screen. With no compositor to reach the test says so and
//! passes.

#![cfg(target_os = "linux")]

use std::os::fd::AsFd;
use std::time::{Duration, Instant};
use wayland::protocol::wayland::{
    wl_registry, wl_shm, WlBuffer, WlCompositor, WlShm, WlShmPool, WlSurface,
};
use wayland::protocol::xdg_shell::{xdg_surface, xdg_wm_base, XdgWmBase};
use wayland::{Connection, Error, Proxy};

#[test]
fn a_toplevel_reaches_its_first_configure_on_the_real_compositor() {
    let mut conn = match Connection::connect() {
        Ok(conn) => conn,
        Err(Error::NoCompositor(why)) => {
            eprintln!("live: skipped, no compositor ({why})");
            return;
        }
        Err(e) => panic!("{e}"),
    };

    // Globals.
    let registry = conn.display().get_registry(&mut conn).unwrap();
    conn.roundtrip().unwrap();
    let mut globals = Vec::new();
    while let Some(event) = conn.next_event().unwrap() {
        assert!(event.is_for(&registry));
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = wl_registry::Event::decode(event).unwrap()
        {
            globals.push((name, interface, version));
        }
    }
    eprintln!("live: {} globals", globals.len());
    let find = |wanted: &str| {
        globals
            .iter()
            .find(|(_, interface, _)| interface == wanted)
            .map(|(name, _, version)| (*name, *version))
            .unwrap_or_else(|| panic!("the compositor has no {wanted}: {globals:?}"))
    };
    let bind_version = |server: u32, ours: u32| server.min(ours);

    let (name, version) = find("wl_compositor");
    let compositor: WlCompositor = registry
        .bind(
            &mut conn,
            name,
            bind_version(version, WlCompositor::interface().version),
        )
        .unwrap();
    let (name, version) = find("wl_shm");
    let shm: WlShm = registry
        .bind(
            &mut conn,
            name,
            bind_version(version, WlShm::interface().version),
        )
        .unwrap();
    let (name, version) = find("xdg_wm_base");
    let wm_base: XdgWmBase = registry
        .bind(
            &mut conn,
            name,
            bind_version(version, XdgWmBase::interface().version),
        )
        .unwrap();

    // The compositor announces its pixel formats once wl_shm is bound.
    conn.roundtrip().unwrap();
    let mut formats = Vec::new();
    while let Some(event) = conn.next_event().unwrap() {
        if event.is_for(&shm) {
            let wl_shm::Event::Format { format } = wl_shm::Event::decode(event).unwrap();
            formats.push(format);
        }
    }
    assert!(
        formats.contains(&wl_shm::format::ARGB8888) && formats.contains(&wl_shm::format::XRGB8888),
        "every compositor supports these two: {formats:?}"
    );

    // A pool over a plain file and a buffer in it: a descriptor crosses to
    // the compositor, which maps it. The buffer is never attached.
    let path = std::env::temp_dir().join(format!("nl-wayland-live-{}", std::process::id()));
    let file = std::fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .unwrap();
    std::fs::remove_file(&path).unwrap();
    file.set_len(64 * 64 * 4).unwrap();
    let pool: WlShmPool = shm
        .create_pool(&mut conn, file.as_fd(), 64 * 64 * 4)
        .unwrap();
    drop(file);
    let buffer: WlBuffer = pool
        .create_buffer(&mut conn, 0, 64, 64, 64 * 4, wl_shm::format::ARGB8888)
        .unwrap();
    // A roundtrip here proves the compositor accepted the descriptor: a
    // bad one is a protocol error (wl_shm.invalid_fd).
    conn.roundtrip().unwrap();

    // A toplevel, committed with no buffer: the compositor answers with
    // the first configure.
    let surface: WlSurface = compositor.create_surface(&mut conn).unwrap();
    let xdg = wm_base.get_xdg_surface(&mut conn, surface).unwrap();
    let toplevel = xdg.get_toplevel(&mut conn).unwrap();
    toplevel
        .set_title(&mut conn, "nativelite-wayland live test")
        .unwrap();
    toplevel
        .set_app_id(&mut conn, "nativelite.wayland.test")
        .unwrap();
    surface.commit(&mut conn).unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut toplevel_configured = false;
    let serial = loop {
        let left = deadline
            .checked_duration_since(Instant::now())
            .expect("no configure within ten seconds");
        let Some(event) = conn.wait_event_timeout(left).unwrap() else {
            continue;
        };
        if event.is_for(&wm_base) {
            let xdg_wm_base::Event::Ping { serial } = xdg_wm_base::Event::decode(event).unwrap();
            wm_base.pong(&mut conn, serial).unwrap();
        } else if event.is_for(&toplevel) {
            toplevel_configured = true;
        } else if event.is_for(&xdg) {
            let xdg_surface::Event::Configure { serial } =
                xdg_surface::Event::decode(event).unwrap();
            break serial;
        }
    };
    assert!(toplevel_configured, "xdg_toplevel.configure comes first");
    xdg.ack_configure(&mut conn, serial).unwrap();

    // Take everything down again; the compositor objects to none of it.
    toplevel.destroy(&mut conn).unwrap();
    xdg.destroy(&mut conn).unwrap();
    surface.destroy(&mut conn).unwrap();
    buffer.destroy(&mut conn).unwrap();
    pool.destroy(&mut conn).unwrap();
    wm_base.destroy(&mut conn).unwrap();
    conn.roundtrip().unwrap();
    // None of them is an object any more.
    for id in [
        toplevel.id(),
        xdg.id(),
        surface.id(),
        buffer.id(),
        pool.id(),
    ] {
        assert!(conn.object(id).is_none());
    }
    eprintln!(
        "live: ran against the compositor: bound wl_compositor, wl_shm, xdg_wm_base; \
         configure serial {serial}"
    );
}
