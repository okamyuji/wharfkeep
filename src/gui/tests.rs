use super::App;
use super::bridge::{Bridge, Reply, Request, WorkerEnds};
use super::confirm::{self, Confirm, PendingAction};
use super::state::{GuiState, Page, Polled, StateWatcher, daemon_stopped, default_selection};
use crate::colima::{Action, Instance, InstanceStatus};
use crate::i18n::Lang;
use crate::snapshot::{self, DockerState, DockerSummary, Snapshot};
use eframe::egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

fn instance(name: &str, status: InstanceStatus) -> Instance {
    Instance {
        name: name.into(),
        status,
        arch: "aarch64".into(),
        cpus: 4,
        memory_bytes: 8 << 30,
        disk_bytes: 60 << 30,
        runtime: "docker".into(),
    }
}

/// `old`（停止中）が先、`dev`（実行中）が後。
fn snapshot_at(updated_at: SystemTime) -> Snapshot {
    Snapshot {
        updated_at,
        instances: vec![
            instance("old", InstanceStatus::Stopped),
            instance("dev", InstanceStatus::Running),
        ],
        docker: BTreeMap::from([
            (
                "old".to_string(),
                DockerState::Unavailable {
                    reason: "the instance is not running".into(),
                },
            ),
            (
                "dev".to_string(),
                DockerState::Ready(DockerSummary::default()),
            ),
        ]),
    }
}

fn fresh() -> Snapshot {
    snapshot_at(SystemTime::now())
}

struct Fixture {
    _dir: tempfile::TempDir,
    ends: WorkerEnds,
    daemon_starts: Rc<Cell<u32>>,
}

fn harness(lang: Lang, snap: Option<Snapshot>, start_ok: bool) -> (Harness<'static, App>, Fixture) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    if let Some(s) = &snap {
        snapshot::write_atomic(&path, s).unwrap();
    }
    let (bridge, ends) = Bridge::pair();
    let daemon_starts = Rc::new(Cell::new(0));
    let counter = daemon_starts.clone();
    let app = App::new(
        lang,
        path,
        bridge,
        Box::new(move || {
            counter.set(counter.get() + 1);
            if start_ok {
                Ok(())
            } else {
                Err(io::Error::other("spawn failed"))
            }
        }),
    );
    let mut h = app_harness(app, lang);
    h.run();
    (
        h,
        Fixture {
            _dir: dir,
            ends,
            daemon_starts,
        },
    )
}

fn app_harness(app: App, lang: Lang) -> Harness<'static, App> {
    Harness::builder()
        .with_size([720.0, 480.0])
        .with_context_setup(move |ctx| super::install_fonts(ctx, lang))
        .build_ui_state(|ui, app: &mut App| app.draw(ui), app)
}

fn requests(f: &mut Fixture) -> Vec<Request> {
    std::iter::from_fn(|| f.ends.requests.try_recv().ok()).collect()
}

fn start_old() -> Request {
    Request::Colima {
        action: Action::Start,
        profile: "old".into(),
    }
}

// --- 確認ダイアログ ---

fn confirm_harness(lang: Lang) -> (Harness<'static, (Option<Confirm>, Bridge)>, WorkerEnds) {
    let (bridge, ends) = Bridge::pair();
    let h = Harness::builder()
        .with_context_setup(move |ctx| super::install_fonts(ctx, lang))
        .build_ui_state(
            move |ui, (confirm, bridge): &mut (Option<Confirm>, Bridge)| {
                if ui.button("Delete dev").clicked() {
                    *confirm = Some(Confirm {
                        message: "Delete dev?".into(),
                        action: PendingAction::Colima {
                            action: Action::Delete,
                            profile: "dev".into(),
                        },
                    });
                }
                if let Some(a) = confirm::show(ui, confirm, lang) {
                    bridge.send(a.into_request());
                }
            },
            (None, bridge),
        );
    (h, ends)
}

#[test]
fn confirm_cancel_sends_nothing() {
    let (mut h, mut ends) = confirm_harness(Lang::En);
    h.get_by_label("Delete dev").click();
    h.run();
    h.get_by_label("Delete dev?");
    h.get_by_label("Cancel").click();
    h.run();
    assert!(ends.requests.try_recv().is_err());
    assert!(h.state().0.is_none());
    assert!(h.query_by_label("Cancel").is_none());
}

