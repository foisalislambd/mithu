//! HTML5 tokenizer and a practical tree builder.
//!
//! The tokenizer covers data, tags, attributes, comments, doctype, character
//! references, raw text (`script`, `style`), and RCDATA (`title`, `textarea`).
//! The tree builder inserts implied `html` / `head` / `body`, keeps a stack of
//! open elements, and records stylesheets, scripts, and the document title.
//!
//! It does not implement the adoption agency algorithm, so badly mis-nested
//! formatting tags are repaired by closing the stack rather than by Chromium's
//! full HTML recovery rules.

use mithu_dom::{Document, NodeId};

const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];

#[derive(Clone, Debug)]
enum Token {
    Doctype,
    Start {
        name: String,
        attrs: Vec<(String, String)>,
        self_closing: bool,
    },
    End {
        name: String,
    },
    Text(String),
    Comment,
    Eof,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    BeforeHtml,
    BeforeHead,
    InHead,
    AfterHead,
    InBody,
    Text,
    AfterBody,
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    mode: Mode,
    return_mode: Mode,
    raw: Option<(String, bool)>,
    raw_done: bool,
    pending: Option<Token>,
    doc: Document,
    stack: Vec<NodeId>,
}

pub fn parse_html(input: &str) -> Document {
    let mut parser = Parser {
        chars: input.chars().collect(),
        pos: 0,
        mode: Mode::BeforeHtml,
        return_mode: Mode::InBody,
        raw: None,
        raw_done: false,
        pending: None,
        doc: Document::new(),
        stack: Vec::new(),
    };
    parser.run();
    parser.doc
}

impl Parser {
    fn run(&mut self) {
        loop {
            let token = self.pull();
            if matches!(token, Token::Eof) {
                break;
            }
            self.dispatch(token);
        }
    }

    fn pull(&mut self) -> Token {
        if let Some(token) = self.pending.take() {
            token
        } else {
            self.tokenize()
        }
    }

    fn reconsume(&mut self, token: Token) {
        self.pending = Some(token);
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += 1;
        Some(ch)
    }

    fn tokenize(&mut self) -> Token {
        if self.raw.is_some() {
            return self.tokenize_raw();
        }
        self.skip_ignored();
        if self.pos >= self.chars.len() {
            return Token::Eof;
        }
        if self.peek() == Some('<') {
            return self.tokenize_tag();
        }
        self.tokenize_text(false)
    }

    fn tokenize_raw(&mut self) -> Token {
        let Some((name, rcdata)) = self.raw.clone() else {
            return Token::Eof;
        };
        if self.raw_done {
            self.raw = None;
            self.raw_done = false;
            return Token::End { name };
        }
        let text = self.read_until_end_tag(&name, rcdata);
        self.raw_done = true;
        Token::Text(text)
    }

    fn read_until_end_tag(&mut self, name: &str, rcdata: bool) -> String {
        let mut out = String::new();
        while self.pos < self.chars.len() {
            if self.looks_like_end_tag(name) {
                self.consume_end_tag();
                break;
            }
            let Some(ch) = self.bump() else { break };
            if rcdata && ch == '&' {
                self.pos -= 1;
                if let Some(entity) = self.try_entity() {
                    out.push(entity);
                } else {
                    self.pos += 1;
                    out.push('&');
                }
            } else {
                out.push(ch);
            }
        }
        out
    }

    fn looks_like_end_tag(&self, name: &str) -> bool {
        if self.peek() != Some('<') {
            return false;
        }
        let mut i = self.pos + 1;
        if self.chars.get(i) != Some(&'/') {
            return false;
        }
        i += 1;
        let rest: String = self.chars[i..].iter().collect();
        let lower = rest.to_ascii_lowercase();
        if !lower.starts_with(name) {
            return false;
        }
        match lower[name.len()..].chars().next() {
            None | Some('>' | '/') => true,
            Some(ch) => ch.is_whitespace(),
        }
    }

    fn consume_end_tag(&mut self) {
        while let Some(ch) = self.bump() {
            if ch == '>' {
                break;
            }
        }
    }

