use super::super::test_util::{harness, requests, row, state_on_prod, state_with};
use super::*;
use crate::gui::bridge::{Reply, ResourceKind, WorkerEnds};
use crate::gui::state::Page;
use crate::snapshot::DiskUsage;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

fn image(id: &str, tags: &[&str], size: u64) -> ImageRow {
    ImageRow {
        id: id.into(),
        repo_tags: tags.iter().map(|t| t.to_string()).collect(),
        size_bytes: size,
        created: 1_000,
    }
}

fn images_state(
    images: Vec<ImageRow>,
    containers: Vec<ContainerRow>,
    build_cache: Option<u64>,
) -> GuiState {
    state_with(
        DockerSummary {
            images,
            containers,
            disk: DiskUsage {
                build_cache,
                ..Default::default()
            },
            ..Default::default()
        },
        Page::Images,
    )
}

type H = Harness<'static, crate::gui::pages::test_util::Fixture>;

fn open(lang: Lang, state: GuiState) -> (H, WorkerEnds) {
    harness(lang, state)
}

fn click(h: &mut H, label: &str) {
    h.get_by_label(label).click();
    h.run_steps(4);
}

#[test]
fn rows_are_one_per_tag_and_untagged_images_show_their_id() {
    let rows = [
        image(
            "sha256:aaaaaaaaaaaabbbbbbbb",
            &["nginx:alpine", "web:1"],
            10,
        ),
        image("sha256:cccccccccccc99999999", &[], 20),
        image("sha256:dddddddddddd11111111", &["<none>:<none>"], 30),
    ];
    let list = entries(&rows);
    let names: Vec<_> = list
        .iter()
        .map(|e| (e.name.as_str(), e.target.as_str()))
        .collect();
    assert_eq!(
        names,
        [
            ("nginx:alpine", "nginx:alpine"),
            ("web:1", "web:1"),
            ("<none> cccccccccccc", "sha256:cccccccccccc99999999"),
            ("<none> dddddddddddd", "sha256:dddddddddddd11111111"),
        ]
    );
}

#[test]
fn image_in_use_compares_the_image_id_first() {
    let rows = [
        image("sha256:aaaaaaaaaaaa1111", &["alpine:latest"], 1),
        image("sha256:bbbbbbbbbbbb2222", &["nginx:latest"], 1),
    ];
    let used = |reference: &str, image_id: &str| {
        let containers = [ContainerRow {
            image: reference.into(),
            image_id: image_id.into(),
            ..row("c1", "c", "running", false)
        }];
        entries(&rows)
            .iter()
            .map(|e| in_use(e, &containers))
            .collect::<Vec<_>>()
    };
    // 参照の書き方が一覧のタグと違っても、IDで使用中とわかる。
    assert_eq!(
        used("docker.io/library/alpine:latest", "sha256:aaaaaaaaaaaa1111"),
        [true, false]
    );
    assert_eq!(
        used("alpine@sha256:0123456789abcdef", "sha256:aaaaaaaaaaaa1111"),
        [true, false]
    );
    // タグが別のイメージへ移ったあとは、名前が同じでも元のイメージだけが使用中。
    assert_eq!(
        used("nginx:latest", "sha256:aaaaaaaaaaaa1111"),
        [true, false]
    );
    // IDを持たない古い状態ファイルでは、名前で比べる。
    assert_eq!(used("nginx:latest", ""), [false, true]);
}

#[test]
fn image_in_use_matches_tags_ids_and_untagged_names() {
    let rows = [image(
        "sha256:aaaaaaaaaaaabbbbbbbb",
        &["nginx:latest", "web:1"],
        1,
    )];
    let used_by = |name: &str| {
        let containers = [ContainerRow {
            image: name.into(),
            ..row("c1", "c", "exited", false)
        }];
        entries(&rows)
            .iter()
            .map(|e| in_use(e, &containers))
            .collect::<Vec<_>>()
    };
    assert_eq!(used_by("nginx:latest"), [true, false]);
    // タグを省いて作ったコンテナは、:latest として数える。
    assert_eq!(used_by("nginx"), [true, false]);
    assert_eq!(used_by("web:1"), [false, true]);
    assert_eq!(used_by("sha256:aaaaaaaaaaaabbbbbbbb"), [true, true]);
    assert_eq!(used_by("aaaaaaaaaaaa"), [true, true]);
    assert_eq!(used_by("aaaa"), [false, false]);
    assert_eq!(used_by(""), [false, false]);
    assert_eq!(used_by("ghcr.io:5000/x"), [false, false]);
    assert_eq!(used_by("other:latest"), [false, false]);
}