#[test]
fn confirm_accept_sends_once() {
    let (mut h, mut ends) = confirm_harness(Lang::En);
    h.get_by_label("Delete dev").click();
    h.run();
    h.get_by_label("Delete").click();
    h.run();
    h.run();
    assert_eq!(
        ends.requests.try_recv().unwrap(),
        Request::Colima {
            action: Action::Delete,
            profile: "dev".into()
        }
    );
    assert!(ends.requests.try_recv().is_err());
    assert!(h.state().0.is_none());
}

#[test]
fn confirm_buttons_are_in_japanese() {
    let (mut h, _ends) = confirm_harness(Lang::Ja);
    h.get_by_label("Delete dev").click();
    h.run();
    h.get_by_label("キャンセル");
    h.get_by_label("削除");
}

// --- バナー ---

#[test]
fn banner_when_snapshot_missing() {
    let (h, _f) = harness(Lang::En, None, true);
    h.get_by_label("The daemon is not running");
    h.get_by_label("Start daemon");
}

#[test]
fn banner_when_stale() {
    let old = SystemTime::now() - Duration::from_secs(16);
    let (h, _f) = harness(Lang::En, Some(snapshot_at(old)), true);
    h.get_by_label("The daemon is not running");
    h.get_by_label("Start daemon");
}

#[test]
fn no_banner_when_fresh() {
    let (h, _f) = harness(Lang::En, Some(fresh()), true);
    assert!(h.query_by_label("The daemon is not running").is_none());
    assert!(h.query_by_label("Start daemon").is_none());
}

#[test]
fn banner_is_shown_in_japanese() {
    let (h, _f) = harness(Lang::Ja, None, true);
    h.get_by_label("デーモンが動いていません");
    h.get_by_label("デーモンを起動");
}

#[test]
fn start_daemon_button_starts_the_daemon() {
    let (mut h, f) = harness(Lang::En, None, true);
    h.get_by_label("Start daemon").click();
    h.run();
    assert_eq!(f.daemon_starts.get(), 1);
    assert!(h.state().state.toasts.is_empty());
}

#[test]
fn start_daemon_failure_is_shown_as_a_toast() {
    let (mut h, f) = harness(Lang::En, None, false);
    h.get_by_label("Start daemon").click();
    h.run();
    assert_eq!(f.daemon_starts.get(), 1);
    h.get_by_label("cannot start the daemon: spawn failed");
}

// --- インスタンスの選択と停止中の表示 ---

#[test]
fn selection_defaults_to_first_running() {
    let (h, _f) = harness(Lang::En, Some(fresh()), true);
    assert_eq!(h.state().state.selected.as_deref(), Some("dev"));
}

#[test]
fn unavailable_instance_shows_reason_and_start_button() {
    let (mut h, mut f) = harness(Lang::En, Some(fresh()), true);
    h.state_mut().state.selected = Some("old".into());
    h.state_mut().state.page = Page::Containers;
    h.run();
    h.get_by_label("the instance is not running");
    h.get_by_label("Start instance").click();
    h.run();
    assert_eq!(requests(&mut f), vec![start_old()]);
}

#[test]
fn start_instance_button_is_disabled_while_the_request_is_running() {
    let (mut h, mut f) = harness(Lang::En, Some(fresh()), true);
    h.state_mut().state.selected = Some("old".into());
    h.state_mut().state.page = Page::Images;
    h.run();
    h.get_by_label("Start instance").click();
    h.run();
    h.get_by_label("Start instance").click();
    h.run();
    assert_eq!(requests(&mut f), vec![start_old()]);
}

#[test]
fn missing_docker_entry_is_treated_as_unavailable() {
    let mut s = fresh();
    s.docker.remove("old");
    let (mut h, _f) = harness(Lang::En, Some(s), true);
    h.state_mut().state.selected = Some("old".into());
    h.state_mut().state.page = Page::Volumes;
    h.run();
    h.get_by_label("the instance is not running");
    h.get_by_label("Start instance");
}

#[test]
fn unavailable_panel_is_in_japanese() {
    let mut s = fresh();
    s.docker.remove("old");
    let (mut h, _f) = harness(Lang::Ja, Some(s), true);
    h.state_mut().state.selected = Some("old".into());
    h.state_mut().state.page = Page::Networks;
    h.run();
    h.get_by_label("インスタンスが動いていません");
    h.get_by_label("インスタンスを起動");
}

