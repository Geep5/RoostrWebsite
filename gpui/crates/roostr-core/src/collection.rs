//! A collection's members (`collectionIds`): their order is the collection's
//! own order, which its views show while no view sort overrides it.

use std::collections::HashMap;

use crate::types::{Fields, QueryRow};

/// The member ids, in order.
pub fn members(fields: &Fields) -> Vec<String> {
    fields
        .get("collectionIds")
        .map(|v| {
            v.items()
                .iter()
                .filter_map(|i| i.string_value.clone())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Order `rows` as `members` lists them (rows not listed keep their order, last).
pub fn sort_by_members(rows: &mut [QueryRow], members: &[String]) {
    let at: HashMap<&str, usize> = members
        .iter()
        .enumerate()
        .map(|(i, m)| (m.as_str(), i))
        .collect();
    rows.sort_by_key(|r| at.get(r.id.as_str()).copied().unwrap_or(usize::MAX));
}

/// `members` with `moved` taken out and put back before `target` (after it when
/// `after`); `None` when that changes nothing or either isn't a member.
pub fn moved(members: &[String], moved: &str, target: &str, after: bool) -> Option<Vec<String>> {
    if moved == target || !members.iter().any(|m| m == moved) {
        return None;
    }
    let mut next: Vec<String> = members.iter().filter(|m| *m != moved).cloned().collect();
    let at = next.iter().position(|m| m == target)? + usize::from(after);
    next.insert(at, moved.to_string());
    (next != members).then_some(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn moving_a_member_puts_it_beside_the_target() {
        let m = ids(&["a", "b", "c", "d"]);
        assert_eq!(moved(&m, "a", "c", true), Some(ids(&["b", "c", "a", "d"])));
        assert_eq!(moved(&m, "d", "a", false), Some(ids(&["d", "a", "b", "c"])));
        // In place: before its next, after its previous, onto itself.
        assert_eq!(moved(&m, "b", "c", false), None);
        assert_eq!(moved(&m, "b", "a", true), None);
        assert_eq!(moved(&m, "b", "b", true), None);
        assert_eq!(moved(&m, "x", "a", true), None);
    }

    #[test]
    fn rows_follow_the_member_order() {
        let row = |id: &str| QueryRow {
            id: id.into(),
            ..QueryRow::default()
        };
        let mut rows = vec![row("x"), row("c"), row("a"), row("b")];
        sort_by_members(&mut rows, &ids(&["a", "b", "c"]));
        let got: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(got, ["a", "b", "c", "x"]);
    }
}
