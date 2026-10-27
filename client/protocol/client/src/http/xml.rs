//! A small XML reader for host replies, and the lookups NvHTTP.java does on them.

use super::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Element {
    pub name: String,
    pub attributes: Vec<(String, String)>,
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Node {
    Element(Element),
    Text(String),
}

impl Element {
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// The element's own text, as XmlPullParser.nextText() returns it.
    pub fn text(&self) -> String {
        self.children
            .iter()
            .filter_map(|c| match c {
                Node::Text(t) => Some(t.as_str()),
                Node::Element(_) => None,
            })
            .collect()
    }

    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|c| match c {
            Node::Element(e) => Some(e),
            Node::Text(_) => None,
        })
    }

    /// Every element below this one, in document order.
    pub fn descendants(&self) -> Vec<&Element> {
        let mut out = Vec::new();
        fn walk<'a>(e: &'a Element, out: &mut Vec<&'a Element>) {
            for child in e.elements() {
                out.push(child);
                walk(child, out);
            }
        }
        walk(self, &mut out);
        out
    }

    /// NvHTTP.getXmlString: the text of the first element with this name that has text.
    pub fn find_text(&self, name: &str) -> Option<String> {
        self.descendants()
            .into_iter()
            .filter(|e| e.name == name)
            .map(Element::text)
            .find(|t| !t.is_empty())
    }

    pub fn required_text(&self, name: &str) -> Result<String, Error> {
        self.find_text(name).ok_or_else(|| {
            Error::Malformed(format!("Missing mandatory field in host response: {name}"))
        })
    }
}

/// Parses a host reply and checks the root's `status_code` like NvHTTP.verifyResponseStatus.
pub(crate) fn parse_reply(text: &str) -> Result<Element, Error> {
    let root = parse(text)?;
    if root.name == "root" {
        verify_status(&root)?;
    }
    Ok(root)
}

fn verify_status(root: &Element) -> Result<(), Error> {
    // GFE can send 0xFFFFFFFF, which reads as -1.
    let code = root
        .attribute("status_code")
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|&v| v >= i32::MIN as i64 && v <= 0xFFFF_FFFF)
        .ok_or_else(|| Error::Malformed("Malformed status code in host response".into()))?
        as i32;
    if code == 200 {
        return Ok(());
    }
    let message = root.attribute("status_message").unwrap_or("").to_owned();
    if code == -1 && message == "Invalid" {
        // GFE's only report of a missing audio capture device.
        return Err(Error::Status {
            code: 418,
            message: "Missing audio capture device. Reinstall GeForce Experience.".into(),
        });
    }
    Err(Error::Status { code, message })
}

pub(crate) fn parse(text: &str) -> Result<Element, Error> {
    let mut parser = Parser { s: text, pos: 0 };
    parser.skip_misc()?;
    if !parser.rest().starts_with('<') {
        return Err(parser.error("expected the root element"));
    }
    let root = parser.element(0)?;
    parser.skip_misc()?;
    if parser.pos != text.len() {
        return Err(parser.error("content after the root element"));
    }
    Ok(root)
}

