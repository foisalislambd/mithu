//! A flat, index-addressed DOM.
//!
//! Nodes live in one arena. Parents store child ids, so the HTML parser can
//! build the tree without reference cycles.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId(pub usize);

#[derive(Clone, Debug)]
pub struct Document {
    pub nodes: Vec<Node>,
    pub root: NodeId,
    pub title: String,
    pub stylesheets: Vec<String>,
    pub stylesheet_links: Vec<String>,
    pub scripts: Vec<Script>,
    pub base_href: Option<String>,
    /// Inline `<script type="importmap">` bodies, in document order.
    pub import_maps: Vec<String>,
    /// Child and attribute changes since the last observer delivery.
    pub mutations: Vec<DomMutation>,
    /// `<template>` elements keep their contents in a detached fragment, the
    /// way the HTML parser does, so the template itself has no child nodes.
    pub template_contents: Vec<(NodeId, NodeId)>,
    /// Bumps on a tree or attribute change so layout can skip an unchanged page.
    pub revision: u64,
}

/// One DOM change a `MutationObserver` can report.
#[derive(Clone, Debug)]
pub struct DomMutation {
    pub target: NodeId,
    pub kind: String,
    pub attribute: Option<String>,
    pub added: Vec<NodeId>,
    pub removed: Vec<NodeId>,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub data: NodeData,
}

#[derive(Clone, Debug)]
pub enum NodeData {
    Document,
    Element(ElementData),
    Text(String),
    Comment(String),
}

#[derive(Clone, Debug)]
pub struct Script {
    pub node: NodeId,
    pub src: Option<String>,
    pub code: String,
    pub module: bool,
    pub defer: bool,
    pub asynchronous: bool,
    pub nomodule: bool,
    /// `type` is empty, JavaScript, or `module`. JSON-LD and other data blocks are not.
    pub javascript: bool,
}

#[derive(Clone, Debug)]
pub struct ElementData {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub id: Option<String>,
    pub classes: Vec<String>,
}

fn attributes_equal(left: &[(String, String)], right: &[(String, String)]) -> bool {
    left.len() == right.len()
        && left.iter().all(|(name, value)| {
            right
                .iter()
                .any(|(other, other_value)| name == other && value == other_value)
        })
}

impl Document {
    pub fn new() -> Self {
        Self {
            nodes: vec![Node {
                parent: None,
                children: Vec::new(),
                data: NodeData::Document,
            }],
            root: NodeId(0),
            title: String::new(),
            stylesheets: Vec::new(),
            stylesheet_links: Vec::new(),
            scripts: Vec::new(),
            base_href: None,
            import_maps: Vec::new(),
            mutations: Vec::new(),
            template_contents: Vec::new(),
            revision: 0,
        }
    }

    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn template_content(&self, template: NodeId) -> Option<NodeId> {
        self.template_contents
            .iter()
            .find(|(id, _)| *id == template)
            .map(|(_, fragment)| *fragment)
    }

