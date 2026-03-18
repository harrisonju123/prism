use std::collections::{HashMap, HashSet, VecDeque};

/// Find a path from `from` to `to` using BFS over 4-directional movement.
///
/// Returns the path from `from` (exclusive) to `to` (inclusive), or an empty
/// `Vec` if the destination is unreachable.
pub fn find_path(
    from: (i32, i32),
    to: (i32, i32),
    walkable: &HashSet<(i32, i32)>,
) -> Vec<(i32, i32)> {
    if from == to {
        return vec![];
    }
    if !walkable.contains(&to) {
        return vec![];
    }

    let mut queue = VecDeque::new();
    let mut visited = HashSet::new();
    let mut parent: HashMap<(i32, i32), (i32, i32)> = HashMap::new();

    queue.push_back(from);
    visited.insert(from);

    'bfs: while let Some(pos) = queue.pop_front() {
        for (dx, dy) in [(0, 1), (0, -1), (1, 0), (-1, 0)] {
            let next = (pos.0 + dx, pos.1 + dy);
            if visited.contains(&next) {
                continue;
            }
            if !walkable.contains(&next) && next != from {
                continue;
            }
            visited.insert(next);
            parent.insert(next, pos);
            if next == to {
                break 'bfs;
            }
            queue.push_back(next);
        }
    }

    // Reconstruct path.
    if !parent.contains_key(&to) {
        return vec![];
    }
    let mut path = vec![to];
    let mut cur = to;
    while cur != from {
        cur = parent[&cur];
        if cur != from {
            path.push(cur);
        }
    }
    path.reverse();
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_line() {
        let walkable: HashSet<(i32, i32)> = (0..5).map(|x| (x, 0)).collect();
        let path = find_path((0, 0), (4, 0), &walkable);
        assert_eq!(path, vec![(1, 0), (2, 0), (3, 0), (4, 0)]);
    }

    #[test]
    fn unreachable() {
        let walkable: HashSet<(i32, i32)> = [(0, 0), (5, 0)].into_iter().collect();
        let path = find_path((0, 0), (5, 0), &walkable);
        assert!(path.is_empty());
    }

    #[test]
    fn same_tile() {
        let walkable: HashSet<(i32, i32)> = [(0, 0)].into_iter().collect();
        let path = find_path((0, 0), (0, 0), &walkable);
        assert!(path.is_empty());
    }
}