struct Parser<'a> {
    s: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        &self.s[self.pos..]
    }

    fn error(&self, what: &str) -> Error {
        Error::Malformed(format!("malformed XML at byte {}: {what}", self.pos))
    }

    fn skip_until(&mut self, end: &str) -> Result<&'a str, Error> {
        let rest = self.rest();
        let at = rest
            .find(end)
            .ok_or_else(|| self.error("unterminated markup"))?;
        self.pos += at + end.len();
        Ok(&rest[..at])
    }

    fn skip_whitespace(&mut self) {
        let rest = self.rest();
        self.pos += rest.len() - rest.trim_start().len();
    }

    /// Whitespace, the declaration, processing instructions, comments and DOCTYPE.
    fn skip_misc(&mut self) -> Result<(), Error> {
        loop {
            self.skip_whitespace();
            let rest = self.rest();
            if rest.starts_with("\u{feff}") {
                self.pos += 3;
            } else if rest.starts_with("<?") {
                self.skip_until("?>")?;
            } else if rest.starts_with("<!--") {
                self.skip_until("-->")?;
            } else if rest.starts_with("<!DOCTYPE") {
                self.skip_until(">")?;
            } else {
                return Ok(());
            }
        }
    }

    fn name(&mut self) -> Result<String, Error> {
        let rest = self.rest();
        let len = rest
            .find(|c: char| c.is_whitespace() || matches!(c, '>' | '/' | '='))
            .unwrap_or(rest.len());
        if len == 0 {
            return Err(self.error("expected a name"));
        }
        self.pos += len;
        Ok(rest[..len].to_owned())
    }

    fn element(&mut self, depth: usize) -> Result<Element, Error> {
        if depth > 64 {
            return Err(self.error("nesting too deep"));
        }
        self.pos += 1;
        let name = self.name()?;
        let mut attributes = Vec::new();
        loop {
            self.skip_whitespace();
            let rest = self.rest();
            if rest.starts_with("/>") {
                self.pos += 2;
                return Ok(Element {
                    name,
                    attributes,
                    children: Vec::new(),
                });
            }
            if rest.starts_with('>') {
                self.pos += 1;
                break;
            }
            let attribute = self.name()?;
            self.skip_whitespace();
            if !self.rest().starts_with('=') {
                return Err(self.error("expected '='"));
            }
            self.pos += 1;
            self.skip_whitespace();
            let quote = self
                .rest()
                .chars()
                .next()
                .ok_or_else(|| self.error("unexpected end"))?;
            if quote != '"' && quote != '\'' {
                return Err(self.error("expected a quoted value"));
            }
            self.pos += 1;
            let raw = self.skip_until(&quote.to_string())?;
            attributes.push((attribute, unescape(raw)));
        }
        let mut children = Vec::new();
        loop {
            let rest = self.rest();
            if rest.is_empty() {
                return Err(self.error(&format!("element <{name}> was not terminated")));
            }
            if rest.starts_with("</") {
                self.pos += 2;
                let end = self.name()?;
                self.skip_whitespace();
                if end != name || !self.rest().starts_with('>') {
                    return Err(self.error(&format!("</{end}> does not close <{name}>")));
                }
                self.pos += 1;
                return Ok(Element {
                    name,
                    attributes,
                    children,
                });
            } else if rest.starts_with("<!--") {
                self.skip_until("-->")?;
            } else if rest.starts_with("<![CDATA[") {
                self.pos += 9;
                let text = self.skip_until("]]>")?;
                push_text(&mut children, text.to_owned());
            } else if rest.starts_with("<?") {
                self.skip_until("?>")?;
            } else if rest.starts_with('<') {
                children.push(Node::Element(self.element(depth + 1)?));
            } else {
                let len = rest.find('<').unwrap_or(rest.len());
                self.pos += len;
                push_text(&mut children, unescape(&rest[..len]));
            }
        }
    }
}

fn push_text(children: &mut Vec<Node>, text: String) {
    if let Some(Node::Text(previous)) = children.last_mut() {
        previous.push_str(&text);
    } else {
        children.push(Node::Text(text));
    }
}

/// The predefined entities and character references; anything else stays as written.
fn unescape(raw: &str) -> String {
    if !raw.contains('&') {
        return raw.to_owned();
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest.find(';').filter(|&e| e <= 12) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .map(|hex| u32::from_str_radix(hex, 16))
                .or_else(|| entity.strip_prefix('#').map(str::parse::<u32>))
                .and_then(Result::ok)
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_host_replies_with_entities_and_cdata() {
        let root = parse_reply(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<root status_code=\"200\">\
             <hostname>A &amp; B &#x263A;</hostname><empty/><e></e>\
             <!-- note --><x><![CDATA[<raw>]]></x></root>",
        )
        .unwrap();
        assert_eq!(root.find_text("hostname").unwrap(), "A & B \u{263a}");
        assert_eq!(root.find_text("x").unwrap(), "<raw>");
        assert_eq!(root.find_text("empty"), None);
        assert_eq!(root.find_text("e"), None);
        assert!(root.required_text("missing").is_err());
    }

    #[test]
    fn status_codes_follow_nvhttp() {
        match parse_reply(
            "<root status_code=\"401\" status_message=\"The client is not authorized\"/>",
        ) {
            Err(Error::Status { code: 401, message }) => {
                assert_eq!(message, "The client is not authorized")
            }
            other => panic!("{other:?}"),
        }
        match parse_reply("<root status_code=\"4294967295\" status_message=\"Invalid\"/>") {
            Err(Error::Status { code: 418, .. }) => {}
            other => panic!("{other:?}"),
        }
        match parse_reply("<root status_code=\"4294967295\"/>") {
            Err(Error::Status { code: -1, .. }) => {}
            other => panic!("{other:?}"),
        }
        assert!(matches!(parse_reply("<root/>"), Err(Error::Malformed(_))));
        assert!(matches!(
            parse_reply("<root status_code=\"x\"/>"),
            Err(Error::Malformed(_))
        ));
    }

    #[test]
    fn malformed_documents_are_refused() {
        for text in [
            "",
            "<root>",
            "<root></other>",
            "<root a=b/>",
            "<root/><x/>",
            "text",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
    }
}