#[test]
fn image_row_delete_confirms_with_name() {
    let state = images_state(
        vec![image("sha256:aaaaaaaaaaaabbbb", &["nginx:alpine"], 5)],
        vec![],
        None,
    );
    let (mut h, mut ends) = open(Lang::En, state);
    click(&mut h, "Delete nginx:alpine");
    let confirm = h.state().0.confirm.clone().unwrap();
    assert_eq!(confirm.message, "Delete image nginx:alpine?");
    // 使われていないイメージには、使用中でも削除の選択を出さない。
    assert!(h.query_by_label("Delete even if in use (force)").is_none());
    assert!(requests(&mut ends).is_empty());
    click(&mut h, "Delete");
    assert_eq!(
        requests(&mut ends),
        [Request::RemoveImage {
            profile: "dev".into(),
            id: "nginx:alpine".into(),
            force: false,
            size: 5
        }]
    );
    assert!(h.state().0.busy_ops.contains("image:nginx:alpine"));
    assert!(h.state().0.confirm.is_none());
}

#[test]
fn cancelling_the_image_confirmation_sends_nothing() {
    let state = images_state(
        vec![image("sha256:aaaaaaaaaaaabbbb", &["nginx:alpine"], 5)],
        vec![],
        None,
    );
    let (mut h, mut ends) = open(Lang::En, state);
    click(&mut h, "Delete nginx:alpine");
    click(&mut h, "Cancel");
    assert!(requests(&mut ends).is_empty());
    assert!(h.state().0.confirm.is_none());
}

#[test]
fn an_image_in_use_offers_force_and_choosing_it_sends_force() {
    let containers = vec![ContainerRow {
        image: "nginx:alpine".into(),
        ..row("c1", "web", "running", false)
    }];
    let state = images_state(
        vec![image("sha256:aaaaaaaaaaaabbbb", &["nginx:alpine"], 5)],
        containers,
        None,
    );
    let (mut h, mut ends) = open(Lang::En, state);
    click(&mut h, "Delete nginx:alpine");
    click(&mut h, "Delete even if in use (force)");
    click(&mut h, "Delete");
    assert_eq!(
        requests(&mut ends),
        [Request::RemoveImage {
            profile: "dev".into(),
            id: "nginx:alpine".into(),
            force: true,
            size: 5
        }]
    );
}

#[test]
fn an_image_in_use_is_not_forced_unless_chosen() {
    let containers = vec![ContainerRow {
        image: "nginx:alpine".into(),
        ..row("c1", "web", "running", false)
    }];
    let state = images_state(
        vec![image("sha256:aaaaaaaaaaaabbbb", &["nginx:alpine"], 5)],
        containers,
        None,
    );
    let (mut h, mut ends) = open(Lang::En, state);
    click(&mut h, "Delete nginx:alpine");
    click(&mut h, "Delete");
    assert_eq!(
        requests(&mut ends),
        [Request::RemoveImage {
            profile: "dev".into(),
            id: "nginx:alpine".into(),
            force: false,
            size: 5
        }]
    );
}

#[test]
fn a_busy_image_row_cannot_be_deleted_again() {
    let mut state = images_state(
        vec![image("sha256:aaaaaaaaaaaabbbb", &["nginx:alpine"], 5)],
        vec![],
        None,
    );
    state.busy_ops.insert("image:nginx:alpine".into());
    let (h, _ends) = open(Lang::En, state);
    assert!(
        h.get_by_label("Delete nginx:alpine")
            .accesskit_node()
            .is_disabled()
    );
}