#[test]
fn ready_instance_shows_no_start_button() {
    let (mut h, _f) = harness(Lang::En, Some(fresh()), true);
    h.state_mut().state.page = Page::Containers;
    h.run();
    assert!(h.query_by_label("Start instance").is_none());
}

#[test]
fn dashboard_and_instances_pages_do_not_need_docker() {
    let (mut h, _f) = harness(Lang::En, Some(fresh()), true);
    h.state_mut().state.selected = Some("old".into());
    for page in [Page::Dashboard, Page::Instances] {
        h.state_mut().state.page = page;
        h.run();
        assert!(h.query_by_label("Start instance").is_none(), "{page:?}");
    }
}

#[test]
fn sidebar_lists_pages_and_switches() {
    let (mut h, _f) = harness(Lang::En, Some(fresh()), true);
    for (label, page) in [
        ("Dashboard", Page::Dashboard),
        ("Instances", Page::Instances),
        ("Containers", Page::Containers),
        ("Images", Page::Images),
        ("Volumes", Page::Volumes),
        ("Networks", Page::Networks),
    ] {
        h.get_by_role_and_label(Role::Button, label).click();
        h.run();
        assert_eq!(h.state().state.page, page);
        // 見出しにも同じ名前が出る。
        h.get_by_role_and_label(Role::Heading, label);
    }
}

#[test]
fn sidebar_pages_are_in_japanese() {
    let (h, _f) = harness(Lang::Ja, Some(fresh()), true);
    for label in [
        "ダッシュボード",
        "インスタンス",
        "コンテナ",
        "イメージ",
        "ボリューム",
        "ネットワーク",
    ] {
        h.get_by_role_and_label(Role::Button, label);
    }
}

#[test]
fn instance_selector_changes_the_selection() {
    let (mut h, _f) = harness(Lang::En, Some(fresh()), true);
    // ダッシュボードにもインスタンス名が出るので、名前の出ない画面で選ぶ。
    h.state_mut().state.page = Page::Images;
    h.run();
    h.get_by_role_and_label(Role::ComboBox, "Instance").click();
    h.run();
    h.get_by_label("old").click();
    h.run();
    assert_eq!(h.state().state.selected.as_deref(), Some("old"));
}

#[test]
fn switching_the_instance_drops_the_views_of_the_previous_one() {
    use super::pages::Inspect;
    use super::pages::containers::{Detail, RenameForm};
    use super::pages::images::{PullState, TagForm};
    let (mut h, mut f) = harness(Lang::En, Some(fresh()), true);
    h.state_mut().state.page = Page::Images;
    {
        let st = &mut h.state_mut().state;
        assert_eq!(st.selected.as_deref(), Some("dev"));
        st.containers.detail = Some(Detail {
            id: "c1".into(),
            name: "web".into(),
            follow: true,
            log: Default::default(),
            inspect: None,
            usage: None,
        });
        st.containers.rename = Some(RenameForm {
            id: "c1".into(),
            name: "web".into(),
            new_name: "web-2".into(),
        });
        st.images.tag = Some(TagForm::new("nginx:alpine".into(), "nginx:alpine".into()));
        st.images.pull = Some(PullState::default());
        st.inspect = Some(Inspect {
            kind: super::bridge::ResourceKind::Image,
            target: "nginx:alpine".into(),
            name: "nginx:alpine".into(),
            text: None,
        });
    }
    h.run();
    requests(&mut f);
    h.get_by_role_and_label(Role::ComboBox, "Instance").click();
    h.run();
    h.get_by_label("old").click();
    h.run();
    let st = &h.state().state;
    assert_eq!(st.selected.as_deref(), Some("old"));
    assert!(st.containers.detail.is_none());
    assert!(st.containers.rename.is_none());
    assert!(st.images.tag.is_none());
    assert!(st.images.pull.is_none());
    assert!(st.inspect.is_none());
    // 前のインスタンスのログの流れを止める要求だけが出て、前のインスタンス向けの要求は出ない。
    assert_eq!(requests(&mut f), [Request::StopLogs { id: "c1".into() }]);
    h.run();
    assert!(requests(&mut f).is_empty());
}

