//! The remote-notice status state on the Session Controller: shown from the initial (cached)
//! value, refreshed from the background channel, and dismissable for the session. No real git /
//! renderer / editor / network, the components are no-op stubs and the update result is injected
//! directly.

mod common;

use common::TempDir;
use herdr_file_viewer::controller::{
    Components, ContentProvider, Controller, EditorHandoff, EditorOutcome, GitService,
    RenderResult, RootProviders,
};
use herdr_file_viewer::git::{Baseline, Status};
use herdr_file_viewer::intent::Intent;
use herdr_file_viewer::update::cache::{self, Cache};
use herdr_file_viewer::update::dismissal::{FileSpotlightDismissalStore, SpotlightDismissalStore};
use herdr_file_viewer::update::spotlight_policy::{
    SpotlightCache, SpotlightInput, cache_delta, project,
};
use herdr_file_viewer::update::{self, NoticeSnapshot, UpdateState, Version};
use herdr_file_viewer::view_policy::ViewMode;
use ratatui::text::Text;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, mpsc};

// ---- minimal no-op stubs (the remote-notice logic exercises none of these) ------------

struct Git;
impl GitService for Git {
    fn status(&self) -> BTreeMap<std::path::PathBuf, Status> {
        BTreeMap::new()
    }
    fn changed_set(&self, _baseline: Baseline) -> BTreeMap<std::path::PathBuf, Status> {
        BTreeMap::new()
    }
    fn diff(&self, _rel: &Path, _baseline: Baseline, _full: bool) -> String {
        String::new()
    }
    fn diff_directory(&self, _rel_dir: &Path, _baseline: Baseline) -> String {
        String::new()
    }
}

struct Content;
impl ContentProvider for Content {
    fn render(&self, _path: &Path, _mode: ViewMode, _raw_diff: Option<&str>) -> RenderResult {
        RenderResult {
            content: Text::raw(""),
            notices: Vec::new(),
            source: None,
        }
    }
}

struct Editor;
impl EditorHandoff for Editor {
    fn open(&mut self, _file: &Path) -> EditorOutcome {
        EditorOutcome::NoTakeover
    }
}

fn controller_in(dir: &Path) -> Controller {
    Controller::new(
        // non-git: keeps the test focused on remote-notice state
        common::resolved(dir.to_path_buf(), false),
        Baseline::Head,
        Components {
            providers: Box::new(move |_resolved| RootProviders {
                git: Arc::new(Git),
                content: Box::new(Content),
            }),
            editor: Box::new(Editor),
            clipboard: Box::new(common::RecordingClipboard::default()),
            renderers: None,
        },
    )
}

fn v(major: u32, minor: u32, patch: u32) -> Version {
    Version {
        major,
        minor,
        patch,
    }
}

fn snapshot(detected_release: Option<Version>, spotlight_title: Option<&str>) -> NoticeSnapshot {
    let mut spotlight = SpotlightCache::default();
    if let Some(title) = spotlight_title {
        spotlight.apply(cache_delta(
            project(SpotlightInput::Available(
                format!("# {title}\nbody\n").into_bytes(),
            )),
            1,
        ));
    }
    NoticeSnapshot {
        detected_release,
        spotlight,
        ..NoticeSnapshot::default()
    }
}

fn snapshot_from_spotlight_input(input: SpotlightInput) -> NoticeSnapshot {
    let mut snapshot = NoticeSnapshot::default();
    snapshot.spotlight.apply(cache_delta(project(input), 1));
    snapshot
}

fn snapshot_from_cache(cache: Cache, session_started_at_unix: u64) -> NoticeSnapshot {
    update::decide(false, session_started_at_unix, &Some(cache)).initial
}

#[test]
fn initial_cached_version_shows_a_remote_notice_status() {
    let dir = TempDir::new();
    let mut c = controller_in(dir.path());
    c.set_update(UpdateState {
        initial: snapshot(Some(v(9, 9, 9)), None),
        rx: None,
    });
    assert!(
        c.view_state()
            .remote_notice_status
            .is_some_and(|b| b.contains("9.9.9")),
        "a cached newer version is advertised on the first frame"
    );
}

