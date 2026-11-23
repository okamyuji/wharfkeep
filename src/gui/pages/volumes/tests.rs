use super::super::test_util::{harness, requests, state_on_prod, state_with};
use super::*;
use crate::gui::bridge::Reply;
use crate::gui::state::Page;
use crate::snapshot::DockerSummary;
use egui_kittest::kittest::{NodeT, Queryable};

fn volume(name: &str, size: Option<u64>, prunable: bool) -> VolumeRow {
    VolumeRow {
        name: name.into(),
        driver: "local".into(),
        mountpoint: format!("/var/lib/docker/volumes/{name}/_data"),
        size_bytes: size,
        prunable,
    }
}

fn volumes_state(volumes: Vec<VolumeRow>) -> GuiState {
    state_with(
        DockerSummary {
            volumes,
            ..Default::default()
        },
        Page::Volumes,
    )
}

#[test]
fn volume_row_delete_confirms() {
    let (mut h, mut ends) = harness(
        Lang::En,
        volumes_state(vec![volume("data-1", Some(5), false)]),
    );
    h.get_by_label("Delete data-1").click();
    h.run_steps(4);
    assert_eq!(
        h.state().0.confirm.clone().unwrap().message,
        "Delete volume data-1?"
    );
    assert!(requests(&mut ends).is_empty());
    h.get_by_label("Delete").click();
    h.run_steps(4);
    assert_eq!(
        requests(&mut ends),
        [Request::RemoveVolume {
            profile: "dev".into(),
            name: "data-1".into(),
            size: Some(5)
        }]
    );
    assert!(h.state().0.busy_ops.contains("volume:data-1"));
}

#[test]
fn a_volume_in_use_shows_dockers_reason_in_a_toast() {
    let mut state = volumes_state(vec![volume("data-1", Some(5), false)]);
    state.busy_ops.insert("volume:data-1".into());
    state.apply_reply(Reply::OpDone {
        key: "volume:data-1".into(),
        error: Some("volume is in use".into()),
    });
    assert!(!state.busy_ops.contains("volume:data-1"));
    assert_eq!(state.toasts.len(), 1);
    assert!(state.toasts[0].message.contains("volume is in use"));
    assert!(!state.toasts[0].info);
}

#[test]
fn cancelling_the_volume_confirmation_sends_nothing() {
    let (mut h, mut ends) = harness(
        Lang::En,
        volumes_state(vec![volume("data-1", Some(5), false)]),
    );
    h.get_by_label("Delete data-1").click();
    h.run_steps(4);
    h.get_by_label("Cancel").click();
    h.run_steps(4);
    assert!(requests(&mut ends).is_empty());
}

#[test]
fn prunable_summary_counts_only_prunable_volumes_and_sums_known_sizes() {
    let rows = [
        volume("a1", Some(10), true),
        volume("b1", None, true),
        volume("c1", Some(100), false),
        volume("d1", Some(5), true),
    ];
    assert_eq!(prunable_summary(&rows), (3, 15));
    assert_eq!(prunable_summary(&[]), (0, 0));
}

#[test]
fn volume_cleanup_shows_count_and_size() {
    let rows = vec![
        volume("a1", Some(1 << 20), true),
        volume("b1", None, true),
        volume("keep", Some(9 << 30), false),
    ];
    let (mut h, mut ends) = harness(Lang::En, volumes_state(rows));
    h.get_by_label("Clean up…").click();
    h.run_steps(4);
    assert_eq!(
        h.state().0.confirm.clone().unwrap().message,
        "Delete 2 unused anonymous volumes (1.0 MiB)?"
    );
    h.get_by_label("Delete").click();
    h.run_steps(4);
    assert_eq!(
        requests(&mut ends),
        [Request::PruneVolumes {
            profile: "dev".into()
        }]
    );
    assert!(h.state().0.busy_ops.contains("prune-volumes:dev"));
}

#[test]
fn volume_cleanup_is_disabled_without_prunable_volumes_or_while_running() {
    let (h, _ends) = harness(
        Lang::En,
        volumes_state(vec![volume("keep", Some(1), false)]),
    );
    assert!(h.get_by_label("Clean up…").accesskit_node().is_disabled());
    let mut state = volumes_state(vec![volume("a1", Some(1), true)]);
    state.busy_ops.insert("prune-volumes:dev".into());
    let (h, _ends) = harness(Lang::En, state);
    assert!(h.get_by_label("Clean up…").accesskit_node().is_disabled());
    let (h, _ends) = harness(Lang::En, volumes_state(vec![volume("a1", Some(1), true)]));
    assert!(!h.get_by_label("Clean up…").accesskit_node().is_disabled());
}