    /// True when `id` is in the document tree. Template contents and other
    /// detached nodes are not.
    pub fn is_connected(&self, id: NodeId) -> bool {
        let mut current = Some(id);
        while let Some(node) = current {
            if node == self.root {
                return true;
            }
            current = self.nodes.get(node.0).and_then(|item| item.parent);
        }
        false
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.0]
    }

    pub fn element_name(&self, id: NodeId) -> Option<&str> {
        match &self.nodes[id.0].data {
            NodeData::Element(element) => Some(element.name.as_str()),
            _ => None,
        }
    }

    pub fn element(&self, id: NodeId) -> Option<&ElementData> {
        match &self.nodes[id.0].data {
            NodeData::Element(element) => Some(element),
            _ => None,
        }
    }

    pub fn attr(&self, id: NodeId, name: &str) -> Option<&str> {
        let element = self.element(id)?;
        element
            .attrs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    pub fn has_class(&self, id: NodeId, class: &str) -> bool {
        self.element(id)
            .is_some_and(|element| element.classes.iter().any(|item| item == class))
    }

    pub fn parent_element(&self, id: NodeId) -> Option<NodeId> {
        let mut current = self.nodes[id.0].parent;
        while let Some(parent) = current {
            if self.element_name(parent).is_some() {
                return Some(parent);
            }
            current = self.nodes[parent.0].parent;
        }
        None
    }

    pub fn get_element_by_id(&self, id: &str) -> Option<NodeId> {
        self.nodes.iter().enumerate().find_map(|(index, node)| {
            let NodeData::Element(element) = &node.data else {
                return None;
            };
            (element.id.as_deref() == Some(id)).then_some(NodeId(index))
        })
    }

    pub fn text_content(&self, id: NodeId) -> String {
        match &self.nodes[id.0].data {
            NodeData::Text(text) => text.clone(),
            NodeData::Element(_) | NodeData::Document => self.nodes[id.0]
                .children
                .iter()
                .filter(|child| !matches!(self.nodes[child.0].data, NodeData::Comment(_)))
                .map(|child| self.text_content(*child))
                .collect(),
            NodeData::Comment(text) => text.clone(),
        }
    }

    pub fn create_element(&mut self, parent: NodeId, name: &str) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(Node {
            parent: Some(parent),
            children: Vec::new(),
            data: NodeData::Element(ElementData {
                name: name.to_ascii_lowercase(),
                attrs: Vec::new(),
                id: None,
                classes: Vec::new(),
            }),
        });
        self.nodes[parent.0].children.push(id);
        id
    }

    pub fn append_text(&mut self, parent: NodeId, text: &str) -> Option<NodeId> {
        if text.is_empty() {
            return None;
        }
        if let Some(last) = self.nodes[parent.0].children.last().copied()
            && matches!(self.nodes[last.0].data, NodeData::Text(_))
        {
            if let NodeData::Text(existing) = &mut self.nodes[last.0].data {
                existing.push_str(text);
            }
            self.touch();
            return Some(last);
        }
        let id = NodeId(self.nodes.len());
        self.nodes.push(Node {
            parent: Some(parent),
            children: Vec::new(),
            data: NodeData::Text(text.to_string()),
        });
        self.nodes[parent.0].children.push(id);
        self.note_children(parent, vec![id], Vec::new());
        Some(id)
    }

    fn note_children(&mut self, target: NodeId, added: Vec<NodeId>, removed: Vec<NodeId>) {
        if added.is_empty() && removed.is_empty() {
            return;
        }
        self.touch();
        self.mutations.push(DomMutation {
            target,
            kind: "childList".to_string(),
            attribute: None,
            added,
            removed,
        });
    }

    pub fn set_attr(&mut self, id: NodeId, name: &str, value: &str) {
        let NodeData::Element(element) = &mut self.nodes[id.0].data else {
            return;
        };
        let name = name.to_ascii_lowercase();
        if let Some((_, existing)) = element.attrs.iter_mut().find(|(key, _)| *key == name) {
            *existing = value.to_string();
        } else {
            element.attrs.push((name.clone(), value.to_string()));
        }
        if name == "id" {
            element.id = if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            };
        }
        if name == "class" {
            element.classes = value.split_whitespace().map(str::to_string).collect();
        }
        self.touch();
    }

    pub fn remove_attr(&mut self, id: NodeId, name: &str) {
        let NodeData::Element(element) = &mut self.nodes[id.0].data else {
            return;
        };
        let name = name.to_ascii_lowercase();
        element.attrs.retain(|(key, _)| key != &name);
        if name == "id" {
            element.id = None;
        }
        if name == "class" {
            element.classes.clear();
        }
        self.touch();
    }

    pub fn append_comment(&mut self, parent: NodeId, text: &str) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(Node {
            parent: Some(parent),
            children: Vec::new(),
            data: NodeData::Comment(text.to_string()),
        });
        if let Some(parent) = self.nodes.get_mut(parent.0) {
            parent.children.push(id);
        }
        id
    }

    pub fn create_text_detached(&mut self, text: &str) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(Node {
            parent: None,
            children: Vec::new(),
            data: NodeData::Text(text.to_string()),
        });
        id
    }

    pub fn create_comment_detached(&mut self, text: &str) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(Node {
            parent: None,
            children: Vec::new(),
            data: NodeData::Comment(text.to_string()),
        });
        id
    }

    pub fn create_element_detached(&mut self, name: &str) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(Node {
            parent: None,
            children: Vec::new(),
            data: NodeData::Element(ElementData {
                name: name.to_ascii_lowercase(),
                attrs: Vec::new(),
                id: None,
                classes: Vec::new(),
            }),
        });
        id
    }

    pub fn clear_children(&mut self, id: NodeId) {
        if self.nodes.get(id.0).is_none() {
            return;
        }
        let children = std::mem::take(&mut self.nodes[id.0].children);
        for child in &children {
            if let Some(node) = self.nodes.get_mut(child.0) {
                node.parent = None;
            }
        }
        self.note_children(id, Vec::new(), children);
    }

    pub fn set_text_content(&mut self, id: NodeId, text: &str) {
        if self.nodes.get(id.0).is_none() {
            return;
        }
        if matches!(
            self.nodes[id.0].data,
            NodeData::Text(_) | NodeData::Comment(_)
        ) {
            if let NodeData::Text(existing) | NodeData::Comment(existing) =
                &mut self.nodes[id.0].data
            {
                *existing = text.to_string();
            }
            self.touch();
            return;
        }
        self.clear_children(id);
        if !text.is_empty() {
            self.append_text(id, text);
        }
    }

    pub fn detach(&mut self, id: NodeId) {
        let Some(node) = self.nodes.get(id.0) else {
            return;
        };
        let parent = node.parent;
        if let Some(parent) = parent
            && let Some(parent_node) = self.nodes.get_mut(parent.0)
        {
            parent_node.children.retain(|child| *child != id);
        }
        if let Some(node) = self.nodes.get_mut(id.0) {
            node.parent = None;
        }
        if let Some(parent) = parent {
            self.note_children(parent, Vec::new(), vec![id]);
        }
    }

    /// Blink's `Node.isEqualNode`: same type, and for elements the same tag,
    /// the same attributes regardless of order, and equal children in order.
    pub fn nodes_equal(&self, left: NodeId, right: NodeId) -> bool {
        if left == right {
            return true;
        }
        let (Some(a), Some(b)) = (self.nodes.get(left.0), self.nodes.get(right.0)) else {
            return false;
        };
        match (&a.data, &b.data) {
            (NodeData::Text(left), NodeData::Text(right))
            | (NodeData::Comment(left), NodeData::Comment(right)) => left == right,
            (NodeData::Document, NodeData::Document) => self.children_equal(left, right),
            (NodeData::Element(left_el), NodeData::Element(right_el)) => {
                left_el.name == right_el.name
                    && attributes_equal(&left_el.attrs, &right_el.attrs)
                    && self.children_equal(left, right)
            }
            _ => false,
        }
    }

    fn children_equal(&self, left: NodeId, right: NodeId) -> bool {
        let left_children = &self.nodes[left.0].children;
        let right_children = &self.nodes[right.0].children;
        left_children.len() == right_children.len()
            && left_children
                .iter()
                .zip(right_children)
                .all(|(left, right)| self.nodes_equal(*left, *right))
    }

    /// A detached copy. `deep` copies descendants. Template contents are copied
    /// with the template, since they are not child nodes.
    pub fn clone_node(&mut self, id: NodeId, deep: bool) -> NodeId {
        let Some(source) = self.nodes.get(id.0) else {
            return id;
        };
        let data = source.data.clone();
        let children = if deep {
            source.children.clone()
        } else {
            Vec::new()
        };
        let fragment = self.template_content(id);
        let copy = NodeId(self.nodes.len());
        self.nodes.push(Node {
            parent: None,
            children: Vec::new(),
            data,
        });
        if deep {
            for child in children {
                let cloned = self.clone_node(child, true);
                self.nodes[cloned.0].parent = Some(copy);
                self.nodes[copy.0].children.push(cloned);
            }
        }
        if let Some(fragment) = fragment {
            let cloned = self.clone_node(fragment, deep);
            self.template_contents.push((copy, cloned));
        }
        copy
    }

    pub fn contains_node(&self, ancestor: NodeId, node: NodeId) -> bool {
        let mut current = Some(node);
        while let Some(id) = current {
            if id == ancestor {
                return true;
            }
            current = self.nodes.get(id.0).and_then(|item| item.parent);
        }
        false
    }

    pub fn append_child(&mut self, parent: NodeId, child: NodeId) -> bool {
        if self.nodes.get(parent.0).is_none() || self.nodes.get(child.0).is_none() {
            return false;
        }
        if parent == child || self.contains_node(child, parent) {
            return false;
        }
        self.detach(child);
        self.nodes[child.0].parent = Some(parent);
        self.nodes[parent.0].children.push(child);
        self.note_children(parent, vec![child], Vec::new());
        true
    }

    pub fn insert_before(&mut self, parent: NodeId, child: NodeId, before: Option<NodeId>) -> bool {
        if self.nodes.get(parent.0).is_none() || self.nodes.get(child.0).is_none() {
            return false;
        }
        if parent == child || self.contains_node(child, parent) {
            return false;
        }
        // Inserting a node before itself means "leave it in front of the next sibling".
        let before = if before == Some(child) {
            self.next_sibling(child)
        } else {
            before
        };
        self.detach(child);
        self.nodes[child.0].parent = Some(parent);
        if let Some(before) = before
            && let Some(index) = self.nodes[parent.0]
                .children
                .iter()
                .position(|id| *id == before)
        {
            self.nodes[parent.0].children.insert(index, child);
            self.note_children(parent, vec![child], Vec::new());
            return true;
        }
        self.nodes[parent.0].children.push(child);
        self.note_children(parent, vec![child], Vec::new());
        true
    }

    pub fn insert_text_before(&mut self, parent: NodeId, before: NodeId, text: &str) {
        if text.is_empty() || self.nodes.get(parent.0).is_none() {
            return;
        }
        let Some(index) = self.nodes[parent.0]
            .children
            .iter()
            .position(|id| *id == before)
        else {
            self.append_text(parent, text);
            return;
        };
        if index > 0 {
            let previous = self.nodes[parent.0].children[index - 1];
            if let NodeData::Text(existing) = &mut self.nodes[previous.0].data {
                existing.push_str(text);
                return;
            }
        }
        let id = NodeId(self.nodes.len());
        self.nodes.push(Node {
            parent: Some(parent),
            children: Vec::new(),
            data: NodeData::Text(text.to_string()),
        });
        self.nodes[parent.0].children.insert(index, id);
    }

    fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
        let parent = self.nodes.get(id.0)?.parent?;
        let children = &self.nodes.get(parent.0)?.children;
        let index = children.iter().position(|child| *child == id)?;
        children.get(index + 1).copied()
    }

    pub fn element_child_index(&self, id: NodeId) -> Option<usize> {
        let parent = self.nodes.get(id.0)?.parent?;
        let mut count = 0;
        for child in &self.nodes.get(parent.0)?.children {
            if self.element_name(*child).is_some() {
                count += 1;
                if *child == id {
                    return Some(count);
                }
            }
        }
        None
    }

    pub fn previous_element_sibling(&self, id: NodeId) -> Option<NodeId> {
        let parent = self.nodes.get(id.0)?.parent?;
        let children = &self.nodes.get(parent.0)?.children;
        let index = children.iter().position(|child| *child == id)?;
        children[..index]
            .iter()
            .rev()
            .copied()
            .find(|child| self.element_name(*child).is_some())
    }

    pub fn next_element_sibling(&self, id: NodeId) -> Option<NodeId> {
        let parent = self.nodes.get(id.0)?.parent?;
        let children = &self.nodes.get(parent.0)?.children;
        let index = children.iter().position(|child| *child == id)?;
        children[index + 1..]
            .iter()
            .copied()
            .find(|child| self.element_name(*child).is_some())
    }

    pub fn shadow_root(&self, id: NodeId) -> Option<NodeId> {
        self.attr(id, "mithu-shadow")
            .and_then(|raw| raw.parse::<usize>().ok())
            .filter(|index| *index < self.nodes.len())
            .map(NodeId)
    }

    /// Children that participate in layout. A shadow host contributes its
    /// shadow tree. A `<slot>` contributes its assigned light-DOM nodes, or
    /// its fallback children when nothing is assigned.
    pub fn rendered_children(&self, id: NodeId) -> Vec<NodeId> {
        let host = if self.shadow_root(id).is_some() {
            Some(id)
        } else {
            self.shadow_host_for_node(id)
        };
        let source = self.shadow_root(id).unwrap_or(id);
        let mut out = Vec::new();
        for child in self
            .nodes
            .get(source.0)
            .map(|node| node.children.clone())
            .unwrap_or_default()
        {
            out.extend(self.expand_slot(child, host));
        }
        out
    }

    fn expand_slot(&self, id: NodeId, host: Option<NodeId>) -> Vec<NodeId> {
        if self.element_name(id) != Some("slot") {
            return vec![id];
        }
        let Some(host) = host else {
            return self
                .nodes
                .get(id.0)
                .map(|node| node.children.clone())
                .unwrap_or_default();
        };
        let name = self.attr(id, "name").unwrap_or("");
        let assigned = self
            .nodes
            .get(host.0)
            .map(|node| node.children.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|child| self.assigned_to_slot(*child, name))
            .collect::<Vec<_>>();
        if assigned.is_empty() {
            return self
                .nodes
                .get(id.0)
                .map(|node| node.children.clone())
                .unwrap_or_default();
        }
        assigned
    }

    fn assigned_to_slot(&self, id: NodeId, slot_name: &str) -> bool {
        if self.element_name(id) == Some("template") {
            return false;
        }
        let name = self.attr(id, "slot").unwrap_or("");
        if slot_name.is_empty() {
            name.is_empty()
        } else {
            name == slot_name
        }
    }

    fn shadow_host_for_node(&self, id: NodeId) -> Option<NodeId> {
        let mut current = Some(id);
        while let Some(node) = current {
            if let Some(raw) = self.attr(node, "mithu-host")
                && let Ok(index) = raw.parse::<usize>()
                && index < self.nodes.len()
            {
                return Some(NodeId(index));
            }
            current = self.nodes.get(node.0).and_then(|item| item.parent);
        }
        None
    }

    pub fn refresh_metadata(&mut self) {
        self.stylesheets.clear();
        self.stylesheet_links.clear();
        let mut title = None;
        let ids: Vec<NodeId> = (0..self.nodes.len()).map(NodeId).collect();
        for id in ids {
            match self.element_name(id) {
                Some("title") if title.is_none() => {
                    title = Some(
                        self.text_content(id)
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" "),
                    );
                }
                Some("style") => {
                    let mut text = self.text_content(id);
                    if let Some(host) = self.shadow_host_for_node(id) {
                        let name = self.element_name(host).unwrap_or("*");
                        text = scope_shadow_css(&text, name);
                    }
                    if !text.trim().is_empty() {
                        self.stylesheets.push(text);
                    }
                }
                Some("link") => {
                    let rel = self.attr(id, "rel").unwrap_or("").to_ascii_lowercase();
                    let stylesheet = rel.split_whitespace().any(|part| part == "stylesheet");
                    if stylesheet && let Some(href) = self.attr(id, "href") {
                        self.stylesheet_links.push(href.to_string());
                    }
                }
                _ => {}
            }
        }
        if let Some(title) = title {
            self.title = title;
        }
    }

    fn fmt_node(&self, id: NodeId, depth: usize, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let indent = "  ".repeat(depth);
        match &self.nodes[id.0].data {
            NodeData::Document => {
                writeln!(f, "{indent}#document")?;
            }
            NodeData::Element(element) => {
                writeln!(f, "{indent}<{}>", element.name)?;
            }
            NodeData::Text(text) => {
                writeln!(f, "{indent}\"{text}\"")?;
            }
            NodeData::Comment(text) => {
                writeln!(f, "{indent}<!--{text}-->")?;
            }
        }
        for child in &self.nodes[id.0].children {
            self.fmt_node(*child, depth + 1, f)?;
        }
        Ok(())
    }
}