#[test]
fn an_instance_that_disappears_from_the_state_file_drops_its_views() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let mut s = fresh();
    s.instances.retain(|i| i.name != "dev");
    s.docker.remove("dev");
    snapshot::write_atomic(&path, &s).unwrap();
    let (bridge, mut ends) = Bridge::pair();
    let mut app = App::new(Lang::En, path, bridge, Box::new(|| Ok(())));
    // 前の描画で dev を選び、タグの入力を開いていた状態から始める。
    app.state.selected = Some("dev".into());
    app.state.images.tag = Some(super::pages::images::TagForm::new(
        "x:1".into(),
        "x:1".into(),
    ));
    let mut h = app_harness(app, Lang::En);
    h.run();
    assert_eq!(h.state().state.selected.as_deref(), Some("old"));
    assert!(h.state().state.images.tag.is_none());
    assert!(ends.requests.try_recv().is_err());
}

#[test]
fn pull_progress_of_another_instance_is_ignored() {
    use super::bridge::Reply;
    use crate::docker::actions::PullProgress;
    let mut s = GuiState::new();
    s.apply_snapshot(Some(fresh()));
    assert_eq!(s.selected.as_deref(), Some("dev"));
    s.images.pull = Some(super::pages::images::PullState::default());
    let progress = |profile: &str, status: &str| Reply::PullProgress {
        profile: profile.into(),
        progress: PullProgress {
            id: None,
            status: status.into(),
            current: None,
            total: None,
        },
    };
    s.apply_reply(progress("old", "from old"));
    assert_eq!(s.images.pull.as_ref().unwrap().status, "");
    s.apply_reply(progress("dev", "from dev"));
    assert_eq!(s.images.pull.as_ref().unwrap().status, "from dev");
}

#[test]
fn keeping_the_same_instance_keeps_the_views() {
    let (mut h, mut f) = harness(Lang::En, Some(fresh()), true);
    h.state_mut().state.page = Page::Images;
    h.state_mut().state.images.tag = Some(super::pages::images::TagForm::new(
        "nginx:alpine".into(),
        "nginx:alpine".into(),
    ));
    h.run();
    h.get_by_role_and_label(Role::ComboBox, "Instance").click();
    h.run();
    h.get_by_label("dev").click();
    h.run();
    assert!(h.state().state.images.tag.is_some());
    assert!(requests(&mut f).is_empty());
}

#[test]
fn no_instances_is_said_so() {
    let mut s = fresh();
    s.instances.clear();
    s.docker.clear();
    let (h, _f) = harness(Lang::En, Some(s), true);
    h.get_by_label("No colima instance found");
}

// --- 通知と返事 ---

#[test]
fn toast_can_be_dismissed() {
    let (mut h, _f) = harness(Lang::En, Some(fresh()), true);
    h.state_mut().state.push_toast("colima failed".into());
    h.run();
    h.get_by_label("colima failed");
    h.get_by_label("Dismiss").click();
    h.run();
    assert!(h.query_by_label("colima failed").is_none());
    assert!(h.state().state.toasts.is_empty());
}

#[test]
fn colima_reply_clears_busy_and_shows_the_error() {
    let (bridge, ends) = Bridge::pair();
    let app_state = GuiState::new();
    drop(app_state);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    snapshot::write_atomic(&path, &fresh()).unwrap();
    let app = App::new(Lang::En, path, bridge, Box::new(|| Ok(())));
    let mut h = app_harness(app, Lang::En);
    h.state_mut().state.busy.insert("old".into());
    ends.replies
        .send(super::bridge::Reply::ColimaDone {
            profile: "old".into(),
            error: Some("colima failed (exit code): 1".into()),
        })
        .unwrap();
    h.run();
    assert!(h.state().state.busy.is_empty());
    h.get_by_label("colima failed (exit code): 1");
}

#[test]
fn unreadable_state_file_is_shown_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    std::fs::write(&path, "not json").unwrap();
    let (bridge, _ends) = Bridge::pair();
    let app = App::new(Lang::En, path, bridge, Box::new(|| Ok(())));
    let mut h = app_harness(app, Lang::En);
    h.run();
    assert_eq!(h.state().state.toasts.len(), 1);
    assert!(
        h.state().state.toasts[0]
            .message
            .contains("the state file is not valid")
    );
}

// --- 純粋な部分 ---

