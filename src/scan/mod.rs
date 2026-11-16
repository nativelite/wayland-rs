//! The protocol scanner: reads a Wayland protocol description (the XML
//! files compositors and `wayland-protocols` publish) and writes the Rust
//! module for it, as `wayland-scanner` writes C.
//!
//! [`parse`] reads the XML into a [`Protocol`]; [`generate`] writes the
//! module. Only what the wire needs is read: interface and message names,
//! opcodes (the order of appearance), argument types, and enum values. The
//! descriptions and the copyright text are not carried into the output.
//!
//! ```
//! let xml = r#"<protocol name="demo">
//!   <interface name="demo_thing" version="2">
//!     <request name="poke"><arg name="how_hard" type="uint"/></request>
//!     <request name="destroy" type="destructor"/>
//!     <event name="poked" since="2"><arg name="serial" type="uint"/></event>
//!   </interface>
//! </protocol>"#;
//! let protocol = wayland::scan::parse(xml).unwrap();
//! assert_eq!(protocol.interfaces[0].requests[1].name, "destroy");
//! let code = wayland::scan::generate(&protocol, &wayland::scan::Options::default());
//! assert!(code.contains("pub fn poke(&self, conn: &mut ::wayland::Connection, how_hard: u32)"));
//! ```

mod gen;
mod xml;

pub use crate::wire::Kind;
pub use gen::{generate, Options};

use std::fmt;
use xml::{Tag, Tags};

/// A protocol file that could not be read: where and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanError {
    /// The line in the XML, counting from one.
    pub line: usize,
    /// What is wrong.
    pub message: String,
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ScanError {}

/// A protocol: a named set of interfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Protocol {
    /// The protocol's name, e.g. `xdg_shell`.
    pub name: String,
    /// Its interfaces, in file order.
    pub interfaces: Vec<Interface>,
}

/// An interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    /// Its name, e.g. `wl_surface`.
    pub name: String,
    /// Its version.
    pub version: u32,
    /// Requests; a request's index is its opcode.
    pub requests: Vec<Message>,
    /// Events; an event's index is its opcode.
    pub events: Vec<Message>,
    /// Enums.
    pub enums: Vec<Enum>,
}

/// A request or an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// Its name.
    pub name: String,
    /// The interface version that introduced it.
    pub since: u32,
    /// Whether it destroys its object: a request that does, or an event
    /// after which the compositor has.
    pub destructor: bool,
    /// Its arguments.
    pub args: Vec<Arg>,
}

/// An argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arg {
    /// Its name.
    pub name: String,
    /// Its wire type.
    pub kind: Kind,
    /// The interface of an object or new id, when the protocol names one.
    pub interface: Option<String>,
    /// Whether a string or object may be null.
    pub nullable: bool,
}

/// An enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enum {
    /// Its name.
    pub name: String,
    /// Whether its values are bits to combine.
    pub bitfield: bool,
    /// Its entries.
    pub entries: Vec<Entry>,
}

/// One value of an enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Its name; unlike every other name it may start with a digit.
    pub name: String,
    /// Its value.
    pub value: u32,
    /// The interface version that introduced it.
    pub since: u32,
}

fn attr<'a>(attrs: &'a [(&str, String)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.as_str())
}

/// Whether `s` can be written into Rust source as part of an identifier.
fn word(s: &str, digit_first: bool) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        Some(c) if c.is_ascii_digit() && digit_first => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn number(s: &str) -> Option<u32> {
    let s = s.trim();
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => s.parse().ok(),
    }
}

