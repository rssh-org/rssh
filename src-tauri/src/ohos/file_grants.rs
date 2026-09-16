use std::collections::HashSet;

/// Picker grants belong to one Ability lifetime. A late result must never
/// authorize a file after that Ability has been replaced.
#[derive(Default)]
pub struct FileGrants {
    generation: u64,
    readable: HashSet<String>,
    writable: HashSet<String>,
}

impl FileGrants {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.readable.clear();
        self.writable.clear();
    }

    pub fn grant(&mut self, generation: u64, uris: &[String], write: bool) -> bool {
        if generation != self.generation {
            return false;
        }
        let grants = if write {
            &mut self.writable
        } else {
            &mut self.readable
        };
        grants.extend(uris.iter().cloned());
        true
    }

    pub fn permits(&self, uri: &str, write: bool) -> bool {
        let grants = if write {
            &self.writable
        } else {
            &self.readable
        };
        grants.contains(uri)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_sources_cannot_be_overwritten_and_arbitrary_paths_are_rejected() {
        let mut grants = FileGrants::default();
        assert!(grants.grant(grants.generation(), &["file://selected".into()], false));
        assert!(grants.permits("file://selected", false));
        assert!(!grants.permits("file://selected", true));
        assert!(!grants.permits("/base/files/master.key", true));
    }

    #[test]
    fn destroyed_ability_revokes_grants_and_rejects_late_picker_results() {
        let mut grants = FileGrants::default();
        let previous = grants.generation();
        grants.grant(previous, &["file://saved".into()], true);
        grants.clear();
        assert!(!grants.permits("file://saved", true));
        assert!(!grants.grant(previous, &["file://late".into()], true));
        assert!(!grants.permits("file://late", true));
        assert!(grants.grant(grants.generation(), &["file://new".into()], true));
        assert!(grants.permits("file://new", true));
    }
}