#[test]
fn cleanup_requests_follow_the_chosen_combination() {
    let want = |dangling: Option<bool>, cache: bool| {
        let mut v = Vec::new();
        if let Some(dangling) = dangling {
            v.push(Request::PruneImages {
                profile: "dev".into(),
                dangling,
            });
        }
        if cache {
            v.push(Request::PruneBuildCache {
                profile: "dev".into(),
            });
        }
        v
    };
    let dialog = |images, build_cache| CleanupDialog {
        images,
        build_cache,
    };
    use ImageCleanup::*;
    assert_eq!(dialog(Keep, false).requests("dev"), want(None, false));
    assert!(dialog(Keep, false).is_empty());
    assert_eq!(dialog(Keep, true).requests("dev"), want(None, true));
    assert_eq!(
        dialog(Dangling, false).requests("dev"),
        want(Some(true), false)
    );
    assert_eq!(
        dialog(Unused, false).requests("dev"),
        want(Some(false), false)
    );
    assert_eq!(
        dialog(Dangling, true).requests("dev"),
        want(Some(true), true)
    );
    assert_eq!(
        dialog(Unused, true).requests("dev"),
        want(Some(false), true)
    );
    assert!(!dialog(Dangling, false).is_empty());
    assert!(!dialog(Keep, true).is_empty());
}

#[test]
fn image_cleanup_dialog_options() {
    let state = images_state(vec![], vec![], Some(3 << 30));
    let (mut h, mut ends) = open(Lang::En, state);
    click(&mut h, "Clean up…");
    h.get_by_label("Keep images");
    h.get_by_label("Remove dangling images only");
    h.get_by_label("Remove all unused images");
    h.get_by_label("Remove the build cache (3.0 GiB)");
    // 何も選ばなければ「削除」は押せない。
    assert!(h.get_by_label("Delete").accesskit_node().is_disabled());
    click(&mut h, "Remove dangling images only");
    click(&mut h, "Remove the build cache (3.0 GiB)");
    assert!(!h.get_by_label("Delete").accesskit_node().is_disabled());
    click(&mut h, "Delete");
    assert_eq!(
        requests(&mut ends),
        [
            Request::PruneImages {
                profile: "dev".into(),
                dangling: true
            },
            Request::PruneBuildCache {
                profile: "dev".into()
            }
        ]
    );
    let busy = &h.state().0.busy_ops;
    assert!(busy.contains("prune-images:dev") && busy.contains("prune-build-cache:dev"));
    assert!(h.state().0.images.cleanup.is_none());
}

#[test]
fn image_cleanup_can_remove_only_the_build_cache() {
    let state = images_state(vec![], vec![], None);
    let (mut h, mut ends) = open(Lang::En, state);
    click(&mut h, "Clean up…");
    // 容量が取れていないときは「不明」と出す。
    click(&mut h, "Remove the build cache (unknown)");
    click(&mut h, "Delete");
    assert_eq!(
        requests(&mut ends),
        [Request::PruneBuildCache {
            profile: "dev".into()
        }]
    );
    let busy = &h.state().0.busy_ops;
    assert!(busy.contains("prune-build-cache:dev") && !busy.contains("prune-images:dev"));
}

#[test]
fn image_cleanup_cancel_sends_nothing_and_keeps_the_choice_out_of_the_next_open() {
    let state = images_state(vec![], vec![], Some(1));
    let (mut h, mut ends) = open(Lang::En, state);
    click(&mut h, "Clean up…");
    click(&mut h, "Remove all unused images");
    click(&mut h, "Cancel");
    assert!(requests(&mut ends).is_empty());
    click(&mut h, "Clean up…");
    assert!(h.get_by_label("Delete").accesskit_node().is_disabled());
}

#[test]
fn cleanup_button_is_disabled_while_a_cleanup_runs() {
    for key in ["prune-images:dev", "prune-build-cache:dev"] {
        let mut state = images_state(vec![], vec![], None);
        state.busy_ops.insert(key.into());
        let (h, _ends) = open(Lang::En, state);
        assert!(
            h.get_by_label("Clean up…").accesskit_node().is_disabled(),
            "{key}"
        );
    }
}