#[test]
fn no_update_means_no_remote_notice_status() {
    let dir = TempDir::new();
    let mut c = controller_in(dir.path());
    c.set_update(UpdateState {
        initial: snapshot(None, None),
        rx: None,
    });
    assert!(c.view_state().remote_notice_status.is_none());
}

#[test]
fn background_result_turns_the_remote_notice_status_on() {
    let dir = TempDir::new();
    let (tx, rx) = mpsc::channel();
    let mut c = controller_in(dir.path());
    c.set_update(UpdateState {
        initial: snapshot(None, None),
        rx: Some(rx),
    });
    assert!(
        c.view_state().remote_notice_status.is_none(),
        "nothing until the check returns"
    );

    tx.send(snapshot(Some(v(2, 0, 0)), None)).unwrap();
    let fx = c.poll().expect("poll applies the background update result");
    assert!(fx.redraw, "a fresh verdict triggers a repaint");
    assert!(
        c.view_state()
            .remote_notice_status
            .is_some_and(|status| status.contains("2.0.0")),
        "the remote-notice status now names the version the check found"
    );
}

#[test]
fn background_up_to_date_clears_a_stale_cached_remote_notice_status() {
    // A cached status, then a successful check that finds nothing newer (`None`) removes it.
    let dir = TempDir::new();
    let (tx, rx) = mpsc::channel();
    let mut c = controller_in(dir.path());
    c.set_update(UpdateState {
        initial: snapshot(Some(v(9, 9, 9)), None),
        rx: Some(rx),
    });
    assert!(c.view_state().remote_notice_status.is_some());

    tx.send(snapshot(None, None)).unwrap();
    c.poll().expect("poll applies the result");
    assert!(
        c.view_state().remote_notice_status.is_none(),
        "a successful 'up-to-date' check clears the stale cached remote-notice status"
    );
}

#[test]
fn background_notice_snapshot_replaces_release_and_spotlight_together() {
    let dir = TempDir::new();
    let (tx, rx) = mpsc::channel();
    let mut c = controller_in(dir.path());
    c.set_update(UpdateState {
        initial: snapshot(Some(v(9, 9, 9)), Some("Before")),
        rx: Some(rx),
    });

    tx.send(snapshot(Some(v(2, 0, 0)), Some("After"))).unwrap();
    c.poll().expect("the completed snapshot redraws");

    assert!(
        c.view_state()
            .remote_notice_status
            .is_some_and(|status| status.contains("2.0.0")),
        "the new snapshot replaces the detected release"
    );
    assert_eq!(
        c.notice_snapshot().spotlight.status_title(),
        Some("After"),
        "the same channel message replaces the spotlight, not only the release"
    );
}

#[test]
fn disconnected_notice_channel_preserves_last_snapshot() {
    let dir = TempDir::new();
    let (tx, rx) = mpsc::channel();
    let mut c = controller_in(dir.path());
    c.set_update(UpdateState {
        initial: snapshot(Some(v(9, 9, 9)), Some("Cached")),
        rx: Some(rx),
    });

    drop(tx);
    c.poll();

    assert!(
        c.view_state()
            .remote_notice_status
            .is_some_and(|status| status.contains("9.9.9")),
        "a disconnected probe keeps the last detected release"
    );
    assert_eq!(
        c.notice_snapshot().spotlight.status_title(),
        Some("Cached"),
        "a disconnected probe keeps the last spotlight in the same snapshot"
    );
}

#[test]
fn dismiss_hides_the_remote_notice_status_for_the_session() {
    let dir = TempDir::new();
    let mut c = controller_in(dir.path());
    c.set_update(UpdateState {
        initial: snapshot(Some(v(9, 9, 9)), Some("Project")),
        rx: None,
    });
    let fx = c.handle(Intent::DismissUpdate);
    assert!(
        fx.redraw,
        "dismissing repaints to remove the remote-notice status"
    );
    assert!(
        c.view_state().remote_notice_status.is_none(),
        "session dismissal hides every notice form at once"
    );
    // Dismiss again is inert (no remote-notice status to hide).
    assert!(!c.handle(Intent::DismissUpdate).redraw);
}

