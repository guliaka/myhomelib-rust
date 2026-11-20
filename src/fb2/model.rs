//! FictionBook 2.1 data model.
//!
//! The model is a **generic element tree** that preserves every element, attribute and text run
//! exactly as parsed. This makes parse → serialize → parse round-trips lossless by construction.
//! Typed accessors on [`Fb2`] expose the fields MyHomeLib actually imports (title, authors, genre,
//! annotation, cover) without sacrificing fidelity for exotic markup.

/// A single node in an FB2 document.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// Character data, already entity-unescaped.
    Text(String),
    Element(Element),
}

/// An XML element: tag name, attributes (in document order) and child nodes.
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

impl Element {
    /// Attribute value by (full, possibly prefixed) name, if present.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// Direct child elements (text nodes skipped), in document order.
    pub fn children_elements(&self) -> Vec<&Element> {
        self.children
            .iter()
            .filter_map(|n| match n {
                Node::Element(e) => Some(e),
                _ => None,
            })
            .collect()
    }

    /// First direct child element with the given tag, if any.
    pub fn find(&self, name: &str) -> Option<&Element> {
        self.children_elements().into_iter().find(|e| e.name == name)
    }

    /// All descendant text concatenated (used for annotation / paragraph text).
    pub fn all_text(&self) -> String {
        let mut out = String::new();
        collect_text(self, &mut out);
        out
    }
}

fn collect_text(el: &Element, out: &mut String) {
    for n in &el.children {
        match n {
            Node::Text(t) => out.push_str(t),
            Node::Element(c) => collect_text(c, out),
        }
    }
}

/// A parsed FictionBook 2.1 document: the `<FictionBook>` root element and its tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Fb2 {
    pub root: Element,
}

impl Fb2 {
    /// `<description>` element, if present.
    pub fn description(&self) -> Option<&Element> {
        self.root.find("description")
    }

    /// `<title-info>` element, if present.
    pub fn title_info(&self) -> Option<&Element> {
        self.description().and_then(|d| d.find("title-info"))
    }

    /// Book title (`<book-title>` text).
    pub fn book_title(&self) -> Option<String> {
        self.title_info().and_then(|t| t.find("book-title")).map(|e| e.all_text())
    }

    /// Language code (`<lang>`), e.g. `"ru"`.
    pub fn lang(&self) -> Option<String> {
        self.title_info().and_then(|t| t.find("lang")).map(|e| e.all_text())
    }

    /// Genre code (first `<genre>`).
    pub fn genre(&self) -> Option<String> {
        self.title_info().and_then(|t| t.find("genre")).map(|e| e.all_text())
    }

    /// Authors from `<author>` elements, as last/first/middle triples.
    pub fn authors(&self) -> Vec<AuthorInfo> {
        let mut out = Vec::new();
        if let Some(ti) = self.title_info() {
            for a in ti.children_elements().into_iter().filter(|e| e.name == "author") {
                out.push(AuthorInfo {
                    last_name: field_text(a, "last-name"),
                    first_name: field_text(a, "first-name"),
                    middle_name: field_text(a, "middle-name"),
                });
            }
        }
        out
    }

    /// Annotation text (`<annotation>`), the blurb shown in book info.
    pub fn annotation(&self) -> Option<String> {
        self.title_info().and_then(|t| t.find("annotation")).map(|e| e.all_text())
    }

    /// Cover image bytes: follow `<coverpage><image xlink:href="#id">` to the matching `<binary id>`.
    pub fn cover(&self) -> Option<Vec<u8>> {
        let cp = self.title_info()?.find("coverpage")?;
        let img = cp.children_elements().into_iter().find(|e| e.name == "image")?;
        let href = img.attr("xlink:href")?.trim_start_matches('#').to_string();
        self.binary(&href)
    }

    /// A `<binary id>` payload decoded from base64.
    pub fn binary(&self, id: &str) -> Option<Vec<u8>> {
        for n in &self.root.children {
            if let Node::Element(b) = n {
                if b.name == "binary" && b.attr("id") == Some(id) {
                    return decode_b64(&b.all_text());
                }
            }
        }
        None
    }

    /// Top-level `<body><section>` elements.
    pub fn body_sections(&self) -> Vec<&Element> {
        match self.root.find("body") {
            Some(b) => b
                .children_elements()
                .into_iter()
                .filter(|e| e.name == "section")
                .collect(),
            None => Vec::new(),
        }
    }
}

fn field_text(parent: &Element, name: &str) -> Option<String> {
    parent.find(name).map(|e| e.all_text())
}

/// An FB2 author (mirrors `<author>` last/first/middle-name).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AuthorInfo {
    pub last_name: Option<String>,
    pub first_name: Option<String>,
    pub middle_name: Option<String>,
}

fn decode_b64(s: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let clean: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD.decode(clean).ok()
}
