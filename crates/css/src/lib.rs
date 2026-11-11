//! CSS parser for the subset Mithu's style engine understands.
//!
//! Selectors support type, class, id, attributes, structural and link
//! pseudos, and the descendant, child, and sibling combinators. `@media`
//! keeps `min-width` and `max-width` queries. `@layer` and `@supports`
//! are transparent wrappers so the rules inside still apply. Other
//! at-rules are skipped. Declarations stay as strings so the style
//! crate can interpret them.

#[derive(Clone, Debug, PartialEq)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub selectors: Vec<Selector>,
    pub declarations: Vec<Declaration>,
    pub media: Option<Media>,
}

/// A width media query. An empty bound means that side is unrestricted.
#[derive(Clone, Debug, PartialEq)]
pub struct Media {
    pub alternatives: Vec<MediaBound>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MediaBound {
    pub min_width: Option<f32>,
    pub max_width: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Selector {
    pub compounds: Vec<Compound>,
    pub combinators: Vec<Combinator>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Compound {
    pub tag: Option<String>,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub attrs: Vec<AttrSelector>,
    pub pseudos: Vec<Pseudo>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttrSelector {
    pub name: String,
    pub op: Option<AttrOp>,
    pub value: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttrOp {
    Exact,
    Includes,
    Dash,
    Prefix,
    Suffix,
    Substring,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Pseudo {
    FirstChild,
    LastChild,
    Nth(i32, i32),
    Link,
    Empty,
    Not(Box<Compound>),
    /// `:is()`. Specificity is the most specific argument.
    Is(Vec<Selector>),
    /// `:where()`. Specificity is zero.
    Where(Vec<Selector>),
    Hover,
    Root,
    Before,
    After,
    /// `:focus`, `:active`, and other pseudos that the style engine does not match yet.
    Unmatched,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Combinator {
    Descendant,
    Child,
    NextSibling,
    SubsequentSibling,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Declaration {
    pub name: String,
    pub value: String,
    pub important: bool,
}

impl Selector {
    pub fn specificity(&self) -> u32 {
        let mut ids = 0;
        let mut classes = 0;
        let mut types = 0;
        for compound in &self.compounds {
            let (more_ids, more_classes, more_types) = compound_specificity(compound);
            ids += more_ids;
            classes += more_classes;
            types += more_types;
        }
        ids * 100 + classes * 10 + types
    }
}

fn compound_specificity(compound: &Compound) -> (u32, u32, u32) {
    let mut ids = u32::from(compound.id.is_some());
    let mut classes = compound.classes.len() as u32 + compound.attrs.len() as u32;
    let mut types = u32::from(compound.tag.is_some());
    for pseudo in &compound.pseudos {
        match pseudo {
            Pseudo::Not(inner) => {
                let (more_ids, more_classes, more_types) = compound_specificity(inner);
                ids += more_ids;
                classes += more_classes;
                types += more_types;
            }
            Pseudo::Is(list) => {
                let best = list.iter().map(Selector::specificity).max().unwrap_or(0);
                ids += best / 100;
                classes += (best / 10) % 10;
                types += best % 10;
            }
            Pseudo::Where(_) => {}
            Pseudo::Before | Pseudo::After => types += 1,
            Pseudo::Unmatched => {}
            _ => classes += 1,
        }
    }
    (ids, classes, types)
}

pub fn parse_css(input: &str) -> Stylesheet {
    let chars = strip_comments(input);
    let mut parser = CssParser {
        chars: chars.chars().collect(),
        pos: 0,
    };
    parser.parse_stylesheet()
}

pub fn parse_declarations(input: &str) -> Vec<Declaration> {
    let chars = strip_comments(input);
    let mut parser = CssParser {
        chars: chars.chars().collect(),
        pos: 0,
    };
    parser.parse_declaration_list()
}

struct CssParser {
    chars: Vec<char>,
    pos: usize,
}

impl CssParser {
    fn parse_stylesheet(&mut self) -> Stylesheet {
        let mut rules = Vec::new();
        self.parse_rule_list(&mut rules, None);
        Stylesheet { rules }
    }

    fn parse_selector_list(&mut self) -> Vec<Selector> {
        let mut selectors = Vec::new();
        loop {
            self.skip_ws();
            if matches!(self.peek(), None | Some('{') | Some('}')) {
                break;
            }
            if let Some(selector) = self.parse_selector() {
                selectors.push(selector);
            } else {
                break;
            }
            self.skip_ws();
            if self.peek() == Some(',') {
                self.bump();
                continue;
            }
            break;
        }
        selectors
    }

    fn parse_selector(&mut self) -> Option<Selector> {
        let mut compounds = Vec::new();
        let mut combinators = Vec::new();
        let first = self.parse_compound()?;
        compounds.push(first);
        loop {
            let before = self.pos;
            let saw_ws = self.skip_ws_counting();
            match self.peek() {
                Some('{') | Some(',') | Some('}') | Some(')') | None => {
                    self.pos = if saw_ws { before } else { self.pos };
                    if saw_ws && matches!(self.peek(), Some('{') | Some(',') | Some('}')) {
                        // Whitespace before the block is not a combinator.
                    }
                    break;
                }
                Some('>') | Some('+') | Some('~') => {
                    let symbol = self.bump();
                    self.skip_ws();
                    let Some(next) = self.parse_compound() else {
                        break;
                    };
                    combinators.push(match symbol {
                        Some('+') => Combinator::NextSibling,
                        Some('~') => Combinator::SubsequentSibling,
                        _ => Combinator::Child,
                    });
                    compounds.push(next);
                }
                Some(_) if saw_ws => {
                    let Some(next) = self.parse_compound() else {
                        self.pos = before;
                        break;
                    };
                    combinators.push(Combinator::Descendant);
                    compounds.push(next);
                }
                _ => break,
            }
        }
        if compounds.is_empty() {
            None
        } else {
            Some(Selector {
                compounds,
                combinators,
            })
        }
    }

    fn parse_compound(&mut self) -> Option<Compound> {
        let mut compound = Compound {
            tag: None,
            id: None,
            classes: Vec::new(),
            attrs: Vec::new(),
            pseudos: Vec::new(),
        };
        let mut any = false;
        loop {
            match self.peek() {
                Some('*') => {
                    self.bump();
                    any = true;
                }
                Some('#') => {
                    self.bump();
                    let id = self.read_ident();
                    if id.is_empty() {
                        break;
                    }
                    compound.id = Some(id);
                    any = true;
                }
                Some('.') => {
                    self.bump();
                    let class = self.read_ident();
                    if class.is_empty() {
                        break;
                    }
                    compound.classes.push(class);
                    any = true;
                }
                Some(ch) if is_ident_start(ch) => {
                    let tag = self.read_ident().to_ascii_lowercase();
                    if tag.is_empty() {
                        break;
                    }
                    compound.tag = Some(tag);
                    any = true;
                }
                Some('[') => {
                    if let Some(attr) = self.parse_attribute() {
                        compound.attrs.push(attr);
                        any = true;
                    } else {
                        break;
                    }
                }
                Some(':') => {
                    self.bump();
                    if self.peek() == Some(':') {
                        self.bump();
                        let name = self.read_ident().to_ascii_lowercase();
                        match name.as_str() {
                            "before" => compound.pseudos.push(Pseudo::Before),
                            "after" => compound.pseudos.push(Pseudo::After),
                            _ => {
                                self.skip_function_args();
                                compound.pseudos.push(Pseudo::Unmatched);
                            }
                        }
                        any = true;
                        continue;
                    }
                    let name = self.read_ident().to_ascii_lowercase();
                    match name.as_str() {
                        "first-child" => compound.pseudos.push(Pseudo::FirstChild),
                        "last-child" => compound.pseudos.push(Pseudo::LastChild),
                        "link" | "visited" => compound.pseudos.push(Pseudo::Link),
                        "empty" => compound.pseudos.push(Pseudo::Empty),
                        "hover" => compound.pseudos.push(Pseudo::Hover),
                        "root" => compound.pseudos.push(Pseudo::Root),
                        "focus" | "active" => compound.pseudos.push(Pseudo::Unmatched),
                        "nth-child" => compound.pseudos.push(self.parse_nth()),
                        "not" => {
                            if let Some(inner) = self.parse_not() {
                                compound.pseudos.push(Pseudo::Not(Box::new(inner)));
                            } else {
                                compound.pseudos.push(Pseudo::Unmatched);
                            }
                        }
                        "is" => {
                            let list = self.parse_group();
                            if list.is_empty() {
                                compound.pseudos.push(Pseudo::Unmatched);
                            } else {
                                compound.pseudos.push(Pseudo::Is(list));
                            }
                        }
                        "where" => compound.pseudos.push(Pseudo::Where(self.parse_group())),
                        _ => {
                            self.skip_function_args();
                            compound.pseudos.push(Pseudo::Unmatched);
                        }
                    }
                    any = true;
                }
                _ => break,
            }
        }
        any.then_some(compound)
    }

    fn parse_attribute(&mut self) -> Option<AttrSelector> {
        if self.peek() != Some('[') {
            return None;
        }
        self.bump();
        self.skip_ws();
        let name = self.read_ident().to_ascii_lowercase();
        if name.is_empty() {
            return None;
        }
        self.skip_ws();
        let op = match self.peek() {
            Some(']') => None,
            Some('=') => {
                self.bump();
                Some(AttrOp::Exact)
            }
            Some(symbol) if matches!(symbol, '~' | '|' | '^' | '$' | '*') => {
                self.bump();
                if self.peek() != Some('=') {
                    return None;
                }
                self.bump();
                Some(match symbol {
                    '~' => AttrOp::Includes,
                    '|' => AttrOp::Dash,
                    '^' => AttrOp::Prefix,
                    '$' => AttrOp::Suffix,
                    _ => AttrOp::Substring,
                })
            }
            _ => return None,
        };
        self.skip_ws();
        let value = if op.is_some() {
            let value = if self.peek() == Some('"') || self.peek() == Some('\'') {
                self.read_string()
            } else {
                self.read_ident()
            };
            Some(value)
        } else {
            None
        };
        self.skip_ws();
        if self.peek() == Some(']') {
            self.bump();
        }
        Some(AttrSelector { name, op, value })
    }

    fn parse_nth(&mut self) -> Pseudo {
        let text = self.read_function_args();
        let compact: String = text.chars().filter(|ch| !ch.is_whitespace()).collect();
        let compact = compact.to_ascii_lowercase();
        match compact.as_str() {
            "odd" => Pseudo::Nth(2, 1),
            "even" => Pseudo::Nth(2, 0),
            "n" => Pseudo::Nth(1, 0),
            "-n" => Pseudo::Nth(-1, 0),
            _ => {
                if let Ok(b) = compact.parse::<i32>() {
                    return Pseudo::Nth(0, b);
                }
                parse_an_b(&compact)
                    .map(|(a, b)| Pseudo::Nth(a, b))
                    .unwrap_or(Pseudo::Unmatched)
            }
        }
    }

    fn parse_group(&mut self) -> Vec<Selector> {
        self.skip_ws();
        if self.peek() != Some('(') {
            return Vec::new();
        }
        self.bump();
        let mut selectors = Vec::new();
        loop {
            self.skip_ws();
            if matches!(self.peek(), Some(')') | None) {
                break;
            }
            let Some(selector) = self.parse_selector() else {
                break;
            };
            selectors.push(selector);
            self.skip_ws();
            if self.peek() == Some(',') {
                self.bump();
                continue;
            }
            break;
        }
        if self.peek() == Some(')') {
            self.bump();
        }
        selectors
    }

    fn parse_not(&mut self) -> Option<Compound> {
        self.skip_ws();
        if self.peek() != Some('(') {
            return None;
        }
        self.bump();
        self.skip_ws();
        let compound = self.parse_compound()?;
        self.skip_ws();
        if self.peek() == Some(')') {
            self.bump();
        }
        Some(compound)
    }

    fn skip_function_args(&mut self) {
        self.skip_ws();
        if self.peek() == Some('(') {
            let _ = self.read_function_args();
        }
    }

    fn read_function_args(&mut self) -> String {
        self.skip_ws();
        if self.peek() != Some('(') {
            return String::new();
        }
        self.bump();
        let mut depth = 1;
        let mut text = String::new();
        while let Some(ch) = self.bump() {
            match ch {
                '(' => {
                    depth += 1;
                    text.push(ch);
                }
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    text.push(ch);
                }
                _ => text.push(ch),
            }
        }
        text
    }

    fn read_string(&mut self) -> String {
        let quote = self.bump();
        let mut out = String::new();
        while let Some(ch) = self.bump() {
            if Some(ch) == quote {
                break;
            }
            if ch == '\\' {
                if let Some(escaped) = self.bump() {
                    out.push(escaped);
                }
            } else {
                out.push(ch);
            }
        }
        out
    }

    fn parse_declaration_block(&mut self) -> Vec<Declaration> {
        let mut declarations = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                None => break,
                Some('}') => {
                    self.bump();
                    break;
                }
                Some(';') => {
                    self.bump();
                }
                _ => {
                    if let Some(declaration) = self.parse_declaration() {
                        declarations.push(declaration);
                    } else {
                        self.skip_declaration();
                    }
                }
            }
        }
        declarations
    }

    fn parse_declaration_list(&mut self) -> Vec<Declaration> {
        let mut declarations = Vec::new();
        while self.pos < self.chars.len() {
            self.skip_ws();
            if self.pos >= self.chars.len() {
                break;
            }
            if self.peek() == Some(';') {
                self.bump();
                continue;
            }
            if let Some(declaration) = self.parse_declaration() {
                declarations.push(declaration);
            } else {
                self.skip_declaration();
            }
        }
        declarations
    }

    fn parse_declaration(&mut self) -> Option<Declaration> {
        let name = self.read_ident().to_ascii_lowercase();
        if name.is_empty() {
            return None;
        }
        self.skip_ws();
        if self.peek() != Some(':') {
            return None;
        }
        self.bump();
        let mut value = String::new();
        while let Some(ch) = self.peek() {
            if ch == ';' || ch == '}' {
                break;
            }
            value.push(ch);
            self.pos += 1;
        }
        if self.peek() == Some(';') {
            self.bump();
        }
        let (value, important) = split_important(&value);
        if value.is_empty() {
            return None;
        }
        Some(Declaration {
            name,
            value,
            important,
        })
    }

    fn take_at_rule(&mut self) -> AtRule {
        let start = self.pos;
        if self.peek() != Some('@') {
            return AtRule::Done;
        }
        self.bump();
        let name = self.read_ident().to_ascii_lowercase();
        if name == "media" {
            self.skip_ws();
            let mut prelude = String::new();
            while let Some(ch) = self.peek() {
                if ch == '{' {
                    break;
                }
                prelude.push(ch);
                self.bump();
            }
            if let Some(media) = media_from_prelude(&prelude) {
                return AtRule::Media(media);
            }
            self.skip_balanced_block();
            return AtRule::Done;
        }
        if name == "layer" || name == "supports" {
            while let Some(ch) = self.peek() {
                if ch == '{' || ch == ';' {
                    break;
                }
                self.bump();
            }
            if self.peek() == Some('{') {
                return AtRule::Enter;
            }
            if self.peek() == Some(';') {
                self.bump();
            }
            return AtRule::Done;
        }
        self.pos = start;
        self.skip_at_rule();
        AtRule::Done
    }

    fn parse_rule_list(&mut self, rules: &mut Vec<Rule>, media: Option<Media>) {
        self.skip_ws();
        if self.peek() == Some('{') {
            self.bump();
        }
        while self.pos < self.chars.len() {
            self.skip_ws();
            if self.peek().is_none() || self.peek() == Some('}') {
                self.bump();
                break;
            }
            if self.peek() == Some('@') {
                match self.take_at_rule() {
                    AtRule::Media(inner) => {
                        self.parse_rule_list(rules, Some(combine_media(media.clone(), inner)));
                    }
                    AtRule::Enter => self.parse_rule_list(rules, media.clone()),
                    AtRule::Done => {}
                }
                continue;
            }
            let selectors = self.parse_selector_list();
            self.skip_ws();
            if self.peek() != Some('{') {
                self.skip_until_recovery();
                continue;
            }
            self.bump();
            let declarations = self.parse_declaration_block();
            if !selectors.is_empty() && !declarations.is_empty() {
                rules.push(Rule {
                    selectors,
                    declarations,
                    media: media.clone(),
                });
            }
        }
    }

    fn skip_balanced_block(&mut self) {
        self.skip_ws();
        if self.peek() != Some('{') {
            return;
        }
        self.bump();
        let mut depth = 1;
        while let Some(ch) = self.bump() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
    }

    fn skip_at_rule(&mut self) {
        self.bump();
        let mut depth = 0;
        while let Some(ch) = self.bump() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                ';' if depth == 0 => break,
                _ => {}
            }
        }
    }

    fn skip_until_recovery(&mut self) {
        while let Some(ch) = self.bump() {
            if ch == '}' || ch == '{' {
                break;
            }
        }
    }

    fn skip_declaration(&mut self) {
        while let Some(ch) = self.bump() {
            if ch == ';' || ch == '}' {
                break;
            }
        }
    }

    fn read_ident(&mut self) -> String {
        let mut out = String::new();
        loop {
            if self.peek() == Some('\\') {
                match self.read_escape() {
                    Escape::Char(ch) => out.push(ch),
                    Escape::Ignore => {}
                    Escape::End => break,
                }
                continue;
            }
            if self.peek().is_some_and(is_ident_char) {
                out.push(self.bump().unwrap());
                continue;
            }
            break;
        }
        out
    }

    fn read_escape(&mut self) -> Escape {
        if self.peek() != Some('\\') {
            return Escape::End;
        }
        self.bump();
        let Some(ch) = self.peek() else {
            return Escape::End;
        };
        if ch == '\n' {
            self.bump();
            return Escape::Ignore;
        }
        if ch == '\r' {
            self.bump();
            if self.peek() == Some('\n') {
                self.bump();
            }
            return Escape::Ignore;
        }
        if ch.is_ascii_hexdigit() {
            let mut hex = String::new();
            while hex.len() < 6 && self.peek().is_some_and(|digit| digit.is_ascii_hexdigit()) {
                hex.push(self.bump().unwrap());
            }
            if self
                .peek()
                .is_some_and(|digit| digit == ' ' || digit == '\t' || digit == '\n')
            {
                self.bump();
            }
            let code = u32::from_str_radix(&hex, 16).unwrap_or(0);
            return char::from_u32(code)
                .filter(|decoded| *decoded != '\0')
                .map(Escape::Char)
                .unwrap_or(Escape::Ignore);
        }
        Escape::Char(self.bump().unwrap())
    }

    fn skip_ws(&mut self) -> bool {
        self.skip_ws_counting()
    }

    fn skip_ws_counting(&mut self) -> bool {
        let start = self.pos;
        while self.peek().is_some_and(|ch| ch.is_whitespace()) {
            self.pos += 1;
        }
        self.pos != start
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += 1;
        Some(ch)
    }
}

fn parse_an_b(text: &str) -> Option<(i32, i32)> {
    let (head, tail) = if let Some(index) = text.find('n') {
        (&text[..index], &text[index + 1..])
    } else {
        return None;
    };
    let a = if head.is_empty() || head == "+" {
        1
    } else if head == "-" {
        -1
    } else {
        head.parse().ok()?
    };
    let b = if tail.is_empty() {
        0
    } else {
        tail.parse().ok()?
    };
    Some((a, b))
}

enum AtRule {
    Media(Media),
    /// `@layer` or `@supports` whose block still needs to be parsed.
    Enter,
    Done,
}

enum Escape {
    Char(char),
    Ignore,
    End,
}

fn combine_media(outer: Option<Media>, inner: Media) -> Media {
    let Some(outer) = outer else {
        return inner;
    };
    let mut alternatives = Vec::new();
    for left in &outer.alternatives {
        for right in &inner.alternatives {
            alternatives.push(MediaBound {
                min_width: match (left.min_width, right.min_width) {
                    (Some(left), Some(right)) => Some(left.max(right)),
                    (left, right) => left.or(right),
                },
                max_width: match (left.max_width, right.max_width) {
                    (Some(left), Some(right)) => Some(left.min(right)),
                    (left, right) => left.or(right),
                },
            });
        }
    }
    Media { alternatives }
}

fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_' || ch == '-'
}

fn is_ident_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
}

fn split_important(value: &str) -> (String, bool) {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    if let Some(index) = lower.rfind("!important") {
        let after = trimmed[index + "!important".len()..].trim();
        if after.is_empty() {
            return (trimmed[..index].trim().to_string(), true);
        }
    }
    (trimmed.to_string(), false)
}

fn strip_comments(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(chars.len());
            out.push(' ');
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn media_from_prelude(prelude: &str) -> Option<Media> {
    let mut alternatives = Vec::new();
    for part in split_media_list(prelude) {
        if let Some(bound) = media_bound(&part) {
            alternatives.push(bound);
        }
    }
    if alternatives.is_empty() {
        None
    } else {
        Some(Media { alternatives })
    }
}

fn split_media_list(prelude: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0;
    for ch in prelude.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                parts.push(std::mem::take(&mut current));
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        parts.push(current);
    }
    parts
}

fn media_bound(part: &str) -> Option<MediaBound> {
    let lower = part.to_ascii_lowercase();
    if lower
        .split_whitespace()
        .any(|word| word == "not" || word == "print")
    {
        return None;
    }
    let mut min_width = None;
    let mut max_width = None;
    let mut rest = lower.as_str();
    while let Some(start) = rest.find('(') {
        let after = &rest[start + 1..];
        let end = after.find(')')?;
        let feature = after[..end].trim();
        if let Some(value) = feature.strip_prefix("min-width:") {
            min_width = Some(media_px(value.trim())?);
        } else if let Some(value) = feature.strip_prefix("max-width:") {
            max_width = Some(media_px(value.trim())?);
        } else {
            return None;
        }
        rest = &after[end + 1..];
    }
    for word in rest.split_whitespace() {
        if word != "all" && word != "screen" && word != "only" && word != "and" {
            return None;
        }
    }
    Some(MediaBound {
        min_width,
        max_width,
    })
}

fn media_px(value: &str) -> Option<f32> {
    let value = value.trim();
    if let Some(number) = value.strip_suffix("px") {
        return number.trim().parse().ok();
    }
    if let Some(number) = value
        .strip_suffix("rem")
        .or_else(|| value.strip_suffix("em"))
    {
        return number.trim().parse::<f32>().ok().map(|em| em * 16.0);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_grouped_selectors_and_important() {
        let sheet = parse_css("/* x */ div > p.a, #x { color: #abc !important; margin: 1px 2px; }");
        assert_eq!(sheet.rules.len(), 1);
        assert_eq!(sheet.rules[0].selectors.len(), 2);
        assert_eq!(
            sheet.rules[0].selectors[0].combinators,
            vec![Combinator::Child]
        );
        assert_eq!(sheet.rules[0].selectors[1].specificity(), 100);
        assert!(sheet.rules[0].declarations[0].important);
        assert_eq!(sheet.rules[0].declarations[0].value, "#abc");
        assert_eq!(sheet.rules[0].declarations[1].name, "margin");
    }

    #[test]
    fn is_takes_the_argument_specificity_and_where_adds_none() {
        let sheet = parse_css("div:is(#a, .b) { color: red } span:where(#a, .b) { color: blue }");
        assert!(matches!(
            sheet.rules[0].selectors[0].compounds[0].pseudos[0],
            Pseudo::Is(_)
        ));
        assert_eq!(sheet.rules[0].selectors[0].specificity(), 101);
        assert_eq!(sheet.rules[1].selectors[0].specificity(), 1);
    }

    #[test]
    fn parses_attribute_sibling_and_nth_selectors() {
        let sheet = parse_css(r#"a[href] + p[class~="note"]:nth-child(2n+1) { color: teal }"#);
        let selector = &sheet.rules[0].selectors[0];
        assert_eq!(selector.combinators, vec![Combinator::NextSibling]);
        assert_eq!(selector.compounds[0].attrs[0].name, "href");
        assert_eq!(selector.compounds[1].attrs[0].op, Some(AttrOp::Includes));
        assert_eq!(selector.compounds[1].pseudos, vec![Pseudo::Nth(2, 1)]);
        assert!(selector.specificity() >= 30);
    }

    #[test]
    fn layer_and_supports_keep_the_rules_inside() {
        let sheet = parse_css(
            r#"
            @layer a, b;
            .keep { color: red }
            @layer utilities {
              .sm\:flex { display: flex }
              .bg-background\/80 { color: blue }
            }
            @supports (display: grid) {
              .box { display: grid }
            }
            @media (hover: hover) { .skip { color: red } }
            @media (min-width: 40rem) { .lg\:flex { display: flex } }
            "#,
        );
        let classes = |name: &str| {
            sheet.rules.iter().any(|rule| {
                rule.selectors.iter().any(|selector| {
                    selector
                        .compounds
                        .iter()
                        .any(|compound| compound.classes.iter().any(|class| class == name))
                })
            })
        };
        assert!(classes("keep"));
        assert!(classes("sm:flex"));
        assert!(classes("bg-background/80"));
        assert!(classes("box"));
        assert!(classes("lg:flex"));
        assert!(!classes("skip"));
        let responsive = sheet.rules.iter().find(|rule| {
            rule.selectors.iter().any(|selector| {
                selector
                    .compounds
                    .iter()
                    .any(|compound| compound.classes.iter().any(|class| class == "lg:flex"))
            })
        });
        assert_eq!(
            responsive
                .and_then(|rule| rule.media.as_ref())
                .map(|media| { media.alternatives[0].min_width }),
            Some(Some(640.0))
        );
    }
}