/// Reads a protocol description.
pub fn parse(src: &str) -> Result<Protocol, ScanError> {
    let mut tags = Tags::new(src);
    let mut protocol: Option<Protocol> = None;
    let mut iface: Option<Interface> = None;
    let mut message: Option<(bool, Message)> = None;
    let mut enumeration: Option<Enum> = None;

    macro_rules! fail {
        ($($t:tt)*) => {
            return Err(ScanError { line: tags.line(), message: format!($($t)*) })
        };
    }

    while let Some(tag) = tags.next()? {
        // An empty element is its own closing tag.
        let (name, attrs, closes) = match tag {
            Tag::Open { name, attrs, empty } => (name, Some(attrs), empty),
            Tag::Close(name) => (name, None, true),
        };
        if let Some(attrs) = attrs {
            let named = |digit_first: bool| -> Result<String, String> {
                match attr(&attrs, "name") {
                    Some(n) if word(n, digit_first) => Ok(n.to_string()),
                    Some(n) => Err(format!("<{name}> name {n:?} is not an identifier")),
                    None => Err(format!("<{name}> has no name")),
                }
            };
            let since = match attr(&attrs, "since").map(number) {
                None => 1,
                Some(Some(v)) if v >= 1 => v,
                Some(_) => fail!("<{name}> since is not a version"),
            };
            match name {
                "protocol" => {
                    if protocol.is_some() {
                        fail!("a second <protocol>");
                    }
                    let n = match named(false) {
                        Ok(n) => n,
                        Err(e) => fail!("{e}"),
                    };
                    protocol = Some(Protocol {
                        name: n,
                        interfaces: Vec::new(),
                    });
                }
                "interface" => {
                    let Some(p) = &protocol else {
                        fail!("<interface> outside <protocol>");
                    };
                    if iface.is_some() {
                        fail!("<interface> inside <interface>");
                    }
                    let n = match named(false) {
                        Ok(n) => n,
                        Err(e) => fail!("{e}"),
                    };
                    if p.interfaces.iter().any(|i| i.name == n) {
                        fail!("interface {n} is defined twice");
                    }
                    let version = match attr(&attrs, "version").map(number) {
                        Some(Some(v)) if v >= 1 => v,
                        _ => fail!("interface {n} has no valid version"),
                    };
                    iface = Some(Interface {
                        name: n,
                        version,
                        requests: Vec::new(),
                        events: Vec::new(),
                        enums: Vec::new(),
                    });
                }
                "request" | "event" => {
                    if iface.is_none() || message.is_some() || enumeration.is_some() {
                        fail!("<{name}> is not directly inside <interface>");
                    }
                    let n = match named(false) {
                        Ok(n) => n,
                        Err(e) => fail!("{e}"),
                    };
                    let destructor = match attr(&attrs, "type") {
                        None => false,
                        Some("destructor") => true,
                        Some(t) => fail!("<{name}> {n} has unknown type {t:?}"),
                    };
                    message = Some((
                        name == "request",
                        Message {
                            name: n,
                            since,
                            destructor,
                            args: Vec::new(),
                        },
                    ));
                }
                "arg" => {
                    let Some((is_request, m)) = &mut message else {
                        fail!("<arg> outside a request or event");
                    };
                    let n = match named(false) {
                        Ok(n) => n,
                        Err(e) => fail!("{e}"),
                    };
                    let kind = match attr(&attrs, "type") {
                        Some("int") => Kind::Int,
                        Some("uint") => Kind::Uint,
                        Some("fixed") => Kind::Fixed,
                        Some("string") => Kind::String,
                        Some("object") => Kind::Object,
                        Some("new_id") => Kind::NewId,
                        Some("array") => Kind::Array,
                        Some("fd") => Kind::Fd,
                        other => fail!("arg {n} has unknown type {other:?}"),
                    };
                    let interface = match attr(&attrs, "interface") {
                        None => None,
                        Some(i) if word(i, false) => Some(i.to_string()),
                        Some(i) => fail!("arg {n} interface {i:?} is not an identifier"),
                    };
                    if interface.is_some() && !matches!(kind, Kind::Object | Kind::NewId) {
                        fail!("arg {n} names an interface but is not an object");
                    }
                    if kind == Kind::NewId && interface.is_none() && !*is_request {
                        fail!("event arg {n} is a new_id without an interface");
                    }
                    let nullable = match attr(&attrs, "allow-null") {
                        None | Some("false") => false,
                        Some("true") if matches!(kind, Kind::String | Kind::Object) => true,
                        Some(v) => fail!("arg {n} has allow-null={v:?}"),
                    };
                    m.args.push(Arg {
                        name: n,
                        kind,
                        interface,
                        nullable,
                    });
                }
                "enum" => {
                    if iface.is_none() || message.is_some() || enumeration.is_some() {
                        fail!("<enum> is not directly inside <interface>");
                    }
                    let n = match named(false) {
                        Ok(n) => n,
                        Err(e) => fail!("{e}"),
                    };
                    let bitfield = match attr(&attrs, "bitfield") {
                        None | Some("false") => false,
                        Some("true") => true,
                        Some(v) => fail!("enum {n} has bitfield={v:?}"),
                    };
                    enumeration = Some(Enum {
                        name: n,
                        bitfield,
                        entries: Vec::new(),
                    });
                }
                "entry" => {
                    let Some(e) = &mut enumeration else {
                        fail!("<entry> outside <enum>");
                    };
                    let n = match named(true) {
                        Ok(n) => n,
                        Err(e) => fail!("{e}"),
                    };
                    let Some(value) = attr(&attrs, "value").and_then(number) else {
                        fail!("entry {n} has no valid value");
                    };
                    e.entries.push(Entry {
                        name: n,
                        value,
                        since,
                    });
                }
                // <description>, <copyright> and anything newer.
                _ => {}
            }
        }
        if closes {
            match name {
                "interface" => {
                    if message.is_some() || enumeration.is_some() {
                        fail!("</interface> inside a message or enum");
                    }
                    match (iface.take(), &mut protocol) {
                        (Some(i), Some(p)) => p.interfaces.push(i),
                        _ => fail!("</interface> without <interface>"),
                    }
                }
                "request" | "event" => match (message.take(), &mut iface) {
                    (Some((true, m)), Some(i)) if name == "request" => i.requests.push(m),
                    (Some((false, m)), Some(i)) if name == "event" => i.events.push(m),
                    _ => fail!("</{name}> does not close a <{name}>"),
                },
                "enum" => match (enumeration.take(), &mut iface) {
                    (Some(e), Some(i)) => i.enums.push(e),
                    _ => fail!("</enum> without <enum>"),
                },
                _ => {}
            }
        }
    }
    if iface.is_some() || message.is_some() || enumeration.is_some() {
        fail!("the file ends inside an element");
    }
    match protocol {
        Some(p) => Ok(p),
        None => fail!("no <protocol> element"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<protocol name="small">
  <copyright>Not carried over.</copyright>
  <interface name="sm_pool" version="3">
    <description summary="a pool">Long text with <b>markup</b>.</description>
    <enum name="flags" bitfield="true">
      <entry name="none" value="0"/>
      <entry name="90" value="0x10" since="2" summary="starts with a digit"/>
    </enum>
    <request name="make">
      <arg name="id" type="new_id" interface="sm_thing"/>
      <arg name="fd" type="fd"/>
      <arg name="label" type="string" allow-null="true"/>
    </request>
    <request name="destroy" type="destructor"/>
    <event name="gone" since="3">
      <arg name="thing" type="object" interface="sm_thing" allow-null="true"/>
      <arg name="where" type="fixed"/>
      <arg name="data" type="array"/>
      <arg name="n" type="int"/>
    </event>
  </interface>
  <interface name="sm_thing" version="1"/>
</protocol>
"#;

    #[test]
    fn a_protocol_is_read_into_its_interfaces_messages_and_enums() {
        let p = parse(SMALL).unwrap();
        assert_eq!(p.name, "small");
        assert_eq!(p.interfaces.len(), 2);
        let pool = &p.interfaces[0];
        assert_eq!((pool.name.as_str(), pool.version), ("sm_pool", 3));
        assert_eq!(pool.requests.len(), 2);
        let make = &pool.requests[0];
        assert_eq!(
            (make.name.as_str(), make.since, make.destructor),
            ("make", 1, false)
        );
        assert_eq!(
            make.args,
            [
                Arg {
                    name: "id".into(),
                    kind: Kind::NewId,
                    interface: Some("sm_thing".into()),
                    nullable: false
                },
                Arg {
                    name: "fd".into(),
                    kind: Kind::Fd,
                    interface: None,
                    nullable: false
                },
                Arg {
                    name: "label".into(),
                    kind: Kind::String,
                    interface: None,
                    nullable: true
                },
            ]
        );
        assert!(pool.requests[1].destructor && pool.requests[1].args.is_empty());
        let gone = &pool.events[0];
        assert_eq!((gone.name.as_str(), gone.since), ("gone", 3));
        assert_eq!(gone.args.len(), 4);
        assert!(gone.args[0].nullable);
        assert_eq!(
            pool.enums,
            [Enum {
                name: "flags".into(),
                bitfield: true,
                entries: vec![
                    Entry {
                        name: "none".into(),
                        value: 0,
                        since: 1
                    },
                    Entry {
                        name: "90".into(),
                        value: 16,
                        since: 2
                    },
                ]
            }]
        );
        assert_eq!(p.interfaces[1].name, "sm_thing");
        assert!(p.interfaces[1].requests.is_empty());
    }

    #[test]
    fn a_protocol_that_does_not_make_sense_is_refused_with_its_line() {
        let wrap = |body: &str| format!("<protocol name=\"p\">\n{body}\n</protocol>");
        let iface = |body: &str| {
            wrap(&format!(
                "<interface name=\"i\" version=\"1\">\n{body}\n</interface>"
            ))
        };
        for (src, line, what) in [
            ("<interface name=\"i\" version=\"1\"/>".to_string(), 1, "outside <protocol>"),
            ("".to_string(), 1, "no <protocol>"),
            (wrap("<interface name=\"i\"/>"), 2, "no valid version"),
            (wrap("<interface name=\"i\" version=\"0\"/>"), 2, "no valid version"),
            (wrap("<interface name=\"a b\" version=\"1\"/>"), 2, "not an identifier"),
            (wrap("<interface version=\"1\"/>"), 2, "has no name"),
            (
                wrap("<interface name=\"i\" version=\"1\"/>\n<interface name=\"i\" version=\"1\"/>"),
                3,
                "defined twice",
            ),
            (iface("<arg name=\"a\" type=\"int\"/>"), 3, "outside a request"),
            (iface("<request name=\"r\"><arg name=\"a\" type=\"float\"/></request>"), 3, "unknown type"),
            (iface("<request name=\"r\"><arg name=\"fn()\" type=\"int\"/></request>"), 3, "not an identifier"),
            (iface("<event name=\"e\" type=\"odd\"/>"), 3, "unknown type"),
            (iface("<event name=\"e\"><arg name=\"a\" type=\"new_id\"/></event>"), 3, "without an interface"),
            (iface("<request name=\"r\"><arg name=\"a\" type=\"int\" interface=\"x\"/></request>"), 3, "not an object"),
            (iface("<request name=\"r\"><arg name=\"a\" type=\"int\" allow-null=\"true\"/></request>"), 3, "allow-null"),
            (iface("<request name=\"r\" since=\"zero\"/>"), 3, "since"),
            (iface("<enum name=\"e\"><entry name=\"x\" value=\"ten\"/></enum>"), 3, "no valid value"),
            (iface("<entry name=\"x\" value=\"1\"/>"), 3, "outside <enum>"),
            (iface("<request name=\"r\"></event>"), 3, "does not close"),
            (iface("<request name=\"r\">"), 4, "inside a message"),
            ("<protocol name=\"p\"><interface name=\"i\" version=\"1\">".to_string(), 1, "ends inside"),
            (wrap("<protocol name=\"q\"/>"), 2, "second <protocol>"),
        ] {
            let e = parse(&src).unwrap_err();
            assert!(e.message.contains(what), "{src}\n{e}");
            assert_eq!(e.line, line, "{src}\n{e}");
        }
    }

    #[test]
    fn numbers_are_decimal_or_hex() {
        assert_eq!(number("12"), Some(12));
        assert_eq!(number("0x7fffffff"), Some(0x7fff_ffff));
        assert_eq!(number("0X10"), Some(16));
        assert_eq!(number("-1"), None);
        assert_eq!(number(""), None);
    }
}