#[test]
fn japanese_labels_are_shown() {
    let state = images_state(
        vec![image("sha256:aaaaaaaaaaaabbbb", &["nginx:alpine"], 5)],
        vec![],
        Some(1 << 20),
    );
    let (mut h, _ends) = open(Lang::Ja, state);
    h.get_by_label("削除 nginx:alpine");
    click(&mut h, "片付け…");
    h.get_by_label("イメージは消さない");
    h.get_by_label("ビルドキャッシュを消す（1.0 MiB）");
}

#[test]
fn rows_show_id_size_and_age() {
    let state = images_state(
        vec![image("sha256:aaaaaaaaaaaabbbb", &["nginx:alpine"], 5 << 20)],
        vec![],
        None,
    );
    let (h, _ends) = open(Lang::En, state);
    h.get_by_label("aaaaaaaaaaaa");
    h.get_by_label("5.0 MiB");
}

#[test]
fn empty_list_says_so() {
    let (h, _ends) = open(Lang::En, images_state(vec![], vec![], None));
    h.get_by_label("No images");
}

#[test]
fn created_is_shown_as_a_date_in_the_given_offset() {
    assert_eq!(date_text(1_000, 0), "1970-01-01");
    // 2023-11-14 22:13:20 UTC は、日本時間では翌日。
    assert_eq!(date_text(1_700_000_000, 0), "2023-11-14");
    assert_eq!(date_text(1_700_000_000, 9 * 3_600), "2023-11-15");
    assert_eq!(date_text(1_700_000_000, -23 * 3_600), "2023-11-13");
    assert_eq!(date_text(951_782_400, 0), "2000-02-29");
    assert_eq!(date_text(951_868_799, 0), "2000-02-29");
    assert_eq!(date_text(951_868_800, 0), "2000-03-01");
    assert_eq!(date_text(4_107_542_400, 0), "2100-03-01");
    assert_eq!(date_text(1_709_164_800, 0), "2024-02-29");
    assert_eq!(date_text(1_735_603_200, 0), "2024-12-31");
    // Dockerが作成日時を返さないときは「-」。
    assert_eq!(date_text(0, 9 * 3_600), "-");
    assert_eq!(date_text(-1, 0), "-");
}

#[test]
fn local_offset_matches_the_system_clock() {
    // date(1) が同じ時刻に出す時差（+HHMM）と比べる。
    for t in [1_700_000_000_i64, 1_720_000_000] {
        let out = std::process::Command::new("date")
            .args(["-r", &t.to_string(), "+%z"])
            .output()
            .unwrap();
        let z = String::from_utf8(out.stdout).unwrap();
        let z = z.trim();
        let sign = if z.starts_with('-') { -1 } else { 1 };
        let hours: i64 = z[1..3].parse().unwrap();
        let minutes: i64 = z[3..5].parse().unwrap();
        assert_eq!(
            local_offset(t),
            sign * (hours * 3_600 + minutes * 60),
            "{z}"
        );
    }
}

#[test]
fn the_row_shows_the_created_date() {
    let created = 1_700_000_000;
    let mut img = image("sha256:aaaaaaaaaaaabbbb", &["nginx:alpine"], 5);
    img.created = created;
    let (h, _ends) = open(Lang::En, images_state(vec![img], vec![], None));
    h.get_by_label(&date_text(created, local_offset(created)));
}

fn pulled(id: &str, status: &str, current: Option<u64>, total: Option<u64>) -> PullProgress {
    PullProgress {
        id: Some(id.into()),
        status: status.into(),
        current,
        total,
    }
}

#[test]
fn pull_progress_aggregates_layers() {
    assert_eq!(overall_progress(&[]), None);
    assert_eq!(overall_progress(&[(0, 0)]), None);
    assert_eq!(overall_progress(&[(5, 10)]), Some(0.5));
    assert_eq!(overall_progress(&[(10, 10), (0, 30)]), Some(0.25));
    // 現在値が総量を超えても、1を超えない。
    assert_eq!(overall_progress(&[(20, 10)]), Some(1.0));
}

