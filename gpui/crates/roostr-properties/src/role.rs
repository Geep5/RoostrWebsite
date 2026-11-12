//! A property's role: the Properties pane section it shows under (Who, When,
//! State, Setup, Details). A relation's `role` field names it; a relation
//! without one takes the built-in place for its key.

use roostr_core::types::RelationDef;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Who,
    When,
    State,
    Setup,
    Details,
}

/// Every role, in the pane's section order.
pub const ROLES: [Role; 5] = [
    Role::Who,
    Role::When,
    Role::State,
    Role::Setup,
    Role::Details,
];

const WHO: &[&str] = &["agent", "assignee", "served_by", "repo_path"];
const WHEN: &[&str] = &["repeat", "repeat_instructions", "check_first", "due_date"];
const STATE: &[&str] = &["done", "status"];
const SETUP: &[&str] = &[
    "model",
    "prompt",
    "skills",
    "tools",
    "credentials",
    "chat_color",
    "capability",
    "tool_inputs",
    "tool_builtin",
    "tool_version",
    "jev_answer",
    "jev_writes",
];

/// Types whose Setup section shows; elsewhere Setup rows sit under Details.
pub const SETUP_TYPES: &[&str] = &["agent", "tool", "skill", "credential", "capability"];

impl Role {
    /// The value a relation's `role` field holds.
    pub fn key(self) -> &'static str {
        match self {
            Role::Who => "who",
            Role::When => "when",
            Role::State => "state",
            Role::Setup => "setup",
            Role::Details => "details",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Role::Who => "Who",
            Role::When => "When",
            Role::State => "State",
            Role::Setup => "Setup",
            Role::Details => "Details",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        ROLES.into_iter().find(|r| r.key() == s)
    }
}

/// The built-in role for a property key (the harness seeds the same).
pub fn default_role(key: &str) -> Role {
    if WHO.contains(&key) {
        Role::Who
    } else if WHEN.contains(&key) {
        Role::When
    } else if STATE.contains(&key) {
        Role::State
    } else if SETUP.contains(&key) {
        Role::Setup
    } else {
        Role::Details
    }
}

/// The role a relation's `role` field (or, when absent, its key) gives it.
pub fn role_of(rel: &RelationDef) -> Role {
    Role::parse(&rel.role).unwrap_or_else(|| default_role(&rel.key))
}

/// The role a relation object's fields give it (the property page's picker).
pub fn role_of_fields(role: &str, key: &str) -> Role {
    Role::parse(role).unwrap_or_else(|| default_role(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_role_wins_over_the_built_in_place() {
        let rel = |key: &str, role: &str| RelationDef {
            key: key.into(),
            role: role.into(),
            ..Default::default()
        };
        assert_eq!(role_of(&rel("agent", "")), Role::Who);
        assert_eq!(role_of(&rel("due_date", "")), Role::When);
        assert_eq!(role_of(&rel("status", "")), Role::State);
        assert_eq!(role_of(&rel("model", "")), Role::Setup);
        assert_eq!(role_of(&rel("customer", "")), Role::Details);
        assert_eq!(role_of(&rel("customer", "who")), Role::Who);
        assert_eq!(role_of(&rel("agent", "nonsense")), Role::Who);
    }
}