#[test]
fn dismisses_spotlight_only_update_only_and_combined_statuses_without_losing_spotlight_body() {
    let cases = [
        (
            "spotlight",
            snapshot(None, Some("Project")),
            Some(b"body\n".as_slice()),
        ),
        ("update", snapshot(Some(v(9, 9, 9)), None), None),
        (
            "combined",
            snapshot(Some(v(9, 9, 9)), Some("Project")),
            Some(b"body\n".as_slice()),
        ),
    ];

    for (name, initial, expected_body) in cases {
        let dir = TempDir::new();
        let mut controller = controller_in(dir.path());
        controller.set_update(UpdateState { initial, rx: None });

        assert!(
            controller.handle(Intent::DismissUpdate).redraw,
            "{name}: a visible remote-notice line dismisses immediately"
        );
        assert_eq!(
            controller.view_state().remote_notice_status,
            None,
            "{name}: the current process hides the complete line"
        );
        assert_eq!(
            controller.notice_snapshot().spotlight.whats_new_body(),
            expected_body,
            "{name}: footer dismissal never discards the accepted What's New body"
        );
    }
}

#[test]
fn refresh_cannot_clear_a_remote_notice_session_dismissal() {
    let dir = TempDir::new();
    let (tx, rx) = mpsc::channel();
    let mut controller = controller_in(dir.path());
    controller.set_update(UpdateState {
        initial: snapshot(Some(v(9, 9, 9)), Some("Before")),
        rx: Some(rx),
    });

    assert!(controller.handle(Intent::DismissUpdate).redraw);
    tx.send(snapshot(Some(v(10, 0, 0)), Some("After")))
        .expect("send refreshed snapshot");
    assert!(controller.poll().expect("apply refreshed snapshot").redraw);
    assert_eq!(
        controller.view_state().remote_notice_status,
        None,
        "a replacement, even with a new spotlight identity, cannot revive a session dismissal"
    );
    assert_eq!(
        controller.notice_snapshot().spotlight.whats_new_body(),
        Some(b"body\n".as_slice()),
        "the replacement still retains its accepted body for What's New"
    );
}

#[test]
fn update_dismissal_is_session_only_and_never_persists() {
    let dir = TempDir::new();
    let initial = snapshot(Some(v(9, 9, 9)), None);
    let mut controller = controller_in(dir.path());
    controller.set_update(UpdateState { initial, rx: None });

    assert!(controller.handle(Intent::DismissUpdate).redraw);
    drop(controller);
    assert_eq!(
        cache::load(dir.path()),
        None,
        "update-only dismissal queues no cache mutation"
    );

    let mut next_controller = controller_in(dir.path());
    next_controller.set_update(UpdateState {
        initial: snapshot(Some(v(9, 9, 9)), None),
        rx: None,
    });
    assert!(
        next_controller.view_state().remote_notice_status.is_some(),
        "an update remains visible to the next controller"
    );
}

#[test]
// Without the app-injected store, the controller remains hermetic and all persistence is absent.
// Keep the original session-only assertions for this unwired boundary, not the live application.
fn unwired_controller_keeps_dismissal_ephemeral_and_never_touches_disk() {
    let session_started_at_unix = 10_000;
    let spotlight = b"# Project\nbody\n".to_vec();
    let cases = [
        (
            "update-only",
            Cache {
                latest_seen: Some("9.9.9".into()),
                ..Cache::default()
            },
            None,
        ),
        (
            "spotlight-only",
            Cache {
                spotlight: Some(spotlight.clone()),
                spotlight_retrieved_at_unix: Some(session_started_at_unix - 1),
                ..Cache::default()
            },
            Some(b"body\n".as_slice()),
        ),
        (
            "combined",
            Cache {
                latest_seen: Some("9.9.9".into()),
                spotlight: Some(spotlight.clone()),
                spotlight_retrieved_at_unix: Some(session_started_at_unix - 1),
                ..Cache::default()
            },
            Some(b"body\n".as_slice()),
        ),
    ];

    for (name, persisted, expected_body) in cases {
        let dir = TempDir::new();
        cache::store(dir.path(), &persisted);
        let initial = snapshot_from_cache(persisted.clone(), session_started_at_unix);
        let mut controller = controller_in(dir.path());
        controller.set_update(UpdateState { initial, rx: None });

        assert!(
            controller.view_state().remote_notice_status.is_some(),
            "{name}: precondition"
        );
        assert!(
            controller.handle(Intent::DismissUpdate).redraw,
            "{name}: u hides the row"
        );
        assert_eq!(
            controller.view_state().remote_notice_status,
            None,
            "{name}: hidden now"
        );
        assert_eq!(
            controller.notice_snapshot().spotlight.whats_new_body(),
            expected_body,
            "{name}: dismissal does not discard What's New content"
        );
        drop(controller);

        assert_eq!(
            cache::load(dir.path()),
            Some(persisted),
            "{name}: dismissal must not write an advisory identity"
        );
        assert!(
            !std::fs::read_to_string(dir.path().join("update-check.json"))
                .expect("the seed cache remains readable")
                .contains("dismissed_spotlight_identity"),
            "{name}: the cache schema contains no dismissal identity"
        );

        let mut fresh = controller_in(dir.path());
        fresh.set_update(UpdateState {
            initial: snapshot_from_cache(
                cache::load(dir.path()).expect("the unchanged cache is reusable"),
                session_started_at_unix,
            ),
            rx: None,
        });
        assert!(
            fresh.view_state().remote_notice_status.is_some(),
            "{name}: the same still-relevant row returns in a fresh session"
        );
    }
}

