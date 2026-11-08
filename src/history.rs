//! Undo / redo. The project document is plain data, so a step is the
//! document as it was before the edit. A run of edits to the same thing
//! close together (a slider drag, a nudge held down) is one step, like the
//! old editor's coalescing commands.

use std::time::{Duration, Instant};

use crate::model::Project;

/// Edits with the same key closer than this merge into one undo step.
const COALESCE: Duration = Duration::from_millis(400);
const MAX_STEPS: usize = 200;

struct Step {
    label: &'static str,
    project: Project,
}

#[derive(Default)]
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// The last edit's coalescing key and when it happened.
    last: Option<(String, Instant)>,
}

impl History {
    /// Records an edit: `before` is the document as it was before it.
    /// With a `coalesce` key equal to the previous edit's (and soon after
    /// it), the edit joins that step instead of making a new one.
    pub fn record(&mut self, label: &'static str, before: Project, coalesce: Option<&str>) {
        let now = Instant::now();
        let joins = match (coalesce, &self.last) {
            (Some(key), Some((last, at))) => key == last && now - *at < COALESCE,
            _ => false,
        };
        self.last = coalesce.map(|k| (k.to_string(), now));
        self.redo.clear();
        if joins && !self.undo.is_empty() {
            return;
        }
        self.undo.push(Step {
            label,
            project: before,
        });
        if self.undo.len() > MAX_STEPS {
            self.undo.remove(0);
        }
    }

    /// The document to go back to (given the current one, kept for redo).
    pub fn undo(&mut self, current: Project) -> Option<(&'static str, Project)> {
        let step = self.undo.pop()?;
        self.last = None;
        self.redo.push(Step {
            label: step.label,
            project: current,
        });
        Some((step.label, step.project))
    }

    pub fn redo(&mut self, current: Project) -> Option<(&'static str, Project)> {
        let step = self.redo.pop()?;
        self.last = None;
        self.undo.push(Step {
            label: step.label,
            project: current,
        });
        Some((step.label, step.project))
    }

    /// Undo steps there are (the agent counts a turn's edits by it).
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn clear(&mut self) {
        *self = History::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> Project {
        let mut p = Project::empty();
        p.name = name.into();
        p
    }

    #[test]
    fn undo_redo_round_trip() {
        let mut h = History::default();
        h.record("Rename", named("a"), None);
        let (_, back) = h.undo(named("b")).unwrap();
        assert_eq!(back.name, "a");
        let (_, fwd) = h.redo(back).unwrap();
        assert_eq!(fwd.name, "b");
        assert!(h.can_undo() && !h.can_redo());
    }

    #[test]
    fn same_key_in_a_burst_is_one_step() {
        let mut h = History::default();
        h.record("Speed", named("a"), Some("speed:c1"));
        h.record("Speed", named("b"), Some("speed:c1"));
        h.record("Speed", named("c"), Some("speed:c1"));
        let (_, back) = h.undo(named("d")).unwrap();
        assert_eq!(back.name, "a");
        assert!(!h.can_undo());
    }

    #[test]
    fn a_new_edit_drops_redo() {
        let mut h = History::default();
        h.record("A", named("a"), None);
        h.undo(named("b"));
        h.record("B", named("a"), None);
        assert!(!h.can_redo());
    }
}