#[test]
fn daemon_stopped_is_decided_by_the_snapshot_age() {
    let now = SystemTime::now();
    assert!(daemon_stopped(None, now));
    assert!(!daemon_stopped(
        Some(&snapshot_at(now - Duration::from_secs(15))),
        now
    ));
    assert!(daemon_stopped(
        Some(&snapshot_at(now - Duration::from_secs(16))),
        now
    ));
    assert!(!daemon_stopped(
        Some(&snapshot_at(now + Duration::from_secs(60))),
        now
    ));
}

#[test]
fn default_selection_prefers_running_then_first() {
    assert_eq!(default_selection(&fresh()).as_deref(), Some("dev"));
    let mut s = fresh();
    s.instances[1].status = InstanceStatus::Stopped;
    assert_eq!(default_selection(&s).as_deref(), Some("old"));
    s.instances.clear();
    assert_eq!(default_selection(&s), None);
}

#[test]
fn apply_snapshot_keeps_a_valid_selection_and_reselects_a_gone_one() {
    let mut st = GuiState::new();
    st.apply_snapshot(Some(fresh()));
    assert_eq!(st.selected.as_deref(), Some("dev"));
    st.selected = Some("old".into());
    st.apply_snapshot(Some(fresh()));
    assert_eq!(st.selected.as_deref(), Some("old"));
    let mut only_dev = fresh();
    only_dev.instances.remove(0);
    st.apply_snapshot(Some(only_dev));
    assert_eq!(st.selected.as_deref(), Some("dev"));
    st.apply_snapshot(None);
    assert_eq!(st.selected.as_deref(), Some("dev"));
    assert!(st.snapshot.is_none());
}

#[test]
fn selected_docker_follows_the_selection() {
    let mut st = GuiState::new();
    assert!(st.selected_docker().is_none());
    st.apply_snapshot(Some(fresh()));
    assert!(matches!(st.selected_docker(), Some(DockerState::Ready(_))));
    st.selected = Some("old".into());
    assert!(matches!(
        st.selected_docker(),
        Some(DockerState::Unavailable { .. })
    ));
    st.selected = Some("none".into());
    assert!(st.selected_docker().is_none());
}

#[test]
fn push_toast_skips_an_immediate_duplicate() {
    let mut st = GuiState::new();
    st.push_toast("a".into());
    st.push_toast("a".into());
    st.push_toast("b".into());
    st.push_toast("a".into());
    let messages: Vec<&str> = st.toasts.iter().map(|t| t.message.as_str()).collect();
    assert_eq!(messages, ["a", "b", "a"]);
}

#[test]
fn page_titles_and_docker_need() {
    assert_eq!(Page::ALL.len(), 6);
    let needs: Vec<bool> = Page::ALL.iter().map(|p| p.needs_docker()).collect();
    assert_eq!(needs, [false, false, true, true, true, true]);
    let titles: std::collections::HashSet<_> = Page::ALL.iter().map(|p| p.title()).collect();
    assert_eq!(titles.len(), 6);
}

fn write(path: &Path, s: &Snapshot) {
    snapshot::write_atomic(path, s).unwrap();
}

#[test]
fn watcher_reads_when_the_file_changes_and_waits_one_second() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let mut w = StateWatcher::new(path.clone());
    let t0 = Instant::now();
    assert!(matches!(w.poll(t0), Polled::Loaded(None)));
    write(&path, &fresh());
    // 1秒たつまでは見ない。
    assert!(matches!(
        w.poll(t0 + Duration::from_millis(999)),
        Polled::Unchanged
    ));
    assert!(matches!(
        w.poll(t0 + Duration::from_secs(1)),
        Polled::Loaded(Some(_))
    ));
    // 変わっていなければ読み直さない。
    assert!(matches!(
        w.poll(t0 + Duration::from_secs(2)),
        Polled::Unchanged
    ));
    std::thread::sleep(Duration::from_millis(20));
    write(&path, &fresh());
    assert!(matches!(
        w.poll(t0 + Duration::from_secs(3)),
        Polled::Loaded(Some(_))
    ));
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(
        w.poll(t0 + Duration::from_secs(4)),
        Polled::Loaded(None)
    ));
}

#[test]
fn watcher_reports_an_invalid_file_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    std::fs::write(&path, "not json").unwrap();
    let mut w = StateWatcher::new(path);
    let t0 = Instant::now();
    assert!(matches!(w.poll(t0), Polled::Failed(_)));
    assert!(matches!(
        w.poll(t0 + Duration::from_secs(1)),
        Polled::Unchanged
    ));
}

