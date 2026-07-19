//! Card-layout helpers derived from the plan tree: parsing an operator's one-line
//! `detail` into a key/value definition grid ([`detail_parts`]) and computing the
//! tree connector rails for a node in the flat, pre-order list ([`guide_rails`]).
//! Both mirror the v19 design (`planDetailParts` / `_planGuides`).

use super::tree::PlanNode;

/// One parsed field of an operator's one-line `detail` — a `key=value` pair
/// (bracket-aware) or a bare fragment (`has_key == false`, spans both columns).
#[derive(Clone, Debug, PartialEq)]
pub struct DetailPart {
    pub key: String,
    pub val: String,
    pub has_key: bool,
}

/// Split an operator's `detail` string into typed key/value parts for the card's
/// definition grid — mirrors the v19 design's `planDetailParts`. Commas inside
/// `()`/`[]`/`{}` don't split; a leading `key=` is lifted out only when the head is
/// a short (`< 26` byte) identifier.
pub fn detail_parts(detail: &str) -> Vec<DetailPart> {
    let mut raw: Vec<String> = Vec::new();
    let mut depth: i32 = 0;
    let mut cur = String::new();
    for ch in detail.chars() {
        match ch {
            '{' | '[' | '(' => {
                depth += 1;
                cur.push(ch);
            }
            '}' | ']' | ')' => {
                depth = (depth - 1).max(0);
                cur.push(ch);
            }
            ',' if depth == 0 => {
                let t = cur.trim();
                if !t.is_empty() {
                    raw.push(t.to_string());
                }
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    let t = cur.trim();
    if !t.is_empty() {
        raw.push(t.to_string());
    }

    raw.into_iter()
       .map(|p| {
           if let Some(eq) = p.find('=') {
               let head = &p[..eq];
               if eq > 0 && eq < 26 && is_detail_ident(head) {
                   return DetailPart {
                       key: head.to_string(),
                       val: p[eq + 1..].to_string(),
                       has_key: true,
                   };
               }
           }
           DetailPart {
               key: String::new(),
               val: p,
               has_key: false,
           }
       })
       .collect()
}

/// `^[A-Za-z][A-Za-z0-9_]*$` — the design's key-head test.
fn is_detail_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Which ancestor columns keep a vertical tree rail for node `i` in a flat
/// pre-order list (`true` = draw the line). Mirrors the design's `_planGuides`:
/// an ancestor column stays lit while it has a following node at that depth; the
/// node's own (elbow) column stays lit while it has a following sibling.
pub fn guide_rails(nodes: &[PlanNode], i: usize) -> Vec<bool> {
    let d = nodes[i].depth;
    let mut rails = Vec::with_capacity(d);
    for l in 0..d {
        let eq_depth = if l + 1 < d { l } else { d };
        let mut on = false;
        for n in &nodes[i + 1..] {
            if n.depth < eq_depth {
                break;
            }
            if n.depth == eq_depth {
                on = true;
                break;
            }
        }
        rails.push(on);
    }
    rails
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::plan::{PlanKind, PlanNode};

    #[test]
    fn parses_detail_into_kv_parts() {
        let parts = detail_parts("mode=Partitioned, join_type=Inner, on=[(user_id@0, user_id@0)]");
        assert_eq!(parts.len(), 3);
        assert!(parts[0].has_key && parts[0].key == "mode" && parts[0].val == "Partitioned");
        assert_eq!(parts[1].key, "join_type");
        // The comma inside the brackets must not split the `on` value.
        assert_eq!(parts[2].key, "on");
        assert_eq!(parts[2].val, "[(user_id@0, user_id@0)]");
        // A bare fragment with no leading identifier stays key-less.
        let bare = detail_parts("TableScan: t");
        assert!(!bare[0].has_key);
    }

    #[test]
    fn guide_rails_track_following_siblings() {
        let n = |depth: usize| PlanNode {
            name: String::new(),
            detail: String::new(),
            kind: PlanKind::Util,
            depth,
            rows: None,
            self_ms: None,
            self_label: String::new(),
            metrics: Vec::new(),
        };
        // root → child → two grandchildren. The first grandchild's own (elbow)
        // column stays lit because a depth-2 sibling still follows; its ancestor
        // (depth-0) column is off (the root has no sibling). The last grandchild
        // lights nothing (no following siblings). The root has no rails.
        let nodes = vec![n(0), n(1), n(2), n(2)];
        assert_eq!(guide_rails(&nodes, 0), Vec::<bool>::new());
        assert_eq!(guide_rails(&nodes, 2), vec![false, true]);
        assert_eq!(guide_rails(&nodes, 3), vec![false, false]);
    }
}
