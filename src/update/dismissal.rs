//! Best-effort persistence of the last explicitly dismissed spotlight, independent of refreshes.
//!
//! Identity is the accepted display title plus byte-exact body, not a retrieval time, commit,
//! installed application, or release. This record only filters status; What's New keeps its data.

use super::cache;
use super::spotlight_policy::{MAX_BODY_BYTES, MAX_TITLE_CHARS, SpotlightCache};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const FILE_NAME: &str = "spotlight-dismissal.json";
const SCHEMA_VERSION: u8 = 1;
// JSON byte arrays need at most four bytes per byte; title escapes need at most six per character.
const MAX_ENCODED_BYTES: usize = MAX_BODY_BYTES * 4 + MAX_TITLE_CHARS * 6 + 128;

/// The identity of one explicitly dismissed, accepted spotlight.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DismissedSpotlight {
    title: String,
    body: Vec<u8>,
}

impl DismissedSpotlight {
    pub fn from_spotlight(spotlight: &SpotlightCache) -> Option<Self> {
        Some(Self {
            title: spotlight.status_title()?.to_owned(),
            body: spotlight.whats_new_body()?.to_vec(),
        })
    }

    /// Compare without cloning per frame. Leading blank metadata and retrieval times are irrelevant.
    pub fn matches(&self, spotlight: &SpotlightCache) -> bool {
        spotlight.status_title() == Some(self.title.as_str())
            && spotlight.whats_new_body() == Some(self.body.as_slice())
    }

    fn is_valid(&self) -> bool {
        !self.title.is_empty()
            && self.title.chars().count() <= MAX_TITLE_CHARS
            && self.body.len() <= MAX_BODY_BYTES
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema_version: u8,
    spotlight: DismissedSpotlight,
}

/// Injected persistence boundary. An unwired controller never accesses the real user's cache.
pub trait SpotlightDismissalStore {
    fn load(&self) -> Option<DismissedSpotlight>;
    fn save(&self, spotlight: &DismissedSpotlight);
}

/// A bounded, atomic, safe-to-delete record beside the remote-notice cache.
pub struct FileSpotlightDismissalStore {
    dir: PathBuf,
}

impl FileSpotlightDismissalStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
}

impl SpotlightDismissalStore for FileSpotlightDismissalStore {
    fn load(&self) -> Option<DismissedSpotlight> {
        let raw = cache::read_bounded(&self.dir, FILE_NAME, MAX_ENCODED_BYTES)?;
        let record: Record = serde_json::from_slice(&raw).ok()?;
        (record.schema_version == SCHEMA_VERSION && record.spotlight.is_valid())
            .then_some(record.spotlight)
    }