#[test]
fn unknown_volume_size_shows_fumei() {
    for (lang, want) in [(Lang::Ja, "不明"), (Lang::En, "unknown")] {
        let (h, _ends) = harness(lang, volumes_state(vec![volume("data-1", None, false)]));
        h.get_by_label(want);
    }
    let (h, _ends) = harness(
        Lang::En,
        volumes_state(vec![volume("data-1", Some(2048), false)]),
    );
    h.get_by_label("2.0 KiB");
    assert!(h.query_by_label("unknown").is_none());
}

#[test]
fn rows_show_driver_and_mountpoint() {
    let (h, _ends) = harness(
        Lang::En,
        volumes_state(vec![volume("data-1", Some(5), false)]),
    );
    h.get_by_label("local");
    h.get_by_label("/var/lib/docker/volumes/data-1/_data");
}

#[test]
fn empty_list_says_so() {
    let (h, _ends) = harness(Lang::En, volumes_state(vec![]));
    h.get_by_label("No volumes");
}

#[test]
fn create_form_sends_the_name() {
    let (mut h, mut ends) = harness(Lang::En, volumes_state(vec![]));
    assert!(h.get_by_label("Create").accesskit_node().is_disabled());
    h.state_mut().0.volumes.name = "data-2".into();
    h.run_steps(4);
    h.get_by_label("Create").click();
    h.run_steps(4);
    assert_eq!(
        requests(&mut ends),
        [Request::CreateVolume {
            profile: "dev".into(),
            name: "data-2".into()
        }]
    );
    assert!(h.state().0.busy_ops.contains("create-volume:dev"));
    assert!(h.get_by_label("Create").accesskit_node().is_disabled());
}

#[test]
fn create_form_explains_a_bad_name() {
    let (mut h, mut ends) = harness(Lang::En, volumes_state(vec![]));
    h.state_mut().0.volumes.name = "a/b".into();
    h.run_steps(4);
    assert!(h.get_by_label("Create").accesskit_node().is_disabled());
    h.get_by_label_contains("invalid name, ID or image reference");
    assert!(requests(&mut ends).is_empty());
}

#[test]
fn create_form_takes_a_255_character_name_and_refuses_256() {
    let (mut h, mut ends) = harness(Lang::En, volumes_state(vec![]));
    h.state_mut().0.volumes.name = "v".repeat(255);
    h.run_steps(4);
    assert!(!h.get_by_label("Create").accesskit_node().is_disabled());
    assert!(h.query_by_label_contains("too long").is_none());
    h.state_mut().0.volumes.name = "v".repeat(256);
    h.run_steps(4);
    assert!(h.get_by_label("Create").accesskit_node().is_disabled());
    h.get_by_label_contains("Volume name is too long (255 characters at most)");
    assert!(requests(&mut ends).is_empty());
}

#[test]
fn volume_inspect_asks_by_name() {
    let (mut h, mut ends) = harness(
        Lang::En,
        volumes_state(vec![volume("data-1", Some(5), false)]),
    );
    h.get_by_label("Inspect data-1").click();
    h.run_steps(4);
    assert_eq!(
        requests(&mut ends),
        [Request::InspectResource {
            profile: "dev".into(),
            kind: crate::gui::bridge::ResourceKind::Volume,
            target: "data-1".into()
        }]
    );
}

#[test]
fn volume_requests_go_to_the_selected_instance() {
    let summary = DockerSummary {
        volumes: vec![volume("data-1", Some(5), true)],
        ..Default::default()
    };
    let (mut h, mut ends) = harness(Lang::En, state_on_prod(summary, Page::Volumes));
    h.get_by_label("Delete data-1").click();
    h.run_steps(4);
    h.get_by_label("Delete").click();
    h.run_steps(4);
    h.get_by_label("Clean up…").click();
    h.run_steps(4);
    h.get_by_label("Delete").click();
    h.run_steps(4);
    assert_eq!(
        requests(&mut ends),
        [
            Request::RemoveVolume {
                profile: "prod".into(),
                name: "data-1".into(),
                size: Some(5)
            },
            Request::PruneVolumes {
                profile: "prod".into()
            }
        ]
    );
}