fn persistent_controller(root: &Path, cache_dir: &Path) -> Controller {
    let mut controller = controller_in(root);
    controller.set_spotlight_dismissal_store(Box::new(FileSpotlightDismissalStore::new(
        cache_dir.to_path_buf(),
    )));
    controller
}

#[test]
fn persisted_spotlight_dismissal_survives_relaunch_without_hiding_release_notices_or_details() {
    let now = 10_000;
    for (name, release, spotlight, expected_status) in [
        (
            "update-only",
            Some("9.9.9"),
            None,
            Some("Update v9.9.9 available · ? details · u dismiss"),
        ),
        (
            "spotlight-only",
            None,
            Some(b"# Project\nbody\n".to_vec()),
            None,
        ),
        (
            "combined",
            Some("9.9.9"),
            Some(b"# Project\nbody\n".to_vec()),
            Some("Update v9.9.9 available · ? details · u dismiss"),
        ),
    ] {
        let root = TempDir::new();
        let dir = TempDir::new();
        let persisted = Cache {
            last_check_unix: now,
            latest_seen: release.map(str::to_owned),
            spotlight: spotlight.clone(),
            spotlight_retrieved_at_unix: spotlight.as_ref().map(|_| now),
            ..Cache::default()
        };
        cache::store(dir.path(), &persisted);
        let mut first = persistent_controller(root.path(), dir.path());
        first.set_update(UpdateState {
            initial: snapshot_from_cache(persisted.clone(), now),
            rx: None,
        });
        assert!(
            first.view_state().remote_notice_status.is_some(),
            "{name}: precondition"
        );
        assert!(first.handle(Intent::DismissUpdate).redraw);
        assert_eq!(first.view_state().remote_notice_status, None);
        assert!(
            !first.handle(Intent::DismissUpdate).redraw,
            "repeat dismissal is inert"
        );
        drop(first);

        assert_eq!(
            cache::load(dir.path()),
            Some(persisted.clone()),
            "{name}: refresh cache is unchanged"
        );
        assert_eq!(
            FileSpotlightDismissalStore::new(dir.path().to_path_buf())
                .load()
                .is_some(),
            spotlight.is_some(),
            "{name}: only a spotlight creates a persistent record"
        );
        assert_eq!(
            std::fs::read_dir(root.path()).unwrap().count(),
            0,
            "the viewed root stays read-only"
        );

        let mut next = persistent_controller(root.path(), dir.path());
        next.set_update(UpdateState {
            initial: snapshot_from_cache(cache::load(dir.path()).unwrap(), now),
            rx: None,
        });
        assert_eq!(
            next.view_state().remote_notice_status.as_deref(),
            expected_status,
            "{name}: relaunch"
        );
        if spotlight.is_some() {
            assert_eq!(
                next.notice_snapshot().spotlight.whats_new_body(),
                Some(b"body\n".as_slice())
            );
            next.handle(Intent::ShowHelp);
            let body = next
                .help_state()
                .unwrap()
                .active_body()
                .lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .map(|span| span.content.as_ref())
                .collect::<String>();
            assert!(
                body.contains("body"),
                "{name}: dismissed details remain in What's New"
            );
        }
        // The remote coordinator may publish another complete snapshot, but never this record.
        cache::store(
            dir.path(),
            &Cache {
                last_check_unix: now + 1,
                ..persisted
            },
        );
        assert_eq!(
            FileSpotlightDismissalStore::new(dir.path().to_path_buf())
                .load()
                .is_some(),
            spotlight.is_some(),
            "{name}: refresh publication cannot erase a dismissal"
        );
    }
}