    fn save(&self, spotlight: &DismissedSpotlight) {
        if !spotlight.is_valid() {
            return;
        }
        let record = Record {
            schema_version: SCHEMA_VERSION,
            spotlight: spotlight.clone(),
        };
        let Ok(raw) = serde_json::to_vec(&record) else {
            return;
        };
        if raw.len() <= MAX_ENCODED_BYTES {
            cache::publish(&self.dir, FILE_NAME, &raw);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::spotlight_policy::{SpotlightInput, cache_delta, project};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            Self(std::env::temp_dir().join(format!(
                "hfv-dismissal-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )))
        }

        fn store(&self) -> FileSpotlightDismissalStore {
            FileSpotlightDismissalStore::new(self.0.clone())
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn spotlight(document: &str, timestamp: u64) -> SpotlightCache {
        let mut spotlight = SpotlightCache::default();
        spotlight.apply(cache_delta(
            project(SpotlightInput::Available(document.as_bytes().to_vec())),
            timestamp,
        ));
        spotlight
    }

    #[test]
    fn identity_tracks_accepted_title_and_exact_body_not_retrieval_metadata() {
        let original = spotlight("# Project\nbody\n", 1);
        let dismissed = DismissedSpotlight::from_spotlight(&original).unwrap();
        assert!(dismissed.matches(&original));
        assert!(dismissed.matches(&spotlight("\n\n# Project\nbody\n", 2)));
        assert!(!dismissed.matches(&spotlight("# Other\nbody\n", 1)));
        assert!(!dismissed.matches(&spotlight("# Project\nchanged body\n", 1)));
        assert!(!dismissed.matches(&SpotlightCache::default()));
        assert!(DismissedSpotlight::from_spotlight(&SpotlightCache::default()).is_none());
    }

    #[test]
    fn round_trip_keeps_exact_bytes_and_only_the_last_dismissal() {
        let dir = TempDir::new();
        let store = dir.store();
        assert_eq!(store.load(), None);
        let first = DismissedSpotlight {
            title: "Project 🚀".into(),
            body: (0..=255).collect(),
        };
        store.save(&first);
        assert_eq!(store.load(), Some(first));
        let second = DismissedSpotlight::from_spotlight(&spotlight("# Other\nbody\n", 2)).unwrap();
        store.save(&second);
        assert_eq!(dir.store().load(), Some(second));
        std::fs::remove_file(dir.0.join(FILE_NAME)).unwrap();
        assert_eq!(store.load(), None);
    }

    #[test]
    fn absent_corrupt_unknown_and_oversized_records_are_ignored() {
        let dir = TempDir::new();
        let store = dir.store();
        std::fs::create_dir_all(&dir.0).unwrap();
        for raw in [
            "not json",
            r#"{"schema_version":2,"spotlight":{"title":"Project","body":[]}}"#,
            r#"{"schema_version":1,"spotlight":{"title":"","body":[]}}"#,
            r#"{"schema_version":1,"spotlight":{"title":"Project","body":[]},"extra":true}"#,
        ] {
            std::fs::write(dir.0.join(FILE_NAME), raw).unwrap();
            assert_eq!(store.load(), None, "{raw}");
        }

        let valid = DismissedSpotlight::from_spotlight(&spotlight("# Project\nbody\n", 1)).unwrap();
        store.save(&valid);
        let mut raw = std::fs::read(dir.0.join(FILE_NAME)).unwrap();
        raw.resize(MAX_ENCODED_BYTES, b' ');
        std::fs::write(dir.0.join(FILE_NAME), &raw).unwrap();
        assert_eq!(store.load(), Some(valid));
        raw.push(b' ');
        std::fs::write(dir.0.join(FILE_NAME), raw).unwrap();
        assert_eq!(
            store.load(),
            None,
            "cap plus one must be rejected before parsing"
        );
    }

    #[test]
    fn semantic_bounds_apply_on_read_and_write_and_failures_are_silent() {
        let dir = TempDir::new();
        let store = dir.store();
        let valid = DismissedSpotlight::from_spotlight(&spotlight("# Project\nbody\n", 1)).unwrap();
        for invalid in [
            DismissedSpotlight {
                title: "x".repeat(MAX_TITLE_CHARS + 1),
                ..valid.clone()
            },
            DismissedSpotlight {
                body: vec![0; MAX_BODY_BYTES + 1],
                ..valid.clone()
            },
        ] {
            store.save(&invalid);
            assert!(!dir.0.exists(), "invalid records never create a directory");
            std::fs::create_dir_all(&dir.0).unwrap();
            let raw = serde_json::to_vec(&Record {
                schema_version: SCHEMA_VERSION,
                spotlight: invalid,
            })
            .unwrap();
            std::fs::write(dir.0.join(FILE_NAME), raw).unwrap();
            assert_eq!(
                store.load(),
                None,
                "a bounded but invalid record is ignored"
            );
            std::fs::remove_dir_all(&dir.0).unwrap();
        }
        std::fs::create_dir_all(dir.0.join(FILE_NAME)).unwrap();
        store.save(&valid);
        assert!(
            dir.0.join(FILE_NAME).is_dir(),
            "failed publication leaves the destination alone"
        );
        assert_eq!(store.load(), None);
    }

    #[test]
    fn maximum_identity_fits_the_encoded_cap() {
        let dir = TempDir::new();
        let identity = DismissedSpotlight {
            title: "\\".repeat(MAX_TITLE_CHARS),
            body: vec![255; MAX_BODY_BYTES],
        };
        dir.store().save(&identity);
        assert_eq!(dir.store().load(), Some(identity));
    }
}
