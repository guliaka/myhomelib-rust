//! [`Fb2`] tree → FB2 XML bytes.
//!
//! Hand-rolled (no quick-xml Writer) so we fully control the output: it re-parses to an identical
//! element tree, which is what makes the round-trip test meaningful.

use super::model::{Element, Fb2, Node};

/// Serialize an FB2 document back to XML bytes.
pub fn serialize(fb2: &Fb2) -> Vec<u8> {
    let mut out = String::new();
    write_element(&fb2.root, &mut out);
    out.into_bytes()
}

fn write_element(el: &Element, out: &mut String) {
    out.push('<');
    out.push_str(&el.name);
    for (k, v) in &el.attrs {
        out.push(' ');
        out.push_str(k);
        out.push('=');
        out.push('"');
        escape_attr(v, out);
        out.push('"');
    }
    if el.children.is_empty() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    for n in &el.children {
        match n {
            Node::Text(t) => escape_text(t, out),
            Node::Element(c) => write_element(c, out),
        }
    }
    out.push_str("</");
    out.push_str(&el.name);
    out.push('>');
}

fn escape_text(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
}

fn escape_attr(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
}