#[test]
fn persisted_dismissal_filters_identical_background_refreshes_but_not_changed_spotlights() {
    let root = TempDir::new();
    let dir = TempDir::new();
    let original = snapshot(None, Some("Project"));
    let mut first = persistent_controller(root.path(), dir.path());
    first.set_update(UpdateState {
        initial: original.clone(),
        rx: None,
    });
    assert!(first.handle(Intent::DismissUpdate).redraw);
    drop(first);

    let (tx, rx) = mpsc::channel();
    let mut next = persistent_controller(root.path(), dir.path());
    // A stale startup cache may initially supply no spotlight. The dismissal must still filter
    // the later fresh remote result; sending before poll forces this path without a sleep.
    next.set_update(UpdateState {
        initial: NoticeSnapshot::default(),
        rx: Some(rx),
    });
    tx.send(original).unwrap();
    assert!(next.poll().unwrap().redraw);
    assert_eq!(
        next.view_state().remote_notice_status,
        None,
        "identical fresh result stays dismissed"
    );

    for (document, visible) in [
        ("\n\n# Project\nbody\n", false),
        ("# Renamed\nbody\n", true),
        ("# Project\nchanged body\n", true),
    ] {
        let (tx, rx) = mpsc::channel();
        next.set_update(UpdateState {
            initial: NoticeSnapshot::default(),
            rx: Some(rx),
        });
        tx.send(snapshot_from_spotlight_input(SpotlightInput::Available(
            document.as_bytes().to_vec(),
        )))
        .unwrap();
        assert!(next.poll().unwrap().redraw);
        assert_eq!(
            next.view_state().remote_notice_status.is_some(),
            visible,
            "{document:?}"
        );
    }

    std::fs::remove_file(dir.path().join("spotlight-dismissal.json")).unwrap();
    let mut restored = persistent_controller(root.path(), dir.path());
    restored.set_update(UpdateState {
        initial: snapshot(None, Some("Project")),
        rx: None,
    });
    assert!(
        restored.view_state().remote_notice_status.is_some(),
        "deleting the advisory record resets dismissal"
    );
}

#[test]
fn failed_spotlight_persistence_still_dismisses_the_current_session() {
    let root = TempDir::new();
    let dir = TempDir::new();
    std::fs::create_dir(dir.path().join("spotlight-dismissal.json")).unwrap();
    let mut first = persistent_controller(root.path(), dir.path());
    first.set_update(UpdateState {
        initial: snapshot(None, Some("Project")),
        rx: None,
    });
    assert!(first.handle(Intent::DismissUpdate).redraw);
    assert_eq!(first.view_state().remote_notice_status, None);
    drop(first);

    let mut next = persistent_controller(root.path(), dir.path());
    next.set_update(UpdateState {
        initial: snapshot(None, Some("Project")),
        rx: None,
    });
    assert!(
        next.view_state().remote_notice_status.is_some(),
        "a failed save stays fail-silent and session-only"
    );
}

