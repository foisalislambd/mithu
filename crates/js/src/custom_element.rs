// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Adapted from Servo (MPL-2.0), components/script/dom/node/customelementregistry.rs
// (`upgrade_element` and `invoke_callback`). The reaction queue there runs
// after the constructor returns. Attribute changes are delivered only for
// names in `observedAttributes`.

//! Custom element reactions, in the order Servo enqueues them.

/// One lifecycle reaction queued by an upgrade or a later attribute write.
#[derive(Debug, PartialEq, Eq)]
pub enum Reaction {
    /// `attributeChangedCallback(name, oldValue, newValue, namespace)`.
    AttributeChanged {
        name: String,
        old: Option<String>,
        new: Option<String>,
    },
    /// `connectedCallback()`.
    Connected,
}

/// Reactions for an upgrade.
///
/// Servo enqueues an `attributeChanged` reaction for every attribute, then a
/// `connected` reaction when the element is in the document, then runs the
/// constructor. The queue is invoked after the constructor returns. Names
/// that are not observed are dropped, which is what `invoke_callback` does
/// before calling the user function.
pub fn upgrade_reactions(
    connected: bool,
    attributes: &[(String, String)],
    observed: &[String],
) -> Vec<Reaction> {
    let mut reactions = Vec::new();
    for (name, value) in attributes {
        if observes(observed, name) {
            reactions.push(Reaction::AttributeChanged {
                name: name.clone(),
                old: None,
                new: Some(value.clone()),
            });
        }
    }
    if connected {
        reactions.push(Reaction::Connected);
    }
    reactions
}

/// A `setAttribute` after upgrade. `old` is `None` when the attribute was absent.
pub fn attribute_reaction(
    name: &str,
    old: Option<String>,
    new: String,
    observed: &[String],
) -> Option<Reaction> {
    if !observes(observed, name) {
        return None;
    }
    Some(Reaction::AttributeChanged {
        name: name.to_string(),
        old,
        new: Some(new),
    })
}

fn observes(observed: &[String], name: &str) -> bool {
    observed
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrade_delivers_observed_attributes_before_connected() {
        let reactions = upgrade_reactions(
            true,
            &[("hidden".into(), "".into()), ("title".into(), "Clip".into())],
            &["title".into()],
        );
        assert_eq!(
            reactions,
            vec![
                Reaction::AttributeChanged {
                    name: "title".into(),
                    old: None,
                    new: Some("Clip".into()),
                },
                Reaction::Connected,
            ]
        );
    }

    #[test]
    fn a_later_write_reports_the_previous_value() {
        let reaction = attribute_reaction("title", Some("a".into()), "b".into(), &["title".into()]);
        assert_eq!(
            reaction,
            Some(Reaction::AttributeChanged {
                name: "title".into(),
                old: Some("a".into()),
                new: Some("b".into()),
            })
        );
        assert!(attribute_reaction("id", None, "x".into(), &["title".into()]).is_none());
    }
}
