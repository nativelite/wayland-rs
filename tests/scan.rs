//! The scanner's output for the test protocol is the module checked in
//! beside it, which `tests/demo.rs` compiles and drives.

use wayland::scan::{generate, parse, Options};

fn lf(s: &str) -> String {
    s.replace("\r\n", "\n")
}

#[test]
fn the_demo_protocol_generates_the_checked_in_module() {
    let protocol = parse(include_str!("fixtures/demo.xml")).unwrap();
    let options = Options {
        source: "tests/fixtures/demo.xml".to_string(),
        ..Options::default()
    };
    let generated = generate(&protocol, &options);
    let checked_in = lf(include_str!("gen/demo.rs"));
    if generated != checked_in {
        let line = generated
            .lines()
            .zip(checked_in.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(generated.lines().count().min(checked_in.lines().count()));
        panic!(
            "tests/gen/demo.rs is out of date from line {}; regenerate it with\n\
             cargo run --example scan -- tests/fixtures/demo.xml \
             --source tests/fixtures/demo.xml --out tests/gen/demo.rs\n\
             generated: {:?}\nchecked in: {:?}",
            line + 1,
            generated.lines().nth(line),
            checked_in.lines().nth(line)
        );
    }
}

#[test]
fn nothing_but_names_types_and_values_is_carried_over() {
    let generated = generate(
        &parse(include_str!("fixtures/demo.xml")).unwrap(),
        &Options::default(),
    );
    for text in [
        "None of this text",
        "Nor may this",
        "a thing was bad",
        "the entry point",
    ] {
        assert!(!generated.contains(text), "{text:?} reached the output");
    }
    // Names that are Rust keywords, or the generated code's own, are moved
    // out of the way; an enum entry may start with a digit.
    for text in [
        "pub fn move_(&self, conn: &mut ::wayland::Connection, type_: u32, conn_: i32)",
        "pub fn bind<T: ::wayland::Proxy>(&self, conn: &mut ::wayland::Connection, name: u32, version: u32) -> Result<T, ::wayland::Error>",
        "pub fn make_item(&self, conn: &mut ::wayland::Connection, label: &str, parent: Option<DemoItem>) -> Result<DemoItem, ::wayland::Error>",
        "pub const _90: u32 = 0x1;",
        "pub const FLIPPED_180: u32 = 0x2;",
        "pub const WORSE_THING: u32 = 1;",
        "pub const REQ_MOVE_SINCE: u32 = 2;",
        "pub const REQ_DESTROY_SINCE: u32 = 3;",
        "pub const EVT_STATUS_SINCE: u32 = 2;",
        "        _Self {",
        "msg_.send_destructor()?;",
        "pub struct DemoMarker(u32);",
    ] {
        assert!(generated.contains(text), "{text:?} is missing");
    }
}
