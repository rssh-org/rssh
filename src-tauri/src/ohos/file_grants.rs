use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, PartialEq)]
enum WriteGrant {
    SelectedFile,
    DirectoryChild,
}

/// Picker grants belong to one Ability lifetime. A late result must never
/// authorize a file after that Ability has been replaced.
#[derive(Default)]
pub struct FileGrants {
    generation: u64,
    readable: HashSet<String>,
    writable: HashMap<String, WriteGrant>,
    readable_directories: HashSet<String>,
    writable_directories: HashSet<String>,
}

impl FileGrants {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.readable.clear();
        self.writable.clear();
        self.readable_directories.clear();
        self.writable_directories.clear();
    }

    pub fn grant(&mut self, generation: u64, uris: &[String], write: bool) -> bool {
        if generation != self.generation {
            return false;
        }
        if write {
            self.writable.extend(
                uris.iter()
                    .cloned()
                    .map(|uri| (uri, WriteGrant::SelectedFile)),
            );
        } else {
            self.readable.extend(uris.iter().cloned());
        }
        true
    }

    /// Only files resolved below a selected directory may use the native path
    /// creation API. A selected file itself continues through its URI provider.
    pub fn grant_directory_files(&mut self, generation: u64, uris: &[String], write: bool) -> bool {
        if !write {
            return self.grant(generation, uris, false);
        }
        if generation != self.generation {
            return false;
        }
        self.writable.extend(
            uris.iter()
                .cloned()
                .map(|uri| (uri, WriteGrant::DirectoryChild)),
        );
        true
    }

    pub fn creates_in_directory(&self, uri: &str) -> bool {
        self.writable.get(uri) == Some(&WriteGrant::DirectoryChild)
    }

    pub fn permits(&self, uri: &str, write: bool) -> bool {
        if write {
            self.writable.contains_key(uri)
        } else {
            self.readable.contains(uri)
        }
    }

    pub fn grant_directory(&mut self, generation: u64, uri: String, write: bool) -> bool {
        if generation != self.generation {
            return false;
        }
        let grants = if write {
            &mut self.writable_directories
        } else {
            &mut self.readable_directories
        };
        grants.insert(uri);
        true
    }

    pub fn permits_directory(&self, uri: &str, write: bool) -> bool {
        let grants = if write {
            &self.writable_directories
        } else {
            &self.readable_directories
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

    #[test]
    fn directory_grants_are_exact_scoped_and_do_not_authorize_arbitrary_descendants() {
        let mut grants = FileGrants::default();
        let generation = grants.generation();
        assert!(grants.grant_directory(generation, "file://docs/source".into(), false));
        assert!(grants.permits_directory("file://docs/source", false));
        assert!(!grants.permits_directory("file://docs/source", true));
        assert!(!grants.permits_directory("file://docs/source-other", false));
        assert!(!grants.permits("file://docs/source/private.txt", false));
        assert!(!grants.permits("file://docs/source", false));
        grants.clear();
        assert!(!grants.permits_directory("file://docs/source", false));
        assert!(!grants.grant_directory(generation, "file://docs/late".into(), true));
        assert!(!grants.grant(generation, &["file://docs/source/late.txt".into()], false));
    }

    #[test]
    fn directory_creation_is_carried_by_the_grant_not_the_uri_string() {
        let mut grants = FileGrants::default();
        let generation = grants.generation();
        let uri = "file://docs/storage/Users/currentUser/Download/child.txt";
        assert!(grants.grant(generation, &[uri.into()], true));
        assert!(grants.permits(uri, true));
        assert!(!grants.creates_in_directory(uri));
        assert!(grants.grant_directory_files(generation, &[uri.into()], true));
        assert!(grants.creates_in_directory(uri));
        assert!(!grants.permits(uri, false));
        assert!(!grants.creates_in_directory(&format!("{uri}/other")));
        grants.clear();
        assert!(!grants.creates_in_directory(uri));
        assert!(!grants.grant_directory_files(generation, &[uri.into()], true));
        assert!(!grants.permits(uri, true));
    }
}