// --- 返事と確認された操作 ---

fn detail_for(id: &str) -> super::pages::containers::Detail {
    super::pages::containers::Detail {
        id: id.into(),
        name: "web".into(),
        follow: true,
        log: super::log_buffer::LogBuffer::default(),
        inspect: None,
        usage: None,
    }
}

#[test]
fn replies_update_the_state() {
    use super::bridge::Reply;
    let mut s = GuiState::new();
    s.busy.insert("dev".into());
    s.busy_ops.insert("c1".into());
    s.containers.detail = Some(detail_for("c1"));

    s.apply_reply(Reply::InstanceLine {
        profile: "dev".into(),
        line: "booting".into(),
    });
    assert_eq!(s.instance_log.lines().collect::<Vec<_>>(), ["booting"]);

    s.apply_reply(Reply::ColimaDone {
        profile: "dev".into(),
        error: None,
    });
    assert!(s.busy.is_empty() && s.toasts.is_empty());

    s.apply_reply(Reply::OpDone {
        key: "c1".into(),
        error: Some("nope".into()),
    });
    assert!(s.busy_ops.is_empty());
    assert_eq!(s.toasts.len(), 1);
    assert_eq!(s.toasts[0].message, "nope");

    s.apply_reply(Reply::Failed("shell failed".into()));
    assert_eq!(s.toasts.len(), 2);

    s.apply_reply(Reply::LogLine {
        id: "c1".into(),
        line: "hi".into(),
    });
    s.apply_reply(Reply::Inspected {
        id: "c1".into(),
        text: "{}".into(),
    });
    let d = s.containers.detail.as_ref().unwrap();
    assert_eq!(d.log.lines().collect::<Vec<_>>(), ["hi"]);
    assert_eq!(d.inspect.as_deref(), Some("{}"));
}

#[test]
fn a_reply_after_the_detail_closed_is_dropped() {
    use super::bridge::Reply;
    let mut s = GuiState::new();
    s.apply_reply(Reply::LogLine {
        id: "c1".into(),
        line: "late".into(),
    });
    assert!(s.containers.detail.is_none());
}

#[test]
fn start_pending_marks_the_right_busy_set_and_returns_the_request() {
    let mut s = GuiState::new();
    s.instance_log.push("old output".into());
    let req = s.start_pending(PendingAction::Colima {
        action: Action::Delete,
        profile: "dev".into(),
    });
    assert_eq!(
        req,
        Request::Colima {
            action: Action::Delete,
            profile: "dev".into()
        }
    );
    assert!(s.busy.contains("dev") && s.busy_ops.is_empty());
    assert!(s.instance_log.is_empty());

    let req = s.start_pending(PendingAction::Container {
        profile: "dev".into(),
        id: "c1".into(),
        force: true,
    });
    assert_eq!(
        req,
        Request::Container {
            profile: "dev".into(),
            id: "c1".into(),
            op: super::bridge::ContainerOp::Remove { force: true }
        }
    );
    assert!(s.busy_ops.contains("c1"));
}

#[test]
fn cleanup_done_toast_shows_reclaimed() {
    let (mut h, _f) = harness(Lang::En, Some(fresh()), true);
    h.state_mut()
        .state
        .apply_reply(Reply::Notice("Freed 1.0 MiB".into()));
    h.run();
    h.get_by_label("Freed 1.0 MiB");
    // 失敗ではないので、珊瑚色の警告にしない。
    assert!(h.state().state.toasts[0].info);
    h.get_by_label("Dismiss").click();
    h.run();
    assert!(h.query_by_label("Freed 1.0 MiB").is_none());
}

#[test]
fn notices_and_errors_are_told_apart_and_deduplicated() {
    let mut s = GuiState::new();
    s.push_notice("Freed 0 B".into());
    s.push_notice("Freed 0 B".into());
    s.push_toast("nope".into());
    let kinds: Vec<(&str, bool)> = s
        .toasts
        .iter()
        .map(|t| (t.message.as_str(), t.info))
        .collect();
    assert_eq!(kinds, [("Freed 0 B", true), ("nope", false)]);
}

