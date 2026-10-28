//! Opt-in AccessKit adapter for the existing immediate-mode controls.
//! Default frames allocate never-reused IDs, not global draw-order identities.
//! An explicit application identity epoch may preserve unchanged control IDs.
use accesskit::{
    Action, ActionData, Node, NodeId, TextDirection, TextPosition, TextSelection, TreeId, TreeInfo,
};
pub use accesskit::{ActionRequest, Role, TreeUpdate};
use liminalkit_raster::Rect;
use liminalkit_shell::input::AccessibleTextEdit;
use liminalkit_shell::{Input, MouseButton};
use unicode_segmentation::UnicodeSegmentation;

/// Bound native replacement requests without truncating their stored UTF-8.
pub const MAX_ACCESSIBLE_TEXT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Default)]
pub struct Semantics {
    title: String,
    nodes: Vec<(NodeId, Node)>,
    bounds: Vec<Rect>,
    focus: Option<NodeId>,
    previous_nodes: Vec<(NodeId, Node)>,
    scope: Option<u64>,
    next_id: u64,
    modal: bool,
    text_runs: Vec<(NodeId, NodeId, Node)>,
    previous_text_runs: Vec<(NodeId, NodeId, Node)>,
}
impl Semantics {
    /// Conservative frame identity: delayed actions from an older frame cannot
    /// address controls substituted at the same draw-order position.
    pub fn begin(&mut self, title: &str) {
        self.start_frame(title, None);
    }
    /// Preserve same-slot, same-role/label IDs within a declared identity epoch.
    /// The application MUST change epoch when a page, row identity or geometry
    /// is replaced, even if the new control has the same label (e.g. "Forget").
    /// Use `begin` instead if that contract cannot be guaranteed.
    pub fn begin_scoped(&mut self, title: &str, epoch: u64) {
        self.start_frame(title, Some(epoch));
    }
    fn start_frame(&mut self, title: &str, scope: Option<u64>) {
        let reuse = scope.is_some() && self.scope == scope && self.title == title;
        std::mem::swap(&mut self.nodes, &mut self.previous_nodes);
        std::mem::swap(&mut self.text_runs, &mut self.previous_text_runs);
        self.text_runs.clear();
        if !reuse {
            self.previous_text_runs.clear();
        }
        self.nodes.clear();
        if !reuse {
            self.previous_nodes.clear();
        }
        self.scope = scope;
        self.title.clear();
        self.title.push_str(title);
        self.bounds.clear();
        self.focus = None;
        self.modal = false;
    }
    /// Call before changing application identity or layout, not merely on the
    /// next redraw. Until a new frame is collected, old actions are rejected.
    pub fn invalidate(&mut self) {
        self.nodes.clear();
        self.previous_nodes.clear();
        self.text_runs.clear();
        self.previous_text_runs.clear();
        self.bounds.clear();
        self.focus = None;
        self.scope = None;
    }
    /// A modal overlay replaces background controls without discarding the
    /// previous frame's modal identities. Its owner still invalidates on transition.
    pub fn clear_controls(&mut self) {
        self.modal = true;
        self.nodes.clear();
        self.text_runs.clear();
        self.bounds.clear();
        self.focus = None;
    }
    /// The application's current interaction scope, not a second control tree.
    pub fn set_modal(&mut self, modal: bool) {
        self.modal = modal;
    }
    pub fn add(
        &mut self,
        role: Role,
        label: &str,
        area: Rect,
        focused: bool,
        checked: Option<bool>,
        value: Option<&str>,
    ) {
        let id = if let Some((id, _)) = self
            .previous_nodes
            .get(self.nodes.len())
            .filter(|(_, node)| node.role() == role && node.label() == Some(label))
        {
            *id
        } else {
            self.next_id = self
                .next_id
                .checked_add(1)
                .expect("semantic node identity exhausted");
            NodeId(self.next_id)
        };
        let mut node = Node::new(role);
        node.set_label(label);
        node.set_bounds(accesskit::Rect::new(
            area.x as f64,
            area.y as f64,
            area.right() as f64,
            area.bottom() as f64,
        ));
        node.add_action(Action::Focus);
        if !matches!(role, Role::TextInput | Role::PasswordInput) {
            node.add_action(Action::Click);
        }
        if let Some(checked) = checked {
            if matches!(role, Role::ListBoxOption | Role::MenuListOption) {
                node.set_selected(checked);
            } else {
                node.set_toggled(if checked {
                    accesskit::Toggled::True
                } else {
                    accesskit::Toggled::False
                });
            }
        }
        if role != Role::PasswordInput {
            if let Some(value) = value {
                node.set_value(value);
            }
        }
        if focused {
            self.focus = Some(id);
        }
        self.nodes.push((id, node));
        self.bounds.push(area);
    }
    /// A plain field's actual selectable text, not its placeholder or a masked value.
    /// Byte positions come from the editing state; AccessKit offsets name run characters.
    pub fn add_text_input(
        &mut self,
        label: &str,
        area: Rect,
        focused: bool,
        value: &str,
        anchor_byte: usize,
        caret_byte: usize,
    ) {
        self.add(Role::TextInput, label, area, focused, None, None);
        let owner = self.nodes.last().unwrap().0;
        let run_id = if let Some((_, id, _)) = self
            .previous_text_runs
            .iter()
            .find(|(parent, _, _)| *parent == owner)
        {
            *id
        } else {
            self.next_id = self
                .next_id
                .checked_add(1)
                .expect("semantic node identity exhausted");
            NodeId(self.next_id)
        };
        let mut lengths = Vec::new();
        for grapheme in value.graphemes(true) {
            if let Ok(length) = u8::try_from(grapheme.len()) {
                lengths.push(length);
            } else {
                // AccessKit's byte-length ABI cannot encode this cluster as one unit.
                // Preserve every byte; the editor still snaps actions to real clusters.
                lengths.extend(grapheme.chars().map(|character| character.len_utf8() as u8));
            }
        }
        let character_index = |byte: usize| {
            let mut end = 0;
            lengths
                .iter()
                .take_while(|length| {
                    end += usize::from(**length);
                    end <= byte
                })
                .count()
        };
        let selection = TextSelection {
            anchor: TextPosition {
                node: run_id,
                character_index: character_index(anchor_byte),
            },
            focus: TextPosition {
                node: run_id,
                character_index: character_index(caret_byte),
            },
        };
        let mut run = Node::new(Role::TextRun);
        run.set_value(value);
        run.set_character_lengths(lengths);
        run.set_text_direction(TextDirection::LeftToRight);
        run.set_bounds(accesskit::Rect::new(
            area.x as f64,
            area.y as f64,
            area.right() as f64,
            area.bottom() as f64,
        ));
        let node = &mut self.nodes.last_mut().unwrap().1;
        node.set_children(vec![run_id]);
        node.set_text_selection(selection);
        node.add_action(Action::SetValue);
        node.add_action(Action::SetTextSelection);
        self.text_runs.push((owner, run_id, run));
    }
    pub fn update(&self) -> TreeUpdate {
        let root = NodeId(0);
        let mut window = Node::new(if self.modal {
            Role::Dialog
        } else {
            Role::Window
        });
        if self.modal {
            window.set_modal();
        }
        window.set_label(self.title.clone());
        window.set_children(self.nodes.iter().map(|(id, _)| *id).collect::<Vec<_>>());
        let mut nodes = self.nodes.clone();
        nodes.extend(
            self.text_runs
                .iter()
                .map(|(_, id, node)| (*id, node.clone())),
        );
        nodes.push((root, window));
        TreeUpdate {
            nodes,
            tree: Some(TreeInfo::new(root)),
            tree_id: TreeId::ROOT,
            focus: self.focus.unwrap_or(root),
        }
    }
    /// Dispatch only actions actually advertised by the current frame. No
    /// separate widget tree or OS policy; existing input/focus handles effects.
    pub fn action(&self, input: &mut Input, request: ActionRequest) {
        if request.target_tree != TreeId::ROOT {
            return;
        }
        let Some(index) = self
            .nodes
            .iter()
            .position(|(id, _)| *id == request.target_node)
        else {
            return;
        };
        let node = &self.nodes[index].1;
        if !node.supports_action(request.action) {
            return;
        }
        let area = self.bounds[index];
        let (x, y) = (area.x + area.w / 2, area.y + area.h / 2);
        let edit = match (request.action, request.data) {
            (Action::SetValue, Some(ActionData::Value(value))) => {
                if value.len() > MAX_ACCESSIBLE_TEXT_BYTES {
                    return;
                }
                Some(AccessibleTextEdit::SetValue(value.into()))
            }
            (Action::SetTextSelection, Some(ActionData::SetTextSelection(selection))) => {
                let Some((_, run_id, run)) = self
                    .text_runs
                    .iter()
                    .find(|(parent, _, _)| *parent == request.target_node)
                else {
                    return;
                };
                if selection.anchor.node != *run_id || selection.focus.node != *run_id {
                    return;
                }
                let lengths = run.character_lengths();
                if selection.anchor.character_index > lengths.len()
                    || selection.focus.character_index > lengths.len()
                {
                    return;
                }
                let offset = |index| {
                    lengths[..index]
                        .iter()
                        .map(|length| usize::from(*length))
                        .sum()
                };
                Some(AccessibleTextEdit::SetSelection {
                    anchor: offset(selection.anchor.character_index),
                    focus: offset(selection.focus.character_index),
                })
            }
            (Action::SetValue | Action::SetTextSelection, _) => return,
            _ => None,
        };
        if let Some(edit) = edit {
            input.set_accessible_text_edit(area, edit);
        } else {
            input.focus_at(x, y);
        }
        if request.action == Action::Click {
            input.cursor_moved(x as f32, y as f32);
            input.mouse(MouseButton::Left, true);
            input.mouse(MouseButton::Left, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click(target: NodeId) -> ActionRequest {
        ActionRequest {
            action: Action::Click,
            target_node: target,
            target_tree: TreeId::ROOT,
            data: None,
        }
    }
    fn text_action(target: NodeId, action: Action, data: ActionData) -> ActionRequest {
        ActionRequest {
            action,
            target_node: target,
            target_tree: TreeId::ROOT,
            data: Some(data),
        }
    }

    #[test]
    fn plain_text_runs_preserve_utf8_and_real_selection_without_becoming_root_controls() {
        let mut semantics = Semantics::default();
        let area = Rect::new(0, 0, 100, 30);
        let value = "cafe\u{301}-家庭-🧑🏽‍💻";
        semantics.begin_scoped("Home", 1);
        semantics.add_text_input("Search", area, true, value, 3, value.len());
        let tree = semantics.update();
        let (owner, input) = &tree.nodes[0];
        let run_id = input.children()[0];
        let run = &tree.nodes.iter().find(|(id, _)| *id == run_id).unwrap().1;
        assert_eq!(input.value(), None);
        assert_eq!(run.role(), Role::TextRun);
        assert_eq!(run.value(), Some(value));
        assert_eq!(run.character_lengths(), &[1, 1, 1, 3, 1, 3, 3, 1, 15]);
        assert_eq!(input.text_selection().unwrap().anchor.character_index, 3);
        assert_eq!(input.text_selection().unwrap().focus.character_index, 9);
        assert_eq!(tree.nodes.last().unwrap().1.children(), &[*owner]);
        semantics.begin_scoped("Home", 1);
        semantics.add_text_input("Search", area, true, value, 0, 0);
        assert_eq!(semantics.update().nodes[0].1.children(), &[run_id]);
    }

    #[test]
    fn live_text_actions_dispatch_once_and_replaced_runs_cannot_edit_new_fields() {
        let mut semantics = Semantics::default();
        let area = Rect::new(0, 0, 100, 30);
        semantics.begin_scoped("Home", 1);
        semantics.add_text_input("Search", area, true, "cafe\u{301}", 0, 6);
        let tree = semantics.update();
        let owner = tree.nodes[0].0;
        let run = tree.nodes[0].1.children()[0];
        let selection = TextSelection {
            anchor: TextPosition {
                node: run,
                character_index: 3,
            },
            focus: TextPosition {
                node: run,
                character_index: 4,
            },
        };
        let mut input = Input::new();
        semantics.action(
            &mut input,
            text_action(
                owner,
                Action::SetTextSelection,
                ActionData::SetTextSelection(selection),
            ),
        );
        assert!(matches!(
            input.take_accessible_text_edit(area),
            Some(AccessibleTextEdit::SetSelection {
                anchor: 3,
                focus: 6
            })
        ));
        assert!(input.take_accessible_text_edit(area).is_none());
        let value = "家庭-🧑🏽‍💻";
        semantics.action(
            &mut input,
            text_action(owner, Action::SetValue, ActionData::Value(value.into())),
        );
        assert!(matches!(input.take_accessible_text_edit(area).as_ref(),
            Some(AccessibleTextEdit::SetValue(text)) if text == value));
        input.end_frame();
        semantics.invalidate();
        semantics.action(
            &mut input,
            text_action(owner, Action::SetValue, ActionData::Value("stale".into())),
        );
        assert!(input.take_accessible_text_edit(area).is_none());
        semantics.begin_scoped("Home", 2);
        semantics.add_text_input("Search", area, false, "", 0, 0);
        let replacement = semantics.update().nodes[0].0;
        semantics.action(
            &mut input,
            text_action(
                replacement,
                Action::SetTextSelection,
                ActionData::SetTextSelection(TextSelection {
                    anchor: TextPosition {
                        node: run,
                        character_index: 0,
                    },
                    focus: TextPosition {
                        node: run,
                        character_index: 0,
                    },
                }),
            ),
        );
        assert!(input.take_accessible_text_edit(area).is_none());
        assert!(!input.focus_requested(area));
        semantics.action(
            &mut input,
            text_action(
                replacement,
                Action::SetValue,
                ActionData::Value("x".repeat(MAX_ACCESSIBLE_TEXT_BYTES + 1).into()),
            ),
        );
        assert!(input.take_accessible_text_edit(area).is_none());
    }

    #[test]
    fn password_fields_never_publish_plaintext_runs_or_edit_actions() {
        let mut semantics = Semantics::default();
        let area = Rect::new(0, 0, 100, 30);
        semantics.begin("Credentials");
        semantics.add(
            Role::PasswordInput,
            "Password",
            area,
            true,
            None,
            Some("secret"),
        );
        let tree = semantics.update();
        let (owner, node) = &tree.nodes[0];
        assert!(node.children().is_empty());
        assert!(!node.supports_action(Action::SetValue));
        assert!(!node.supports_action(Action::SetTextSelection));
        assert!(tree
            .nodes
            .iter()
            .all(|(_, node)| node.value().is_none() && node.role() != Role::TextRun));
        let mut input = Input::new();
        semantics.action(
            &mut input,
            text_action(*owner, Action::SetValue, ActionData::Value("leak".into())),
        );
        assert!(input.take_accessible_text_edit(area).is_none());
    }

    #[test]
    fn oversized_grapheme_run_metadata_never_truncates_plaintext() {
        let mut semantics = Semantics::default();
        let value = format!("a{}", "\u{301}".repeat(300));
        semantics.begin("Home");
        semantics.add_text_input(
            "Search",
            Rect::new(0, 0, 100, 30),
            true,
            &value,
            0,
            value.len(),
        );
        let tree = semantics.update();
        let run = tree
            .nodes
            .iter()
            .find(|(_, node)| node.role() == Role::TextRun)
            .unwrap();
        assert_eq!(run.1.value(), Some(value.as_str()));
        assert_eq!(
            run.1
                .character_lengths()
                .iter()
                .map(|length| usize::from(*length))
                .sum::<usize>(),
            value.len()
        );
    }

    #[test]
    fn delayed_reset_cannot_click_a_substituted_same_label_control() {
        let mut semantics = Semantics::default();
        let area = Rect::new(0, 0, 100, 30);
        semantics.begin("Settings");
        semantics.add(Role::Button, "Reset", area, false, None, None);
        let old = semantics.update().nodes[0].0;
        semantics.begin("Settings");
        semantics.add(Role::Button, "Reset", area, false, None, None);
        let new = semantics.update().nodes[0].0;
        assert_ne!(old, new);
        let mut input = Input::new();
        semantics.action(&mut input, click(old));
        assert!(input.press_position().is_none());
        assert!(!input.take_click(area));
        assert!(!input.focus_requested(area));
        semantics.action(&mut input, click(new));
        assert!(input.take_click(area));
    }

    #[test]
    fn explicit_epoch_preserves_controls_but_retires_replaced_forget_rows() {
        let mut semantics = Semantics::default();
        let area = Rect::new(0, 0, 100, 30);
        semantics.begin_scoped("Index", 1);
        semantics.add(Role::Button, "Forget", area, false, None, None);
        let old = semantics.update().nodes[0].0;
        semantics.begin_scoped("Index", 1);
        semantics.add(Role::Button, "Forget", area, false, None, None);
        assert_eq!(old, semantics.update().nodes[0].0);
        semantics.invalidate();
        let mut input = Input::new();
        semantics.action(&mut input, click(old));
        assert!(input.press_position().is_none());
        assert!(!input.take_click(area));
        semantics.begin_scoped("Index", 2);
        semantics.add(Role::Button, "Forget", area, false, None, None);
        assert_ne!(old, semantics.update().nodes[0].0);
        semantics.action(&mut input, click(old));
        assert!(input.press_position().is_none());
        assert!(!input.take_click(area));
    }

    #[test]
    fn modal_overlay_retires_background_actions_and_keeps_its_current_controls() {
        let mut semantics = Semantics::default();
        let area = Rect::new(0, 0, 100, 30);
        semantics.begin_scoped("Menu", 1);
        semantics.add(
            Role::Button,
            "Delete background item",
            area,
            false,
            None,
            None,
        );
        let background = semantics.update().nodes[0].0;
        semantics.clear_controls();
        semantics.add(Role::MenuItem, "Cancel", area, true, None, None);
        let current = semantics.update().nodes[0].0;
        let mut input = Input::new();
        semantics.action(&mut input, click(background));
        assert!(!input.take_click(area));
        semantics.action(&mut input, click(current));
        assert!(input.take_click(area));
        let tree = semantics.update();
        assert_eq!(tree.nodes.last().unwrap().1.role(), Role::Dialog);
        assert!(tree.nodes.last().unwrap().1.is_modal());
        semantics.begin_scoped("Home", 2);
        let tree = semantics.update();
        assert_eq!(tree.nodes.last().unwrap().1.role(), Role::Window);
        assert!(!tree.nodes.last().unwrap().1.is_modal());
    }
    #[test]
    fn text_and_password_fields_do_not_advertise_pointer_activation() {
        let mut semantics = Semantics::default();
        semantics.begin("Credentials");
        for role in [Role::TextInput, Role::PasswordInput] {
            semantics.add(role, "Field", Rect::new(0, 0, 100, 20), false, None, None);
        }
        let tree = semantics.update();
        for (_, node) in tree.nodes.iter().filter(|(id, _)| *id != NodeId(0)) {
            assert!(node.supports_action(Action::Focus));
            assert!(!node.supports_action(Action::Click));
        }
    }
}