#[test]
fn pull_state_tracks_each_layer_and_marks_finished_ones() {
    let mut p = PullState::default();
    assert_eq!(p.progress(), None);
    p.update(&pulled("l1", "Downloading", Some(5), Some(10)));
    p.update(&pulled("l2", "Downloading", Some(0), Some(30)));
    assert_eq!(p.progress(), Some(0.125));
    p.update(&pulled("l1", "Downloading", Some(10), Some(10)));
    p.update(&pulled("l2", "Download complete", None, None));
    assert_eq!(p.progress(), Some(1.0));
    // 展開の進捗は、取得の割合を戻さない。
    p.update(&pulled("l2", "Extracting", Some(1), Some(5)));
    assert_eq!(p.progress(), Some(1.0));
    p.update(&pulled("l3", "Already exists", None, None));
    assert_eq!(p.progress(), Some(1.0));
    // 層を持たない行は、状態の文字だけ更新する。
    p.update(&PullProgress {
        id: None,
        status: "Status: Downloaded newer image".into(),
        current: None,
        total: None,
    });
    assert_eq!(p.status, "Status: Downloaded newer image");
    // 層ごとの完了は、その層の総量まで進める。
    let mut q = PullState::default();
    q.update(&pulled("a", "Downloading", Some(1), Some(8)));
    q.update(&pulled("a", "Pull complete", None, None));
    assert_eq!(q.progress(), Some(1.0));
}

#[test]
fn pull_form_sends_the_reference_and_shows_progress() {
    let state = images_state(vec![], vec![], None);
    let (mut h, mut ends) = open(Lang::En, state);
    assert!(h.get_by_label("Pull").accesskit_node().is_disabled());
    h.state_mut().0.images.reference = "nginx:alpine".into();
    h.run_steps(4);
    click(&mut h, "Pull");
    assert_eq!(
        requests(&mut ends),
        [Request::Pull {
            profile: "dev".into(),
            reference: "nginx:alpine".into()
        }]
    );
    assert!(h.state().0.busy_ops.contains("pull:dev"));
    assert!(h.state().0.images.pull.is_some());
    assert!(h.get_by_label("Pull").accesskit_node().is_disabled());
    // 進捗は返事から画面に映る。
    h.state_mut().0.apply_reply(Reply::PullProgress {
        profile: "dev".into(),
        progress: pulled("l1", "Downloading", Some(5), Some(10)),
    });
    h.run_steps(4);
    assert_eq!(
        h.state().0.images.pull.as_ref().unwrap().progress(),
        Some(0.5)
    );
}

#[test]
fn pull_form_rejects_a_bad_reference_and_explains() {
    let state = images_state(vec![], vec![], None);
    let (mut h, mut ends) = open(Lang::En, state);
    h.state_mut().0.images.reference = "a b".into();
    h.run_steps(4);
    assert!(h.get_by_label("Pull").accesskit_node().is_disabled());
    h.get_by_label_contains("invalid name, ID or image reference");
    assert!(requests(&mut ends).is_empty());
}

#[test]
fn a_finished_pull_clears_the_progress() {
    let state = images_state(vec![], vec![], None);
    let (mut h, _ends) = open(Lang::En, state);
    h.state_mut().0.images.pull = Some(PullState::default());
    h.run_steps(4);
    assert!(h.state().0.images.pull.is_none());
}

fn one_image() -> GuiState {
    images_state(
        vec![image("sha256:aaaaaaaaaaaabbbb", &["nginx:alpine"], 5)],
        vec![],
        None,
    )
}

#[test]
fn inspect_button_opens_the_detail_and_shows_the_reply() {
    let (mut h, mut ends) = open(Lang::En, one_image());
    click(&mut h, "Inspect nginx:alpine");
    assert_eq!(
        requests(&mut ends),
        [Request::InspectResource {
            profile: "dev".into(),
            kind: ResourceKind::Image,
            target: "nginx:alpine".into()
        }]
    );
    h.state_mut().0.apply_reply(Reply::ResourceInspected {
        kind: ResourceKind::Image,
        target: "nginx:alpine".into(),
        text: "{\"Id\": \"sha256:aa\"}".into(),
    });
    h.run_steps(4);
    h.get_by_label_contains("sha256:aa");
    // 別のリソースの返事は、開いている詳細に入らない。
    h.state_mut().0.apply_reply(Reply::ResourceInspected {
        kind: ResourceKind::Volume,
        target: "nginx:alpine".into(),
        text: "wrong".into(),
    });
    h.run_steps(4);
    assert!(h.query_by_label_contains("wrong").is_none());
    click(&mut h, "Close");
    assert!(h.state().0.inspect.is_none());
}