#[test]
fn withdrawal_freshness_and_replacement_remain_independent_of_a_prior_session_dismissal() {
    let session_started_at_unix = 100_000;
    let original = b"# Project\nold body\n".to_vec();
    let dir = TempDir::new();
    let persisted = Cache {
        spotlight: Some(original),
        spotlight_retrieved_at_unix: Some(session_started_at_unix - 1),
        ..Cache::default()
    };
    cache::store(dir.path(), &persisted);

    let initial = snapshot_from_cache(persisted.clone(), session_started_at_unix);
    let mut dismissed = controller_in(dir.path());
    dismissed.set_update(UpdateState { initial, rx: None });
    assert!(
        dismissed.handle(Intent::DismissUpdate).redraw,
        "u dismisses this session"
    );
    drop(dismissed);

    let withdrawn = Cache {
        spotlight: None,
        spotlight_retrieved_at_unix: Some(session_started_at_unix - 1),
        ..persisted.clone()
    };
    cache::store(dir.path(), &withdrawn);
    let fresh_withdrawal = snapshot_from_cache(
        cache::load(dir.path()).expect("complete withdrawal snapshot is reusable"),
        session_started_at_unix,
    );
    assert!(fresh_withdrawal.spotlight.status_title().is_none());
    assert!(fresh_withdrawal.spotlight.whats_new_body().is_none());
    assert!(
        !herdr_file_viewer::update::spotlight_policy::should_retrieve(
            session_started_at_unix,
            &fresh_withdrawal.spotlight
        ),
        "a fresh withdrawal remains fresh regardless of dismissal"
    );

    let stale_withdrawal = snapshot_from_cache(
        Cache {
            spotlight: None,
            spotlight_retrieved_at_unix: Some(session_started_at_unix - 24 * 60 * 60),
            ..withdrawn
        },
        session_started_at_unix,
    );
    assert!(
        herdr_file_viewer::update::spotlight_policy::should_retrieve(
            session_started_at_unix,
            &stale_withdrawal.spotlight
        ),
        "a stale withdrawal retries regardless of dismissal"
    );

    let replacement = Cache {
        spotlight: Some(b"# Project\nnew body\n".to_vec()),
        spotlight_retrieved_at_unix: Some(session_started_at_unix),
        ..persisted
    };
    cache::store(dir.path(), &replacement);
    let next = snapshot_from_cache(
        cache::load(dir.path()).expect("complete replacement snapshot is reusable"),
        session_started_at_unix,
    );
    assert_eq!(next.spotlight.status_title(), Some("Project"));
    assert_eq!(
        next.spotlight.whats_new_body(),
        Some(b"new body\n".as_slice())
    );
}

#[test]
fn controller_projects_typed_notice_status_forms_with_default_labels() {
    let update = v(9, 9, 9);
    let cases = [
        (
            "update",
            snapshot(Some(update), None),
            Some("Update v9.9.9 available · ? details · u dismiss"),
        ),
        (
            "accepted spotlight safe title",
            snapshot(
                None,
                Some("Project \x1b]8;;https://evil.invalid/\x1b\\Meteor"),
            ),
            Some("Spotlight: Project Meteor · ? details · u dismiss"),
        ),
        (
            "combined",
            snapshot(Some(update), Some("Project")),
            Some("Update v9.9.9 available · Spotlight: Project · ? details · u dismiss"),
        ),
        ("empty", snapshot(None, None), None),
    ];

    for (name, initial, expected) in cases {
        let dir = TempDir::new();
        let mut controller = controller_in(dir.path());
        controller.set_update(UpdateState { initial, rx: None });
        let actual = controller.view_state().remote_notice_status;
        assert_eq!(actual.as_deref(), expected, "{name}");
        assert!(
            !actual.as_deref().is_some_and(
                |line| line.contains("herdr plugin install") || line.contains("install")
            ),
            "{name}: status must not advertise an install or automatic action: {actual:?}"
        );
    }
}

#[test]
fn disabled_and_invalid_spotlights_project_no_status_line() {
    let invalid = [
        ("missing", SpotlightInput::Missing),
        ("empty", SpotlightInput::Available(Vec::new())),
        ("blank", SpotlightInput::Available(b" \r\n\t".to_vec())),
        (
            "non-utf8",
            SpotlightInput::Available(vec![b'#', b' ', 0xff]),
        ),
        (
            "headingless",
            SpotlightInput::Available(b"ordinary body without a heading\n".to_vec()),
        ),
        (
            "empty title",
            SpotlightInput::Available(b"# \r\nbody\n".to_vec()),
        ),
        ("unavailable", SpotlightInput::Unavailable),
    ];

    let dir = TempDir::new();
    let mut disabled = controller_in(dir.path());
    disabled.set_update(UpdateState::disabled());
    assert_eq!(disabled.view_state().remote_notice_status, None, "disabled");

    for (name, input) in invalid {
        let dir = TempDir::new();
        let mut controller = controller_in(dir.path());
        controller.set_update(UpdateState {
            initial: snapshot_from_spotlight_input(input),
            rx: None,
        });
        assert_eq!(
            controller.view_state().remote_notice_status,
            None,
            "{name}: only an accepted typed spotlight can reach the status line"
        );
    }
}
