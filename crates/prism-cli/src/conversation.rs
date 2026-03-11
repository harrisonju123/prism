use chrono::Utc;
use prism_types::{Message, MessageRole};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationNode {
    pub id: u32,
    pub parent_id: Option<u32>,
    pub message: Message,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchSummary {
    pub node_id: u32,
    pub role: MessageRole,
    /// Number of messages from this node to the deepest descendant
    pub depth: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTree {
    nodes: Vec<ConversationNode>,
    next_id: u32,
    active_leaf: u32,
}

impl ConversationTree {
    /// Create a tree from a linear message list (v1 migration).
    pub fn from_messages(msgs: Vec<Message>) -> Self {
        let mut tree = Self {
            nodes: Vec::new(),
            next_id: 0,
            active_leaf: 0,
        };

        if msgs.is_empty() {
            return tree;
        }

        for msg in msgs {
            tree.push(msg);
        }

        tree
    }

    /// Append a message as a child of the current active_leaf.
    pub fn push(&mut self, msg: Message) -> u32 {
        let id = self.next_id;
        let parent_id = if self.nodes.is_empty() {
            None
        } else {
            Some(self.active_leaf)
        };

        let node = ConversationNode {
            id,
            parent_id,
            message: msg,
            created_at: Utc::now().to_rfc3339(),
        };
        debug_assert_eq!(
            node.id,
            self.nodes.len() as u32,
            "node.id must equal its index in nodes vec"
        );
        self.nodes.push(node);

        self.next_id += 1;
        self.active_leaf = id;
        id
    }

    /// Walk from root to active_leaf, returning messages in order.
    pub fn active_path(&self) -> Vec<Message> {
        let ancestors = self.ancestors(self.active_leaf);
        ancestors
            .into_iter()
            .map(|id| self.nodes[id as usize].message.clone())
            .collect()
    }

    /// Number of messages on the active path.
    pub fn active_message_count(&self) -> usize {
        self.ancestors(self.active_leaf).len()
    }

    /// Move active_leaf back past the last assistant turn on the active path.
    /// Old nodes stay in the tree (creating a branch point).
    /// Returns the number of messages "removed" from the active path.
    pub fn undo(&mut self) -> usize {
        let path = self.ancestors(self.active_leaf);
        if path.is_empty() {
            return 0;
        }

        // Find the last assistant message on the active path
        let ast_pos = path
            .iter()
            .rposition(|&id| self.nodes[id as usize].message.role == MessageRole::Assistant);

        let Some(pos) = ast_pos else { return 0 };

        // Move active_leaf to the node just before the assistant message
        let removed = path.len() - pos;
        if pos == 0 {
            // The assistant message is the root — nothing before it
            // This shouldn't happen in normal usage, but handle gracefully
            return 0;
        }

        self.active_leaf = path[pos - 1];
        removed
    }

    /// List sibling branches at a given node (children of the same parent).
    pub fn branches_at(&self, node_id: u32) -> Vec<BranchSummary> {
        let parent_id = self.nodes.get(node_id as usize).and_then(|n| n.parent_id);

        self.nodes
            .iter()
            .filter(|n| n.parent_id == parent_id && n.id != node_id)
            .map(|n| BranchSummary {
                node_id: n.id,
                role: n.message.role.clone(),
                depth: self.subtree_depth(n.id),
            })
            .collect()
    }

    /// Switch active_leaf to the deepest descendant of the given node.
    pub fn switch_branch(&mut self, node_id: u32) {
        self.active_leaf = self.deepest_descendant(node_id);
    }

    /// Replace the active path with new messages (used after compression).
    /// Creates a new branch from the root's parent, effectively forking.
    pub fn replace_active_path(&mut self, msgs: Vec<Message>) {
        // Find the root of the current active path
        let path = self.ancestors(self.active_leaf);
        let root_parent = if path.is_empty() {
            None
        } else {
            self.nodes[path[0] as usize].parent_id
        };

        // Build new branch: first node's parent is root_parent, rest chain
        let mut prev_id = root_parent;
        for msg in msgs {
            let id = self.next_id;
            self.nodes.push(ConversationNode {
                id,
                parent_id: prev_id,
                message: msg,
                created_at: Utc::now().to_rfc3339(),
            });
            self.next_id += 1;
            prev_id = Some(id);
        }

        if let Some(leaf) = prev_id {
            self.active_leaf = leaf;
        }
    }

    /// Returns true if any node has more than one child (indicating the user retried/branched).
    pub fn has_branches(&self) -> bool {
        !self.branch_points().is_empty()
    }

    /// All branch point node IDs (nodes with more than one child).
    pub fn branch_points(&self) -> Vec<(u32, Vec<BranchSummary>)> {
        use std::collections::HashMap;

        let mut children_of: HashMap<Option<u32>, Vec<u32>> = HashMap::new();
        for node in &self.nodes {
            children_of.entry(node.parent_id).or_default().push(node.id);
        }

        children_of
            .into_iter()
            .filter(|(_, kids)| kids.len() > 1)
            .filter_map(|(parent, kids)| {
                let parent_id = parent?;
                let summaries: Vec<BranchSummary> = kids
                    .iter()
                    .map(|&kid| BranchSummary {
                        node_id: kid,
                        role: self.nodes[kid as usize].message.role.clone(),
                        depth: self.subtree_depth(kid),
                    })
                    .collect();
                Some((parent_id, summaries))
            })
            .collect()
    }

    // --- private helpers ---

    /// Return node IDs from root to the given node (inclusive).
    fn ancestors(&self, node_id: u32) -> Vec<u32> {
        let mut path = Vec::new();
        let mut current = Some(node_id);

        while let Some(id) = current {
            if let Some(node) = self.nodes.get(id as usize) {
                path.push(id);
                current = node.parent_id;
            } else {
                break;
            }
        }

        path.reverse();
        path
    }

    fn deepest_descendant(&self, node_id: u32) -> u32 {
        // BFS to find the deepest child following the last-child path
        let mut current = node_id;
        loop {
            let children: Vec<u32> = self
                .nodes
                .iter()
                .filter(|n| n.parent_id == Some(current))
                .map(|n| n.id)
                .collect();

            if children.is_empty() {
                return current;
            }
            // Follow the last (most recent) child
            current = *children.last().unwrap();
        }
    }

    fn subtree_depth(&self, node_id: u32) -> usize {
        let mut depth = 1;
        let mut current = node_id;
        loop {
            let children: Vec<u32> = self
                .nodes
                .iter()
                .filter(|n| n.parent_id == Some(current))
                .map(|n| n.id)
                .collect();

            if children.is_empty() {
                return depth;
            }
            current = *children.last().unwrap();
            depth += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg(role: MessageRole) -> Message {
        Message {
            role,
            content: Some(json!("test")),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        }
    }

    fn tool_msg(id: &str) -> Message {
        Message {
            role: MessageRole::Tool,
            content: Some(json!("result")),
            name: None,
            tool_calls: None,
            tool_call_id: Some(id.to_string()),
            extra: Default::default(),
        }
    }

    #[test]
    fn from_messages_creates_linear_tree() {
        let msgs = vec![
            msg(MessageRole::System),
            msg(MessageRole::User),
            msg(MessageRole::Assistant),
        ];
        let tree = ConversationTree::from_messages(msgs);
        assert_eq!(tree.active_message_count(), 3);
        assert_eq!(tree.active_leaf, 2);
        assert_eq!(tree.nodes[0].parent_id, None);
        assert_eq!(tree.nodes[1].parent_id, Some(0));
        assert_eq!(tree.nodes[2].parent_id, Some(1));
    }

    #[test]
    fn push_extends_active_path() {
        let mut tree =
            ConversationTree::from_messages(vec![msg(MessageRole::System), msg(MessageRole::User)]);
        let id = tree.push(msg(MessageRole::Assistant));
        assert_eq!(id, 2);
        assert_eq!(tree.active_message_count(), 3);
    }

    #[test]
    fn undo_removes_last_assistant_turn() {
        let mut tree = ConversationTree::from_messages(vec![
            msg(MessageRole::System),
            msg(MessageRole::User),
            msg(MessageRole::Assistant),
        ]);
        let removed = tree.undo();
        assert_eq!(removed, 1);
        assert_eq!(tree.active_message_count(), 2);
        let path = tree.active_path();
        assert_eq!(path[0].role, MessageRole::System);
        assert_eq!(path[1].role, MessageRole::User);
    }

    #[test]
    fn undo_removes_assistant_and_tools() {
        let mut tree = ConversationTree::from_messages(vec![
            msg(MessageRole::System),
            msg(MessageRole::User),
            msg(MessageRole::Assistant),
            tool_msg("tc1"),
            tool_msg("tc2"),
        ]);
        let removed = tree.undo();
        assert_eq!(removed, 3); // assistant + 2 tools
        assert_eq!(tree.active_message_count(), 2);
    }

    #[test]
    fn undo_no_assistant_returns_zero() {
        let mut tree =
            ConversationTree::from_messages(vec![msg(MessageRole::System), msg(MessageRole::User)]);
        let removed = tree.undo();
        assert_eq!(removed, 0);
        assert_eq!(tree.active_message_count(), 2);
    }

    #[test]
    fn undo_multiple_turns() {
        let mut tree = ConversationTree::from_messages(vec![
            msg(MessageRole::System),
            msg(MessageRole::User),
            msg(MessageRole::Assistant),
            msg(MessageRole::User),
            msg(MessageRole::Assistant),
        ]);
        // First undo: removes second assistant turn
        let removed = tree.undo();
        assert_eq!(removed, 1);
        assert_eq!(tree.active_message_count(), 4);
        // Second undo: removes second user + first assistant
        let removed = tree.undo();
        assert_eq!(removed, 2);
        assert_eq!(tree.active_message_count(), 2);
    }

    #[test]
    fn undo_creates_branch_point() {
        let mut tree = ConversationTree::from_messages(vec![
            msg(MessageRole::System),
            msg(MessageRole::User),
            msg(MessageRole::Assistant),
        ]);

        // Undo, then push a new assistant response
        tree.undo();
        tree.push(msg(MessageRole::Assistant));

        // Node 1 (user) now has two children: node 2 (old) and node 3 (new)
        let branches = tree.branches_at(3);
        assert_eq!(branches.len(), 1); // one sibling
        assert_eq!(branches[0].node_id, 2);
    }

    #[test]
    fn switch_branch_follows_deepest_descendant() {
        let mut tree = ConversationTree::from_messages(vec![
            msg(MessageRole::System),
            msg(MessageRole::User),
            msg(MessageRole::Assistant),
            msg(MessageRole::User),
            msg(MessageRole::Assistant),
        ]);

        // Undo twice back to system+user, add new branch
        tree.undo();
        tree.undo();
        tree.push(msg(MessageRole::Assistant)); // node 5

        // Switch back to original branch (node 2)
        tree.switch_branch(2);
        assert_eq!(tree.active_message_count(), 5); // original full path
        assert_eq!(tree.active_leaf, 4);

        // Switch to new branch
        tree.switch_branch(5);
        assert_eq!(tree.active_message_count(), 3);
        assert_eq!(tree.active_leaf, 5);
    }

    #[test]
    fn replace_active_path() {
        let mut tree = ConversationTree::from_messages(vec![
            msg(MessageRole::System),
            msg(MessageRole::User),
            msg(MessageRole::Assistant),
            msg(MessageRole::User),
            msg(MessageRole::Assistant),
        ]);

        // Simulate compression: replace with shorter sequence
        let compressed = vec![msg(MessageRole::System), msg(MessageRole::User)];
        tree.replace_active_path(compressed);
        assert_eq!(tree.active_message_count(), 2);
        // Old nodes still exist in the tree
        assert_eq!(tree.nodes.len(), 7); // 5 original + 2 new
    }

    #[test]
    fn branch_points_lists_forks() {
        let mut tree = ConversationTree::from_messages(vec![
            msg(MessageRole::System),
            msg(MessageRole::User),
            msg(MessageRole::Assistant),
        ]);

        tree.undo();
        tree.push(msg(MessageRole::Assistant));

        let points = tree.branch_points();
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].0, 1); // user node is the branch point
        assert_eq!(points[0].1.len(), 2); // two children
    }

    #[test]
    fn has_branches_detects_forks() {
        let mut tree = ConversationTree::from_messages(vec![
            msg(MessageRole::System),
            msg(MessageRole::User),
            msg(MessageRole::Assistant),
        ]);
        assert!(!tree.has_branches());

        tree.undo();
        tree.push(msg(MessageRole::Assistant));
        assert!(tree.has_branches());
    }

    #[test]
    fn has_branches_linear_is_false() {
        let tree = ConversationTree::from_messages(vec![
            msg(MessageRole::System),
            msg(MessageRole::User),
            msg(MessageRole::Assistant),
            msg(MessageRole::User),
            msg(MessageRole::Assistant),
        ]);
        assert!(!tree.has_branches());
    }

    #[test]
    fn empty_tree() {
        let tree = ConversationTree::from_messages(vec![]);
        assert_eq!(tree.active_message_count(), 0);
        assert!(tree.active_path().is_empty());
    }

    #[test]
    fn serialization_roundtrip() {
        let tree = ConversationTree::from_messages(vec![
            msg(MessageRole::System),
            msg(MessageRole::User),
            msg(MessageRole::Assistant),
        ]);
        let json = serde_json::to_string(&tree).unwrap();
        let restored: ConversationTree = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.active_message_count(), 3);
        assert_eq!(restored.active_leaf, 2);
    }
}
