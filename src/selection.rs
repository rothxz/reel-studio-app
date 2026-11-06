//! The timeline's clip selection: any number of clips, one of them the
//! primary (the last picked), which the inspector, the toolbar's clip
//! actions and ⌘C / ⌘D act on. Edits that make sense on many clips
//! (delete, deactivate) take them all.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    /// In the order picked; the last one is the primary.
    ids: Vec<String>,
}

impl Selection {
    /// The clip the inspector and single-clip actions follow.
    pub fn primary(&self) -> Option<&str> {
        self.ids.last().map(String::as_str)
    }

    /// The primary clip when it's the only one (trim handles, keyframes).
    pub fn single(&self) -> Option<&str> {
        (self.ids.len() == 1).then(|| self.ids[0].as_str())
    }

    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    pub fn contains(&self, id: &str) -> bool {
        self.ids.iter().any(|s| s == id)
    }

    /// Just this clip.
    pub fn set(&mut self, id: impl Into<String>) {
        self.ids = vec![id.into()];
    }

    /// Just this clip, or nothing.
    pub fn set_opt(&mut self, id: Option<String>) {
        self.ids = id.into_iter().collect();
    }

    /// Exactly these clips (the last becomes the primary).
    pub fn set_all(&mut self, ids: impl IntoIterator<Item = String>) {
        self.ids.clear();
        for id in ids {
            self.add(id);
        }
    }

    /// Adds a clip as the new primary (moved to the end if already in).
    pub fn add(&mut self, id: impl Into<String>) {
        let id = id.into();
        self.ids.retain(|s| *s != id);
        self.ids.push(id);
    }

    /// ⇧ / ⌘ click: in if it wasn't, out if it was.
    pub fn toggle(&mut self, id: &str) {
        if self.contains(id) {
            self.ids.retain(|s| s != id);
        } else {
            self.ids.push(id.to_string());
        }
    }

    pub fn clear(&mut self) {
        self.ids.clear();
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&str) -> bool) {
        self.ids.retain(|s| keep(s));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_picked_is_the_primary() {
        let mut s = Selection::default();
        assert_eq!(s.primary(), None);
        s.set("a");
        assert_eq!(s.single(), Some("a"));
        s.toggle("b");
        assert_eq!(s.primary(), Some("b"));
        assert_eq!(s.single(), None);
        s.add("a");
        assert_eq!(s.ids(), ["b", "a"]);
        s.toggle("a");
        assert_eq!(s.ids(), ["b"]);
        assert_eq!(s.single(), Some("b"));
    }

    #[test]
    fn set_all_dedupes() {
        let mut s = Selection::default();
        s.set_all(["a".into(), "b".into(), "a".into()]);
        assert_eq!(s.ids(), ["b", "a"]);
        s.retain(|id| id != "a");
        assert_eq!(s.primary(), Some("b"));
    }
}