/// `:host` and `::slotted()` only match inside a shadow tree. The flattened
/// tree is one document, so those selectors become the host tag and the
/// slotted selector.
pub fn scope_shadow_css(css: &str, host: &str) -> String {
    let mut rest = css;
    let mut out = String::new();
    while let Some(index) = next_shadow_selector(rest) {
        out.push_str(&rest[..index]);
        let tail = &rest[index..];
        if let Some(inner) = tail.strip_prefix("::slotted(") {
            let Some((body, after)) = split_group(inner) else {
                out.push_str("::slotted(");
                rest = inner;
                continue;
            };
            out.push_str(body);
            rest = after;
        } else if let Some(inner) = tail.strip_prefix(":host(") {
            out.push_str(host);
            out.push_str(":is(");
            rest = inner;
        } else if let Some(inner) = tail.strip_prefix(":host") {
            out.push_str(host);
            rest = inner;
        } else {
            out.push_str(&tail[..1]);
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

fn next_shadow_selector(css: &str) -> Option<usize> {
    let host = css.find(":host");
    let slotted = css.find("::slotted(");
    match (host, slotted) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(index), None) | (None, Some(index)) => Some(index),
        (None, None) => None,
    }
}

fn split_group(input: &str) -> Option<(&str, &str)> {
    let mut depth = 1i32;
    for (index, ch) in input.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&input[..index], &input[index + 1..]));
                }
            }
            _ => {}
        }
    }
    None
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for Document {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_node(self.root, 0, f)
    }
}