    fn tokenize_tag(&mut self) -> Token {
        let start = self.pos;
        self.bump();
        match self.peek() {
            Some('!') => {
                self.bump();
                self.tokenize_markup()
            }
            Some('/') => {
                self.bump();
                if !self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
                    self.pos = start + 1;
                    return self.tokenize_text(true);
                }
                let name = self.read_tag_name();
                self.skip_until_gt();
                Token::End { name }
            }
            Some(c) if c.is_ascii_alphabetic() => self.read_start_tag(),
            Some('?') => {
                self.skip_until_gt();
                Token::Comment
            }
            _ => {
                self.pos = start + 1;
                self.tokenize_text(true)
            }
        }
    }

    fn tokenize_markup(&mut self) -> Token {
        if self.starts_with("--") {
            self.pos += 2;
            while self.pos < self.chars.len() {
                if self.chars[self.pos] == '-'
                    && self.chars.get(self.pos + 1) == Some(&'-')
                    && self.chars.get(self.pos + 2) == Some(&'>')
                {
                    self.pos += 3;
                    break;
                }
                self.pos += 1;
            }
            return Token::Comment;
        }
        let rest: String = self.chars[self.pos..].iter().take(7).collect();
        if rest.to_ascii_lowercase().starts_with("doctype") {
            self.skip_until_gt();
            return Token::Doctype;
        }
        self.skip_until_gt();
        Token::Comment
    }

    fn read_start_tag(&mut self) -> Token {
        let name = self.read_tag_name();
        let mut attrs = Vec::new();
        let mut self_closing = false;
        loop {
            self.skip_ws();
            match self.peek() {
                None => break,
                Some('>') => {
                    self.bump();
                    break;
                }
                Some('/') => {
                    self.bump();
                    self_closing = true;
                    self.skip_ws();
                    if self.peek() == Some('>') {
                        self.bump();
                    }
                    break;
                }
                Some(_) => {
                    let attr_name = self.read_attr_name();
                    if attr_name.is_empty() {
                        self.bump();
                        continue;
                    }
                    self.skip_ws();
                    let value = if self.peek() == Some('=') {
                        self.bump();
                        self.skip_ws();
                        self.read_attr_value()
                    } else {
                        String::new()
                    };
                    if !attrs.iter().any(|(key, _)| key == &attr_name) {
                        attrs.push((attr_name, value));
                    }
                }
            }
        }
        Token::Start {
            name,
            attrs,
            self_closing,
        }
    }

    fn tokenize_text(&mut self, already_started: bool) -> Token {
        let mut out = String::new();
        if already_started {
            out.push('<');
        }
        while let Some(ch) = self.peek() {
            if ch == '<' {
                break;
            }
            if ch == '&' {
                if let Some(entity) = self.try_entity() {
                    out.push(entity);
                    continue;
                }
            }
            out.push(ch);
            self.pos += 1;
        }
        Token::Text(out)
    }

    fn read_tag_name(&mut self) -> String {
        let mut name = String::new();
        while let Some(ch) = self.peek() {
            if ch.is_whitespace() || ch == '>' || ch == '/' {
                break;
            }
            name.push(ch.to_ascii_lowercase());
            self.pos += 1;
        }
        name
    }

    fn read_attr_name(&mut self) -> String {
        let mut name = String::new();
        while let Some(ch) = self.peek() {
            if ch.is_whitespace() || ch == '=' || ch == '>' || ch == '/' {
                break;
            }
            name.push(ch.to_ascii_lowercase());
            self.pos += 1;
        }
        name
    }

    fn read_attr_value(&mut self) -> String {
        let quote = match self.peek() {
            Some('"') | Some('\'') => self.bump(),
            _ => None,
        };
        let mut value = String::new();
        loop {
            let Some(ch) = self.peek() else { break };
            if let Some(quote) = quote {
                if ch == quote {
                    self.bump();
                    break;
                }
            } else if ch.is_whitespace() || ch == '>' {
                break;
            }
            if ch == '&' {
                if let Some(entity) = self.try_entity() {
                    value.push(entity);
                    continue;
                }
            }
            value.push(ch);
            self.pos += 1;
        }
        value
    }

    fn try_entity(&mut self) -> Option<char> {
        let save = self.pos;
        if self.peek() != Some('&') {
            return None;
        }
        self.pos += 1;
        if self.peek() == Some('#') {
            self.pos += 1;
            let hex = matches!(self.peek(), Some('x') | Some('X'));
            if hex {
                self.pos += 1;
            }
            let start = self.pos;
            while self.peek().is_some_and(|ch| {
                if hex {
                    ch.is_ascii_hexdigit()
                } else {
                    ch.is_ascii_digit()
                }
            }) && self.pos - start < 8
            {
                self.pos += 1;
            }
            if self.pos > start && self.peek() == Some(';') {
                let digits: String = self.chars[start..self.pos].iter().collect();
                self.pos += 1;
                let code = if hex {
                    u32::from_str_radix(&digits, 16).ok()
                } else {
                    digits.parse().ok()
                };
                if let Some(ch) = code.and_then(char::from_u32) {
                    return Some(ch);
                }
            }
            self.pos = save;
            return None;
        }
        let start = self.pos;
        while self.peek().is_some_and(|ch| ch.is_ascii_alphanumeric()) && self.pos - start < 16 {
            self.pos += 1;
        }
        if self.pos > start && self.peek() == Some(';') {
            let name: String = self.chars[start..self.pos].iter().collect();
            self.pos += 1;
            if let Some(ch) = named_entity(&name) {
                return Some(ch);
            }
        }
        self.pos = save;
        None
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(|ch| ch.is_whitespace()) {
            self.pos += 1;
        }
    }

    fn skip_ignored(&mut self) {}

    fn skip_until_gt(&mut self) {
        while let Some(ch) = self.bump() {
            if ch == '>' {
                break;
            }
        }
    }

    fn starts_with(&self, text: &str) -> bool {
        self.chars[self.pos..].iter().collect::<String>().starts_with(text)
    }

    fn dispatch(&mut self, token: Token) {
        match self.mode {
            Mode::BeforeHtml => self.before_html(token),
            Mode::BeforeHead => self.before_head(token),
            Mode::InHead => self.in_head(token),
            Mode::AfterHead => self.after_head(token),
            Mode::InBody => self.in_body(token),
            Mode::Text => self.in_text(token),
            Mode::AfterBody => self.after_body(token),
        }
    }

    fn before_html(&mut self, token: Token) {
        match token {
            Token::Doctype | Token::Comment => {}
            Token::Start { name, attrs, .. } if name == "html" => {
                let id = self.insert("html");
                self.apply_attrs(id, attrs);
                self.mode = Mode::BeforeHead;
            }
            Token::End { name } if name == "head" || name == "body" || name == "html" => {}
            Token::Text(text) if text.chars().all(|ch| ch.is_whitespace()) => {}
            other => {
                self.insert("html");
                self.mode = Mode::BeforeHead;
                self.reconsume(other);
            }
        }
    }

    fn before_head(&mut self, token: Token) {
        match token {
            Token::Comment | Token::Doctype => {}
            Token::Text(text) if text.chars().all(|ch| ch.is_whitespace()) => {}
            Token::Start { name, attrs, .. } if name == "html" => {
                if let Some(html) = self.stack.first().copied() {
                    self.apply_attrs(html, attrs);
                }
            }
            Token::Start { name, attrs, .. } if name == "head" => {
                let id = self.insert("head");
                self.apply_attrs(id, attrs);
                self.mode = Mode::InHead;
            }
            Token::End { name } if name == "head" || name == "body" || name == "html" => {}
            other => {
                self.insert("head");
                self.mode = Mode::InHead;
                self.reconsume(other);
            }
        }
    }

    fn in_head(&mut self, token: Token) {
        match token {
            Token::Comment | Token::Doctype => {}
            Token::Text(text) if text.chars().all(|ch| ch.is_whitespace()) => {}
            Token::Start { name, attrs, .. } if matches!(name.as_str(), "title" | "style" | "script" | "textarea") => {
                self.start_raw(name, attrs);
            }
            Token::Start { name, attrs, .. } if is_void(&name) => {
                let id = self.insert(&name);
                self.apply_attrs(id, attrs);
                self.stack.pop();
            }
            Token::Start { name, attrs, .. } if name == "head" || name == "html" => {
                if name == "html"
                    && let Some(html) = self.stack.first().copied()
                {
                    self.apply_attrs(html, attrs);
                }
            }
            Token::End { name } if name == "head" => {
                self.pop_until("head");
                self.mode = Mode::AfterHead;
            }
            Token::End { name } if name == "html" => {
                self.pop_until("head");
                self.mode = Mode::AfterHead;
                self.reconsume(Token::End { name });
            }
            other => {
                self.pop_until("head");
                self.mode = Mode::AfterHead;
                self.reconsume(other);
            }
        }
    }

    fn after_head(&mut self, token: Token) {
        match token {
            Token::Comment | Token::Doctype => {}
            Token::Text(text) if text.chars().all(|ch| ch.is_whitespace()) => {}
            Token::Start { name, attrs, .. } if name == "body" => {
                let id = self.insert("body");
                self.apply_attrs(id, attrs);
                self.mode = Mode::InBody;
            }
            Token::Start { name, attrs, .. } if name == "html" => {
                if let Some(html) = self.stack.first().copied() {
                    self.apply_attrs(html, attrs);
                }
            }
            Token::Start { name, attrs, .. } if matches!(name.as_str(), "title" | "style" | "script" | "base" | "link" | "meta") => {
                self.mode = Mode::InHead;
                self.stack.push(self.head_id().unwrap_or(self.stack[0]));
                self.reconsume(Token::Start {
                    name,
                    attrs,
                    self_closing: false,
                });
            }
            other => {
                self.insert("body");
                self.mode = Mode::InBody;
                self.reconsume(other);
            }
        }
    }

    fn in_body(&mut self, token: Token) {
        match token {
            Token::Comment | Token::Doctype => {}
            Token::Text(text) => {
                if let Some(parent) = self.current() {
                    self.doc.append_text(parent, &text);
                }
            }
            Token::Start { name, attrs, self_closing } => self.start_in_body(name, attrs, self_closing),
            Token::End { name } => self.end_in_body(name),
            Token::Eof => {}
        }
    }

    fn start_in_body(&mut self, name: String, attrs: Vec<(String, String)>, _self_closing: bool) {
        if matches!(name.as_str(), "html") {
            return;
        }
        if name == "body" {
            return;
        }
        if name == "head" {
            return;
        }
        if matches!(name.as_str(), "title" | "style" | "script" | "textarea") {
            self.start_raw(name, attrs);
            return;
        }
        if is_block_close_p(&name) {
            self.close_p_if_open();
        }
        if name == "li" && self.current_name() == Some("li") {
            self.stack.pop();
        }
        if is_heading(&name) && self.current_name().is_some_and(|current| is_heading(current)) {
            self.stack.pop();
        }
        let id = self.insert(&name);
        self.apply_attrs(id, attrs);
        if is_void(&name) {
            self.stack.pop();
        }
    }

    fn end_in_body(&mut self, name: String) {
        match name.as_str() {
            "body" => self.mode = Mode::AfterBody,
            "html" => {
                self.mode = Mode::AfterBody;
                self.reconsume(Token::End { name });
            }
            "p" | "li" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "div" | "pre" | "button"
            | "form" | "ul" | "ol" | "section" | "article" | "header" | "footer" | "nav"
            | "main" | "blockquote" | "a" | "b" | "strong" | "i" | "em" | "span" | "code" => {
                self.pop_until(&name);
            }
            _ => self.pop_until(&name),
        }
    }

    fn in_text(&mut self, token: Token) {
        match token {
            Token::Text(text) => {
                if let Some(parent) = self.current() {
                    self.doc.append_text(parent, &text);
                }
            }
            Token::End { name } => {
                if self.current_name() == Some(name.as_str()) {
                    let id = self.stack.pop().unwrap();
                    self.capture_raw_element(id, &name);
                }
                self.mode = self.return_mode;
            }
            other => {
                self.mode = self.return_mode;
                self.reconsume(other);
            }
        }
    }

    fn after_body(&mut self, token: Token) {
        match token {
            Token::Comment | Token::Doctype => {}
            Token::Text(text) if text.chars().all(|ch| ch.is_whitespace()) => {}
            Token::End { name } if name == "html" => {}
            other => {
                self.mode = Mode::InBody;
                self.reconsume(other);
            }
        }
    }

    fn start_raw(&mut self, name: String, attrs: Vec<(String, String)>) {
        let rcdata = matches!(name.as_str(), "title" | "textarea");
        let id = self.insert(&name);
        self.apply_attrs(id, attrs);
        self.return_mode = if self.mode == Mode::Text {
            self.return_mode
        } else {
            self.mode
        };
        self.raw = Some((name, rcdata));
        self.raw_done = false;
        self.mode = Mode::Text;
    }

    fn capture_raw_element(&mut self, id: NodeId, name: &str) {
        let text = self.doc.text_content(id);
        match name {
            "title" => {
                self.doc.title = collapse_ws(text.trim());
            }
            "style" => {
                if !text.trim().is_empty() {
                    self.doc.stylesheets.push(text);
                }
            }
            "script" => self.doc.scripts.push(text),
            _ => {}
        }
    }

    fn insert(&mut self, name: &str) -> NodeId {
        let parent = self.current().unwrap_or(self.doc.root);
        let id = self.doc.create_element(parent, name);
        self.stack.push(id);
        id
    }

    fn apply_attrs(&mut self, id: NodeId, attrs: Vec<(String, String)>) {
        for (name, value) in attrs {
            self.doc.set_attr(id, &name, &value);
        }
        self.note_metadata(id);
    }

    fn note_metadata(&mut self, id: NodeId) {
        let Some(name) = self.doc.element_name(id).map(str::to_string) else {
            return;
        };
        if name == "link" {
            let rel = self.doc.attr(id, "rel").unwrap_or("").to_ascii_lowercase();
            let stylesheet = rel.split_whitespace().any(|part| part == "stylesheet");
            if stylesheet && let Some(href) = self.doc.attr(id, "href") {
                self.doc.stylesheet_links.push(href.to_string());
            }
        }
        if name == "base" && self.doc.base_href.is_none() && let Some(href) = self.doc.attr(id, "href")
        {
            self.doc.base_href = Some(href.to_string());
        }
    }

    fn current(&self) -> Option<NodeId> {
        self.stack.last().copied()
    }

    fn current_name(&self) -> Option<&str> {
        self.current().and_then(|id| self.doc.element_name(id))
    }

    fn head_id(&self) -> Option<NodeId> {
        self.doc.node(self.doc.root).children.iter().copied().find(|id| {
            self.doc.element_name(*id) == Some("html")
        }).and_then(|html| {
            self.doc.node(html).children.iter().copied().find(|id| {
                self.doc.element_name(*id) == Some("head")
            })
        })
    }

    fn close_p_if_open(&mut self) {
        if self.current_name() == Some("p") {
            self.stack.pop();
        }
    }

    fn pop_until(&mut self, tag: &str) {
        if !self.stack.iter().any(|id| self.doc.element_name(*id) == Some(tag)) {
            return;
        }
        while let Some(id) = self.stack.pop() {
            let name = self.doc.element_name(id).unwrap_or("");
            if name == tag {
                break;
            }
            if name == "html" || name == "body" {
                self.stack.push(id);
                break;
            }
        }
    }
}