#[test]
fn start_pending_marks_the_resource_busy_and_builds_the_request() {
    let mut s = GuiState::new();
    let cases = [
        (
            PendingAction::Image {
                profile: "dev".into(),
                id: "nginx:alpine".into(),
                force: true,
                in_use: true,
                size: 7,
            },
            "image:nginx:alpine",
            Request::RemoveImage {
                profile: "dev".into(),
                id: "nginx:alpine".into(),
                force: true,
                size: 7,
            },
        ),
        (
            PendingAction::Volume {
                profile: "dev".into(),
                name: "data-1".into(),
                size: Some(3),
            },
            "volume:data-1",
            Request::RemoveVolume {
                profile: "dev".into(),
                name: "data-1".into(),
                size: Some(3),
            },
        ),
        (
            PendingAction::Network {
                profile: "dev".into(),
                id: "abc1".into(),
            },
            "network:abc1",
            Request::RemoveNetwork {
                profile: "dev".into(),
                id: "abc1".into(),
            },
        ),
        (
            PendingAction::PruneVolumes {
                profile: "dev".into(),
            },
            "prune-volumes:dev",
            Request::PruneVolumes {
                profile: "dev".into(),
            },
        ),
        (
            PendingAction::PruneNetworks {
                profile: "dev".into(),
            },
            "prune-networks:dev",
            Request::PruneNetworks {
                profile: "dev".into(),
            },
        ),
    ];
    for (action, key, request) in cases {
        assert_eq!(s.start_pending(action), request);
        assert!(s.busy_ops.contains(key), "{key}");
    }
    assert_eq!(s.busy_ops.len(), 5);
}

#[test]
fn pull_progress_reaches_the_pull_state_only_while_one_is_open() {
    use crate::docker::actions::PullProgress;
    let progress = |current| Reply::PullProgress {
        profile: "dev".into(),
        progress: PullProgress {
            id: Some("l1".into()),
            status: "Downloading".into(),
            current: Some(current),
            total: Some(10),
        },
    };
    let mut s = GuiState::new();
    s.selected = Some("dev".into());
    s.apply_reply(progress(5));
    assert!(s.images.pull.is_none());
    s.images.pull = Some(Default::default());
    s.apply_reply(progress(5));
    assert_eq!(s.images.pull.as_ref().unwrap().progress(), Some(0.5));
}

#[test]
fn the_force_choice_appears_only_for_an_image_in_use() {
    for (in_use, offered) in [(true, true), (false, false)] {
        let confirm = Some(Confirm {
            message: "Delete image x?".into(),
            action: PendingAction::Image {
                profile: "dev".into(),
                id: "x:1".into(),
                force: false,
                in_use,
                size: 1,
            },
        });
        let mut h = Harness::builder()
            .with_context_setup(|ctx| super::install_fonts(ctx, Lang::En))
            .build_ui_state(
                |ui, confirm: &mut Option<Confirm>| {
                    confirm::show(ui, confirm, Lang::En);
                },
                confirm,
            );
        h.run();
        let found = h.query_by_label("Delete even if in use (force)").is_some();
        assert_eq!(found, offered);
    }
}

#[test]
fn the_preferred_instance_is_selected_when_it_exists() {
    let mut s = GuiState::new();
    s.preferred = Some("old".into());
    s.apply_snapshot(Some(fresh()));
    assert_eq!(s.selected.as_deref(), Some("old"));
    // 無い名前は無視して、既定の選び方に戻る。
    let mut s = GuiState::new();
    s.preferred = Some("nope".into());
    s.apply_snapshot(Some(fresh()));
    assert_eq!(s.selected.as_deref(), Some("dev"));
    assert_eq!(GuiState::new().preferred, None);
}

#[test]
fn app_with_preferred_passes_the_name_on() {
    let (bridge, _ends) = Bridge::pair();
    let app = App::new(
        Lang::En,
        std::path::PathBuf::from("state.json"),
        bridge,
        Box::new(|| Ok(())),
    )
    .with_preferred(Some("old".into()));
    assert_eq!(app.state.preferred.as_deref(), Some("old"));
}

#[test]
fn window_opens_at_the_size_the_dashboard_fits_without_a_minimize_button() {
    let options = super::native_options();
    assert_eq!(
        options.viewport.inner_size,
        Some(eframe::egui::vec2(
            super::WINDOW_SIZE[0],
            super::WINDOW_SIZE[1]
        ))
    );
    assert_eq!(options.viewport.title.as_deref(), Some("wharfkeep"));
    assert_eq!(options.viewport.minimize_button, Some(false));
    assert!(options.event_loop_builder.is_some());
}
