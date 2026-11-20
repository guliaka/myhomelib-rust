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