#[test]
fn tag_form_sends_the_new_tag() {
    let (mut h, mut ends) = open(Lang::En, one_image());
    click(&mut h, "Tag nginx:alpine");
    assert!(h.get_by_label("Add tag").accesskit_node().is_disabled());
    {
        let form = h.state_mut().0.images.tag.as_mut().unwrap();
        form.repo = "ghcr.io/o/app".into();
        form.tag = "v1".into();
    }
    h.run_steps(4);
    click(&mut h, "Add tag");
    assert_eq!(
        requests(&mut ends),
        [Request::TagImage {
            profile: "dev".into(),
            id: "nginx:alpine".into(),
            repo: "ghcr.io/o/app".into(),
            tag: "v1".into()
        }]
    );
    assert!(h.state().0.busy_ops.contains("tag:dev"));
    assert!(h.state().0.images.tag.is_none());
}

#[test]
fn tag_form_rejects_bad_input_and_can_be_cancelled() {
    let (mut h, mut ends) = open(Lang::En, one_image());
    click(&mut h, "Tag nginx:alpine");
    h.state_mut().0.images.tag.as_mut().unwrap().repo = "a b".into();
    h.run_steps(4);
    assert!(h.get_by_label("Add tag").accesskit_node().is_disabled());
    h.get_by_label_contains("invalid name, ID or image reference");
    {
        let form = h.state_mut().0.images.tag.as_mut().unwrap();
        form.repo = "app".into();
        form.tag = "a b".into();
    }
    h.run_steps(4);
    assert!(h.get_by_label("Add tag").accesskit_node().is_disabled());
    // docker tag と同じく、リポジトリは小文字、タグは英数字か _ で始まる。
    for (repo, tag) in [("App", "v1"), ("app", ".v1"), ("app:v1", "v1")] {
        {
            let form = h.state_mut().0.images.tag.as_mut().unwrap();
            form.repo = repo.into();
            form.tag = tag.into();
        }
        h.run_steps(4);
        assert!(
            h.get_by_label("Add tag").accesskit_node().is_disabled(),
            "{repo} {tag}"
        );
    }
    click(&mut h, "Cancel");
    assert!(h.state().0.images.tag.is_none());
    assert!(requests(&mut ends).is_empty());
}

#[test]
fn tag_form_defaults_to_latest() {
    let f = TagForm::new("x:1".into(), "x:1".into());
    assert_eq!(f.tag, "latest");
    assert!(f.check().is_err());
}

#[test]
fn the_pull_field_is_locked_while_a_pull_runs() {
    use eframe::egui::accesskit::Role;
    for (busy, locked) in [(false, false), (true, true)] {
        let mut state = images_state(vec![], vec![], None);
        if busy {
            state.busy_ops.insert("pull:dev".into());
        }
        let (h, _ends) = open(Lang::En, state);
        let field = h.get_by_role(Role::TextInput);
        assert_eq!(field.accesskit_node().is_disabled(), locked, "busy={busy}");
    }
}

#[test]
fn image_cleanup_goes_to_the_selected_instance() {
    let summary = DockerSummary {
        disk: DiskUsage {
            build_cache: Some(1),
            ..Default::default()
        },
        ..Default::default()
    };
    let (mut h, mut ends) = open(Lang::En, state_on_prod(summary, Page::Images));
    click(&mut h, "Clean up…");
    click(&mut h, "Remove all unused images");
    click(&mut h, "Remove the build cache (1 B)");
    click(&mut h, "Delete");
    assert_eq!(
        requests(&mut ends),
        [
            Request::PruneImages {
                profile: "prod".into(),
                dangling: false
            },
            Request::PruneBuildCache {
                profile: "prod".into()
            }
        ]
    );
}
