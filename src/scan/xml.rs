//! The little of XML a protocol file uses: elements and their attributes.
//! Text, comments, processing instructions, CDATA and a DOCTYPE are skipped.

use super::ScanError;

/// A tag.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Tag<'a> {
    /// `<name a="v">`, or `<name a="v"/>` when `empty`.
    Open {
        name: &'a str,
        attrs: Vec<(&'a str, String)>,
        empty: bool,
    },
    /// `</name>`.
    Close(&'a str),
}

pub(super) struct Tags<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Tags<'a> {
    pub(super) fn new(src: &'a str) -> Tags<'a> {
        Tags { src, pos: 0 }
    }

    /// The line the reader is on, counting from one.
    pub(super) fn line(&self) -> usize {
        1 + self.src.as_bytes()[..self.pos]
            .iter()
            .filter(|&&b| b == b'\n')
            .count()
    }

    fn err<T>(&self, what: &str) -> Result<T, ScanError> {
        Err(ScanError {
            line: self.line(),
            message: what.to_string(),
        })
    }

    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    /// Moves past the next `end`, or fails naming `what` was left open.
    fn skip_past(&mut self, end: &str, what: &str) -> Result<(), ScanError> {
        match self.rest().find(end) {
            Some(i) => {
                self.pos += i + end.len();
                Ok(())
            }
            None => self.err(&format!("unterminated {what}")),
        }
    }

    fn skip_space(&mut self) {
        let rest = self.rest();
        self.pos += rest.len() - rest.trim_start().len();
    }

    fn name(&mut self) -> Result<&'a str, ScanError> {
        let rest = self.rest();
        let end = rest
            .find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '-' | ':' | '.')))
            .unwrap_or(rest.len());
        if end == 0 {
            return self.err("expected a name");
        }
        self.pos += end;
        Ok(&rest[..end])
    }

    /// The next tag, or `None` at the end of the document.
    pub(super) fn next(&mut self) -> Result<Option<Tag<'a>>, ScanError> {
        loop {
            match self.rest().find('<') {
                Some(i) => self.pos += i,
                None => {
                    self.pos = self.src.len();
                    return Ok(None);
                }
            }
            let rest = self.rest();
            if rest.starts_with("<!--") {
                self.skip_past("-->", "comment")?;
            } else if rest.starts_with("<![CDATA[") {
                self.skip_past("]]>", "CDATA section")?;
            } else if rest.starts_with("<?") {
                self.skip_past("?>", "processing instruction")?;
            } else if rest.starts_with("<!") {
                // A DOCTYPE, with or without an internal subset.
                let close = rest.find('>').unwrap_or(rest.len());
                match rest[..close].find('[') {
                    Some(_) => self.skip_past("]>", "DOCTYPE")?,
                    None => self.skip_past(">", "DOCTYPE")?,
                }
            } else if rest.starts_with("</") {
                self.pos += 2;
                let name = self.name()?;
                self.skip_space();
                if !self.rest().starts_with('>') {
                    return self.err("expected '>' to end a closing tag");
                }
                self.pos += 1;
                return Ok(Some(Tag::Close(name)));
            } else {
                self.pos += 1;
                return self.open().map(Some);
            }
        }
    }

    fn open(&mut self) -> Result<Tag<'a>, ScanError> {
        let name = self.name()?;
        let mut attrs = Vec::new();
        loop {
            self.skip_space();
            let rest = self.rest();
            if rest.starts_with("/>") {
                self.pos += 2;
                return Ok(Tag::Open {
                    name,
                    attrs,
                    empty: true,
                });
            }
            if rest.starts_with('>') {
                self.pos += 1;
                return Ok(Tag::Open {
                    name,
                    attrs,
                    empty: false,
                });
            }
            if rest.is_empty() {
                return self.err("unterminated tag");
            }
            let key = self.name()?;
            self.skip_space();
            if !self.rest().starts_with('=') {
                return self.err("expected '=' after an attribute name");
            }
            self.pos += 1;
            self.skip_space();
            let rest = self.rest();
            let quote = match rest.chars().next() {
                Some(q @ ('"' | '\'')) => q,
                _ => return self.err("expected a quoted attribute value"),
            };
            let Some(end) = rest[1..].find(quote) else {
                return self.err("unterminated attribute value");
            };
            let raw = &rest[1..1 + end];
            let value = match unescape(raw) {
                Some(v) => v,
                None => return self.err("unknown entity in an attribute value"),
            };
            self.pos += end + 2;
            attrs.push((key, value));
        }
    }
}

/// `raw` with the predefined entities and character references replaced;
/// `None` if one is not recognised.
fn unescape(raw: &str) -> Option<String> {
    if !raw.contains('&') {
        return Some(raw.to_string());
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        let end = rest.find(';')?;
        let entity = &rest[..end];
        rest = &rest[end + 1..];
        let c = match entity {
            "lt" => '<',
            "gt" => '>',
            "amp" => '&',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let code = match entity.strip_prefix("#x") {
                    Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                    None => entity.strip_prefix('#')?.parse().ok()?,
                };
                char::from_u32(code)?
            }
        };
        out.push(c);
    }
    out.push_str(rest);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(src: &str) -> Result<Vec<Tag<'_>>, ScanError> {
        let mut t = Tags::new(src);
        let mut out = Vec::new();
        while let Some(tag) = t.next()? {
            out.push(tag);
        }
        Ok(out)
    }

    #[test]
    fn tags_and_attributes_come_out_and_everything_else_is_skipped() {
        let src = "<?xml version=\"1.0\"?>\n<!DOCTYPE p [ <!ENTITY x \"y\"> ]>\n\
                   <!-- a <comment> -->\n<p name='a&lt;b&#65;&#x42;' n = \"1\">text \
                   <![CDATA[ <not a tag> ]]><q/><r a=\"\"  /></p >";
        let tags = all(src).unwrap();
        assert_eq!(
            tags,
            [
                Tag::Open {
                    name: "p",
                    attrs: vec![("name", "a<bAB".into()), ("n", "1".into())],
                    empty: false
                },
                Tag::Open {
                    name: "q",
                    attrs: vec![],
                    empty: true
                },
                Tag::Open {
                    name: "r",
                    attrs: vec![("a", String::new())],
                    empty: true
                },
                Tag::Close("p"),
            ]
        );
    }

    #[test]
    fn broken_markup_is_an_error_with_its_line() {
        for (src, line) in [
            ("<a>\n<!-- never closed", 2),
            ("<a b>", 1),
            ("\n\n<a b=c>", 3),
            ("<a b=\"c>", 1),
            ("<a b=\"&nope;\">", 1),
            ("<a", 1),
            ("< a>", 1),
            ("</a b>", 1),
            ("<?xml", 1),
        ] {
            let e = all(src).unwrap_err();
            assert_eq!(e.line, line, "{src:?}: {e:?}");
        }
        assert_eq!(all("").unwrap(), []);
        assert_eq!(all("just text").unwrap(), []);
    }
}
