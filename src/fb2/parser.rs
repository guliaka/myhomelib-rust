//! FB2 XML → [`Fb2`] tree parser, driven by quick-xml's streaming reader.
//!
//! Builds a lossless element tree: every Start/Empty/Text event becomes a node, and comments /
//! processing instructions / the XML declaration are dropped (they carry no book content). Text and
//! attribute values are entity-unescaped on parse so the serializer can re-escape them exactly once.

use quick_xml::events::{BytesStart, Event};
use quick_xml::{escape, Reader};

use crate::error::{AppError, Result};

use super::model::{Element, Fb2, Node};

/// Parse FB2 2.1 XML bytes into a lossless element tree rooted at `<FictionBook>`.
pub fn parse_fb2(data: &[u8]) -> Result<Fb2> {
    // Legacy libraries are full of windows-1251 / koi8-r / utf-16 files; quick-xml (as built here)
    // only reads UTF-8, so transcode first. UTF-8 input is borrowed untouched.
    let data = decode_xml(data);
    let data: &[u8] = data.as_ref();
    let mut reader = Reader::from_reader(data);
    let mut buf = Vec::new();
    let mut root_holder: Option<Element> = None;
    // Stack of elements currently open; innermost is last.
    let mut stack: Vec<Element> = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Err(e) => return Err(AppError::Other(format!("fb2 parse error: {e}"))),
            Ok(Event::Eof) => break,
            Ok(ev) => {
                if handle_event(ev, &mut stack, &mut root_holder)? {
                    break; // `<FictionBook>` closed — document complete
                }
            }
        }
        buf.clear();
    }

    match root_holder {
        Some(root) => Ok(Fb2 { root }),
        None => Err(AppError::Other("fb2: no <FictionBook> root element found".into())),
    }
}

/// Parse only the metadata of an FB2: everything up to `</description>`. Import needs the title-info
/// and nothing else, and skipping the body + base64 binaries makes scanning large archives several
/// times faster. Falls back to a full parse when no `</description>` is found.
pub fn parse_fb2_description(data: &[u8]) -> Result<Fb2> {
    const END: &[u8] = b"</description>";
    // UTF-16 is not ASCII-compatible, so the byte search below can't see the tag — decode it whole.
    let utf16 = data.starts_with(&[0xFF, 0xFE]) || data.starts_with(&[0xFE, 0xFF]);
    if !utf16 {
        if let Some(pos) = find_bytes(data, END) {
            let head = &data[..pos + END.len()];
            let mut text = decode_xml(head).into_owned();
            text.extend_from_slice(b"</FictionBook>");
            return parse_fb2(&text);
        }
    }
    parse_fb2(data)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Transcode an XML document to UTF-8 bytes: honours a BOM first, then the `encoding="…"` pseudo-
/// attribute of the XML declaration. Unknown labels and plain UTF-8 are returned as-is (borrowed).
pub fn decode_xml(data: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    use std::borrow::Cow;
    if let Some((enc, bom_len)) = encoding_rs::Encoding::for_bom(data) {
        if enc == encoding_rs::UTF_8 {
            return Cow::Borrowed(&data[bom_len..]);
        }
        let (text, _) = enc.decode_without_bom_handling(&data[bom_len..]);
        return Cow::Owned(text.into_owned().into_bytes());
    }
    let head = &data[..data.len().min(200)];
    let label = find_bytes(head, b"encoding=").and_then(|p| {
        let rest = &head[p + 9..];
        let quote = *rest.first()?;
        if quote != b'"' && quote != b'\'' {
            return None;
        }
        let end = rest[1..].iter().position(|&c| c == quote)?;
        Some(&rest[1..1 + end])
    });
    match label.and_then(encoding_rs::Encoding::for_label) {
        Some(enc) if enc != encoding_rs::UTF_8 => {
            let (text, _, _) = enc.decode(data);
            Cow::Owned(text.into_owned().into_bytes())
        }
        _ => Cow::Borrowed(data),
    }
}

/// Process one event, updating the tree. Returns `Ok(true)` when the document is complete.
fn handle_event(ev: Event, stack: &mut Vec<Element>, root: &mut Option<Element>) -> Result<bool> {
    match ev {
        Event::Start(tag) => {
            let el = element_from_start(&tag)?;
            if stack.is_empty() && root.is_none() {
                if el.name == "FictionBook" {
                    *root = Some(el);
                } else {
                    return Err(AppError::Other(format!(
                        "unexpected root <{}>; expected <FictionBook>",
                        el.name
                    )));
                }
            } else {
                stack.push(el);
            }
            Ok(false)
        }
        Event::Empty(tag) => {
            let el = element_from_start(&tag)?;
            push_child(stack, root, Node::Element(el));
            Ok(false)
        }
        Event::Text(t) => {
            // `into_inner()` yields the (entity-escaped) content; decode it once.
            let raw = t.into_inner();
            let text = escape::unescape(&raw)
                .map_err(|e| AppError::Other(format!("fb2 unescape error: {e}")))?;
            push_child(stack, root, Node::Text(text.into_owned()));
            Ok(false)
        }
        Event::End(tag) => {
            let name = tag.local_name().as_ref().to_string();
            if stack.is_empty() && name == "FictionBook" {
                return Ok(true); // document complete
            }
            let el = stack
                .pop()
                .ok_or_else(|| AppError::Other(format!("fb2: unbalanced </{name}>")))?;
            push_child(stack, root, Node::Element(el));
            Ok(false)
        }
        Event::Eof => Ok(true),
        // Everything else (comments, PI, decl, CDATA, doctype, general refs) is not book content.
        _ => Ok(false),
    }
}

/// Append a child node to the innermost open element, or to the root if none is open.
fn push_child(stack: &mut Vec<Element>, root: &mut Option<Element>, node: Node) {
    if let Some(top) = stack.last_mut() {
        top.children.push(node);
    } else if let Some(r) = root.as_mut() {
        r.children.push(node);
    }
}

fn element_from_start(tag: &BytesStart) -> Result<Element> {
    let name = tag.local_name().as_ref().to_string();
    let mut attrs = Vec::new();
    for a in tag.attributes() {
        let a = a.map_err(|e| AppError::Other(format!("fb2 attribute error: {e}")))?;
        // Unescape the value so it round-trips with exactly one escape on write.
        let val = escape::unescape(&a.value)
            .map_err(|e| AppError::Other(format!("fb2 attribute unescape error: {e}")))?;
        // Preserve the full (possibly prefixed) attribute name, e.g. "xlink:href".
        attrs.push((a.key.as_ref().to_string(), val.into_owned()));
    }
    Ok(Element {
        name,
        attrs,
        children: Vec::new(),
    })
}
