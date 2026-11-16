//! Lists what the running compositor offers: `cargo run --example globals`.

#[cfg(target_os = "linux")]
fn main() -> Result<(), wayland::Error> {
    use wayland::protocol::wayland::wl_registry;
    use wayland::Connection;

    let mut conn = Connection::connect()?;
    let registry = conn.display().get_registry(&mut conn)?;
    // One trip to the compositor and back: every global it has is announced
    // before the reply.
    conn.roundtrip()?;
    while let Some(event) = conn.next_event()? {
        if !event.is_for(&registry) {
            continue;
        }
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = wl_registry::Event::decode(event)?
        {
            println!("{name:>3}  {interface} v{version}");
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("globals: the connection is Linux only");
}