fn is_void(name: &str) -> bool {
    VOID_ELEMENTS.contains(&name)
}

fn is_heading(name: &str) -> bool {
    matches!(name, "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
}

fn is_block_close_p(name: &str) -> bool {
    matches!(
        name,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "div"
            | "dl"
            | "fieldset"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hr"
            | "main"
            | "nav"
            | "ol"
            | "p"
            | "pre"
            | "section"
            | "table"
            | "ul"
    )
}

fn collapse_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn named_entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{00A0}',
        "copy" => '©',
        "reg" => '®',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "bull" => '•',
        "middot" => '·',
        "trade" => '™',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implies_html_head_and_body() {
        let doc = parse_html("<p id='hi' class='a b'>Hello <b>world</b></p>");
        let html = doc.node(doc.root).children[0];
        assert_eq!(doc.element_name(html), Some("html"));
        let children = &doc.node(html).children;
        assert_eq!(doc.element_name(children[0]), Some("head"));
        assert_eq!(doc.element_name(children[1]), Some("body"));
        let p = doc.get_element_by_id("hi").unwrap();
        assert!(doc.has_class(p, "a") && doc.has_class(p, "b"));
        assert_eq!(doc.text_content(p), "Hello world");
    }

    #[test]
    fn decodes_entities_and_keeps_raw_style() {
        let doc = parse_html("<title>A &amp; B</title><style>div > p { color: red }</style><p>&lt;x&gt;</p>");
        assert_eq!(doc.title, "A & B");
        assert_eq!(doc.stylesheets[0], "div > p { color: red }");
        let body = doc
            .node(doc.node(doc.root).children[0])
            .children
            .iter()
            .copied()
            .find(|id| doc.element_name(*id) == Some("body"))
            .unwrap();
        assert!(doc.text_content(body).contains("<x>"));
    }

    #[test]
    fn skips_comments_and_records_stylesheet_links() {
        let doc = parse_html("<link rel='stylesheet' href='app.css'><!-- <div id=no> --><p>Hi</p>");
        assert_eq!(doc.stylesheet_links, vec!["app.css".to_string()]);
        assert!(doc.get_element_by_id("no").is_none());
    }
}
