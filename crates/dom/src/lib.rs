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
    pub scripts: Vec<String>,
    pub base_href: Option<String>,
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
pub struct ElementData {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub id: Option<String>,
    pub classes: Vec<String>,
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
        }
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
                .map(|child| self.text_content(*child))
                .collect(),
            NodeData::Comment(_) => String::new(),
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
            && let NodeData::Text(existing) = &mut self.nodes[last.0].data
        {
            existing.push_str(text);
            return Some(last);
        }
        let id = NodeId(self.nodes.len());
        self.nodes.push(Node {
            parent: Some(parent),
            children: Vec::new(),
            data: NodeData::Text(text.to_string()),
        });
        self.nodes[parent.0].children.push(id);
        Some(id)
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
        if name == "id" && element.id.is_none() {
            element.id = Some(value.to_string());
        }
        if name == "class" {
            element.classes = value.split_whitespace().map(str::to_string).collect();
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
