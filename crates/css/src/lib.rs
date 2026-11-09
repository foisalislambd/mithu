//! CSS parser for the subset Mithu's style engine understands.
//!
//! Selectors support type, class, id, the descendant combinator, and the child
//! combinator. At-rules are skipped. Declarations stay as strings so the style
//! crate can interpret them.

#[derive(Clone, Debug, PartialEq)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub selectors: Vec<Selector>,
    pub declarations: Vec<Declaration>,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Combinator {
    Descendant,
    Child,
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
            if compound.id.is_some() {
                ids += 1;
            }
            classes += compound.classes.len() as u32;
            if compound.tag.is_some() {
                types += 1;
            }
        }
        ids * 100 + classes * 10 + types
    }
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
        while self.pos < self.chars.len() {
            self.skip_ws();
            if self.pos >= self.chars.len() {
                break;
            }
            if self.peek() == Some('@') {
                self.skip_at_rule();
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
                });
            }
        }
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
                Some('{') | Some(',') | Some('}') | None => {
                    self.pos = if saw_ws { before } else { self.pos };
                    if saw_ws && matches!(self.peek(), Some('{') | Some(',') | Some('}')) {
                        // Whitespace before the block is not a combinator.
                    }
                    break;
                }
                Some('>') => {
                    self.bump();
                    self.skip_ws();
                    let Some(next) = self.parse_compound() else {
                        break;
                    };
                    combinators.push(Combinator::Child);
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
                _ => break,
            }
        }
        any.then_some(compound)
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
        while self.peek().is_some_and(is_ident_char) {
            out.push(self.bump().unwrap());
        }
        out
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
}
