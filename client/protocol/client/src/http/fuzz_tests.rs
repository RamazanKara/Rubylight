//! Property tests over the NvHTTP replies the client reads: serverinfo, app lists, launch
//! replies and the XML reader under them. See `crate::fuzz_tests`.

use proptest::collection::vec;
use proptest::prelude::*;
use proptest::sample::{select, Index};

use super::capabilities::{Evidence, HostCapabilities};
use super::serverinfo::{self, ServerInfo};
use super::xml::{self, Element, Node};
use crate::control::negotiation;
use crate::fuzz_tests::text;

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn write_element(e: &Element, out: &mut String) {
    out.push('<');
    out.push_str(&e.name);
    for (name, value) in &e.attributes {
        out.push_str(&format!(" {name}=\"{}\"", escape(value)));
    }
    if e.children.is_empty() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    for child in &e.children {
        match child {
            Node::Element(child) => write_element(child, out),
            Node::Text(t) => out.push_str(&escape(t)),
        }
    }
    out.push_str(&format!("</{}>", e.name));
}

fn written(e: &Element) -> String {
    let mut out = String::new();
    write_element(e, &mut out);
    out
}

fn name() -> impl Strategy<Value = String> {
    "[A-Za-z_][A-Za-z0-9_.-]{0,10}"
}

/// Text the reader keeps as written: no whitespace-only runs and nothing it reads as markup.
fn element_text() -> impl Strategy<Value = String> {
    prop_oneof!["[!-~]{1,12}", "[a-z][ -~\u{e9}\u{263a}]{0,12}[a-z]"]
}

fn element() -> impl Strategy<Value = Element> {
    let leaf = (name(), vec((name(), "[ -~]{0,8}"), 0..3)).prop_map(|(name, attributes)| {
        let mut seen = Vec::new();
        let attributes = attributes
            .into_iter()
            .filter(|(n, _)| {
                let fresh = !seen.contains(n);
                seen.push(n.clone());
                fresh
            })
            .collect();
        Element {
            name,
            attributes,
            children: Vec::new(),
        }
    });
    leaf.prop_recursive(4, 32, 4, |inner| {
        (
            inner.clone(),
            vec(
                prop_oneof![
                    inner.prop_map(Node::Element),
                    element_text().prop_map(Node::Text)
                ],
                0..4,
            ),
        )
            .prop_map(|(mut e, children)| {
                // The reader joins neighbouring text, so the tree has none.
                for child in children {
                    if let (Node::Text(t), Some(Node::Text(previous))) =
                        (&child, e.children.last_mut())
                    {
                        previous.push_str(t);
                        continue;
                    }
                    e.children.push(child);
                }
                e
            })
    })
}

proptest! {
    #[test]
    fn xml_round_trips(root in element()) {
        let text = written(&root);
        prop_assert_eq!(xml::parse(&text).ok(), Some(root), "{}", text);
    }

    #[test]
    fn xml_reads_anything(text in text(serverinfo::tests::RUBYLIGHT)) {
        if let Ok(root) = xml::parse(&text) {
            let _ = HostCapabilities::from_evidence(Evidence::parse(&root));
        }
        let _ = xml::parse_reply(&text);
    }

    #[test]
    fn xml_nesting_is_bounded(depth in 0usize..400, close in any::<bool>()) {
        let mut text = "<a>".repeat(depth);
        if close {
            text.push_str(&"</a>".repeat(depth));
        }
        let parsed = xml::parse(&text);
        prop_assert!(depth <= 65 || parsed.is_err(), "{} levels read", depth);
    }

    #[test]
    fn serverinfo_reads_anything(text in text(serverinfo::tests::RUBYLIGHT), port in any::<u16>()) {
        if let Ok(info) = ServerInfo::parse(&text, port) {
            let _ = (info.version_quad(), info.major_version(), info.supports_4k());
            let _ = (info.nvidia_server(), info.trusted_codec_mode_support());
        }
    }

    /// Every field of a real reply set to an awkward value.
    #[test]
    fn serverinfo_reads_any_field_value(
        edits in vec((any::<Index>(), prop_oneof![
            Just(String::new()),
            select(vec!["-1", "0", "4294967296", "18446744073709551616", "1.2.3.4.5", "7.1.431.-1", "-7.-1"])
                .prop_map(str::to_owned),
            "[ -~]{0,12}",
            "[0-9]{1,24}",
        ]), 1..6),
    ) {
        let mut root = xml::parse(serverinfo::tests::RUBYLIGHT).unwrap();
        for (at, value) in edits {
            let at = at.index(root.children.len());
            if let Node::Element(field) = &mut root.children[at] {
                field.children = if value.is_empty() { Vec::new() } else { vec![Node::Text(value)] };
            }
        }
        let _ = HostCapabilities::from_evidence(Evidence::parse(&root));
        if let Ok(info) = ServerInfo::parse(&written(&root), 47989) {
            let _ = (info.version_quad(), info.major_version(), info.supports_4k());
        }
    }

    #[test]
    fn host_replies_read_anything(text in text(serverinfo::tests::RUBYLIGHT), resume in any::<bool>()) {
        let _ = super::apps::parse_app_list(&text, &HostCapabilities::UNKNOWN);
        let _ = super::launch::parse_launch_reply(&text, resume);
        let _ = super::capabilities::parse_rubylight_version(&text);
        let _ = super::transport::Host::parse(&text);
        let _ = negotiation::parse_app_version(&text);
        let _ = serverinfo::parse_version_quad(&text);
    }

    #[test]
    fn certificates_read_anything(der in vec(any::<u8>(), 0..512)) {
        let _ = super::der::parse_certificate(&der);
    }
}
