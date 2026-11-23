use super::super::test_util::{containers_state, harness, requests, row};
use super::*;
use crate::colima::Action;
use crate::gui::bridge::{Reply, WorkerEnds};
use crate::gui::state::Page;
use egui_kittest::kittest::{NodeT, Queryable};

fn container_request(id: &str, op: ContainerOp) -> Request {
    Request::Container {
        profile: "dev".into(),
        id: id.into(),
        op,
    }
}

fn rgb(hex: u32) -> egui::Color32 {
    egui::Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

#[test]
fn status_bar_color() {
    let tone = |state: &str, failed| status_tone(&row("c1", "web", state, failed));
    assert_eq!(tone("running", true), StatusTone::Failed);
    assert_eq!(tone("exited", true), StatusTone::Failed);
    assert_eq!(tone("running", false), StatusTone::Running);
    assert_eq!(tone("restarting", false), StatusTone::Running);
    assert_eq!(tone("paused", false), StatusTone::Paused);
    assert_eq!(tone("exited", false), StatusTone::Stopped);
    assert_eq!(tone("created", false), StatusTone::Stopped);
    assert_eq!(tone("dead", false), StatusTone::Stopped);
    assert_eq!(StatusTone::Failed.color(), rgb(0xD2665A));
    assert_eq!(StatusTone::Running.color(), rgb(0x3FA66B));
    assert_eq!(StatusTone::Paused.color(), rgb(0xD9A441));
    assert_eq!(StatusTone::Stopped.color(), rgb(0x8A8F98));
}

#[test]
fn status_text_names_every_state_in_both_languages() {
    let text = |state: &str, failed, lang| status_text(&row("c1", "web", state, failed), lang);
    let cases = [
        ("running", "Running", "実行中"),
        ("paused", "Paused", "一時停止中"),
        ("exited", "Stopped", "停止中"),
        ("created", "Created", "作成済み"),
        ("restarting", "Restarting", "再起動中"),
    ];
    for (state, en, ja) in cases {
        assert_eq!(text(state, false, Lang::En), en);
        assert_eq!(text(state, false, Lang::Ja), ja);
    }
    assert_eq!(text("running", true, Lang::En), "Failed");
    assert_eq!(text("running", true, Lang::Ja), "失敗");
    assert_eq!(text("removing", false, Lang::En), "removing");
}

#[test]
fn container_filter_by_name() {
    let rows = [
        row("1", "web-1", "running", false),
        row("2", "db", "running", false),
        row("3", "my-Web", "exited", false),
    ];
    let names = |q: &str| {
        filter_rows(&rows, q)
            .iter()
            .map(|r| r.name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(names("web"), ["web-1", "my-Web"]);
    assert_eq!(names(""), ["web-1", "db", "my-Web"]);
    assert!(names("zzz").is_empty());
}

#[test]
fn row_actions_by_state() {
    use RowAction::*;
    assert_eq!(
        row_actions("running"),
        [Stop, Pause, Restart, Logs, Shell, Rename, Delete]
    );
    assert_eq!(row_actions("exited"), [Start, Logs, Shell, Rename, Delete]);
    assert_eq!(row_actions("created"), [Start, Logs, Shell, Rename, Delete]);
    assert_eq!(
        row_actions("paused"),
        [Unpause, Logs, Shell, Rename, Delete]
    );
    assert_eq!(
        row_actions("restarting"),
        [Stop, Logs, Shell, Rename, Delete]
    );
}

#[test]
fn row_icons_are_set_and_tell_the_actions_apart() {
    use RowAction::*;
    let icons: Vec<&str> = [Start, Stop, Pause, Restart, Logs, Shell, Rename, Delete]
        .iter()
        .map(|a| a.icon())
        .collect();
    assert!(icons.iter().all(|i| !i.is_empty()));
    let unique: std::collections::HashSet<_> = icons.iter().collect();
    assert_eq!(unique.len(), icons.len());
    // 再開は開始と同じ三角で足りる。
    assert_eq!(Unpause.icon(), Start.icon());
}

#[test]
fn row_actions_map_to_container_operations() {
    use RowAction::*;
    let ops = [
        (Start, ContainerOp::Start),
        (Stop, ContainerOp::Stop),
        (Pause, ContainerOp::Pause),
        (Unpause, ContainerOp::Unpause),
        (Restart, ContainerOp::Restart),
    ];
    for (action, op) in ops {
        assert_eq!(action.op(), Some(op));
    }
    for action in [Logs, Shell, Rename, Delete] {
        assert_eq!(action.op(), None);
    }
}

#[test]
fn row_shows_status_text_with_bar() {
    for (lang, running, failed) in [
        (Lang::En, "Running", "Failed"),
        (Lang::Ja, "実行中", "失敗"),
    ] {
        let state = containers_state(vec![
            row("c1", "web", "running", false),
            row("c2", "bad", "exited", true),
        ]);
        let (h, _ends) = harness(lang, state);
        h.get_by_label(running);
        h.get_by_label(failed);
    }
}

#[test]
fn row_buttons_are_one_click() {
    let state = containers_state(vec![row("c1", "web", "running", false)]);
    let (mut h, mut ends) = harness(Lang::En, state);
    h.get_by_label("Stop web").click();
    h.run_steps(4);
    assert_eq!(
        requests(&mut ends),
        [container_request("c1", ContainerOp::Stop)]
    );
    assert!(h.state().0.busy_ops.contains("c1"));
}

#[test]
fn every_operation_button_sends_its_operation() {
    let cases = [
        ("running", "Pause web", ContainerOp::Pause),
        ("running", "Restart web", ContainerOp::Restart),
        ("exited", "Start web", ContainerOp::Start),
        ("paused", "Resume web", ContainerOp::Unpause),
    ];
    for (state, label, op) in cases {
        let (mut h, mut ends) = harness(
            Lang::En,
            containers_state(vec![row("c1", "web", state, false)]),
        );
        h.get_by_label(label).click();
        h.run_steps(4);
        assert_eq!(
            requests(&mut ends),
            [container_request("c1", op)],
            "{label}"
        );
    }
}

#[test]
fn row_busy_disables_buttons() {
    let state = containers_state(vec![row("c1", "web", "running", false)]);
    let (mut h, mut ends) = harness(Lang::En, state);
    h.get_by_label("Stop web").click();
    h.run_steps(4);
    assert_eq!(requests(&mut ends).len(), 1);
    for label in [
        "Stop web",
        "Pause web",
        "Restart web",
        "Logs web",
        "Shell web",
        "Delete web",
    ] {
        assert!(
            h.get_by_label(label).accesskit_node().is_disabled(),
            "{label}"
        );
    }
    h.get_by_label("Stop web").click();
    h.run_steps(4);
    assert!(requests(&mut ends).is_empty());
    h.state_mut().0.apply_reply(Reply::OpDone {
        key: "c1".into(),
        error: None,
    });
    h.run_steps(4);
    assert!(!h.get_by_label("Stop web").accesskit_node().is_disabled());
    h.get_by_label("Stop web").click();
    h.run_steps(4);
    assert_eq!(requests(&mut ends).len(), 1);
}

#[test]
fn only_the_pressed_row_is_busy() {
    let state = containers_state(vec![
        row("c1", "web", "running", false),
        row("c2", "db", "running", false),
    ]);
    let (mut h, _ends) = harness(Lang::En, state);
    h.get_by_label("Stop web").click();
    h.run_steps(4);
    assert!(h.get_by_label("Stop web").accesskit_node().is_disabled());
    assert!(!h.get_by_label("Stop db").accesskit_node().is_disabled());
}

#[test]
fn row_buttons_have_accessible_names() {
    for lang in [Lang::En, Lang::Ja] {
        for state in ["running", "exited", "paused", "restarting"] {
            let (h, _ends) = harness(lang, containers_state(vec![row("c1", "web", state, false)]));
            for action in row_actions(state) {
                let name = format!("{} web", action.label().text(lang));
                assert!(!action.label().text(lang).is_empty());
                h.get_by_label(&name);
            }
        }
    }
    let (h, _ends) = harness(
        Lang::En,
        containers_state(vec![row("c1", "web", "running", false)]),
    );
    for name in [
        "Stop web",
        "Pause web",
        "Restart web",
        "Logs web",
        "Shell web",
        "Delete web",
    ] {
        h.get_by_label(name);
    }
}

#[test]
fn running_container_delete_uses_force_wording() {
    for (lang, wording) in [(Lang::Ja, "停止して削除"), (Lang::En, "Stop and delete")] {
        let state = containers_state(vec![row("c1", "web", "running", false)]);
        let (mut h, mut ends) = harness(lang, state);
        let button = format!("{} web", Msg::Delete.text(lang));
        h.get_by_label(&button).click();
        h.run_steps(4);
        let confirm = h.state().0.confirm.clone().unwrap();
        assert!(confirm.message.contains(wording), "{}", confirm.message);
        assert!(confirm.message.contains("web"));
        assert_eq!(
            confirm.action,
            PendingAction::Container {
                profile: "dev".into(),
                id: "c1".into(),
                force: true
            }
        );
        assert!(requests(&mut ends).is_empty());
    }
}

#[test]
fn stopped_container_delete_is_not_forced_and_needs_the_confirmation() {
    let state = containers_state(vec![row("c1", "web", "exited", false)]);
    let (mut h, mut ends) = harness(Lang::En, state);
    h.get_by_label("Delete web").click();
    h.run_steps(4);
    let confirm = h.state().0.confirm.clone().unwrap();
    assert_eq!(confirm.message, "Delete web?");
    assert!(requests(&mut ends).is_empty());
    h.get_by_label("Delete").click();
    h.run_steps(4);
    h.run_steps(4);
    assert_eq!(
        requests(&mut ends),
        [container_request(
            "c1",
            ContainerOp::Remove { force: false }
        )]
    );
    assert!(h.state().0.busy_ops.contains("c1"));
}

#[test]
fn a_running_container_is_removed_with_force_after_the_confirmation() {
    let state = containers_state(vec![row("c1", "web", "running", false)]);
    let (mut h, mut ends) = harness(Lang::En, state);
    h.get_by_label("Delete web").click();
    h.run_steps(4);
    h.get_by_label("Delete").click();
    h.run_steps(4);
    h.run_steps(4);
    assert_eq!(
        requests(&mut ends),
        [container_request("c1", ContainerOp::Remove { force: true })]
    );
}

#[test]
fn force_is_needed_unless_the_container_is_stopped() {
    assert!(needs_force("running"));
    assert!(needs_force("paused"));
    assert!(needs_force("restarting"));
    assert!(!needs_force("exited"));
    assert!(!needs_force("created"));
}

#[test]
fn shell_button_opens_a_shell_for_the_container() {
    let state = containers_state(vec![row("c1", "web", "running", false)]);
    let (mut h, mut ends) = harness(Lang::En, state);
    h.get_by_label("Shell web").click();
    h.run_steps(4);
    assert_eq!(
        requests(&mut ends),
        [Request::Shell {
            profile: "dev".into(),
            container: Some("c1".into())
        }]
    );
    assert!(!h.state().0.busy_ops.contains("c1"));
}

#[test]
fn filter_box_narrows_the_rows() {
    let state = containers_state(vec![
        row("c1", "web", "running", false),
        row("c2", "db", "running", false),
    ]);
    let (mut h, _ends) = harness(Lang::En, state);
    h.state_mut().0.containers.filter = "web".into();
    h.run_steps(4);
    h.get_by_label("Stop web");
    assert!(h.query_by_label("Stop db").is_none());
}

#[test]
fn empty_list_says_so() {
    let (h, _ends) = harness(Lang::En, containers_state(vec![]));
    h.get_by_label("No containers");
}

#[test]
fn logs_button_first_reads_the_logs_once_without_opening_the_detail() {
    let state = containers_state(vec![row("c1", "web", "running", false)]);
    let (mut h, mut ends) = harness(Lang::En, state);
    h.get_by_label("Logs web").click();
    h.run_steps(4);
    assert_eq!(
        requests(&mut ends),
        [Request::Logs {
            profile: "dev".into(),
            id: "c1".into(),
            follow: false
        }]
    );
    assert!(h.state().0.containers.detail.is_none());
    assert!(h.query_by_label("Follow").is_none());
}

#[test]
fn logs_with_lines_open_the_detail_and_ask_for_logs_inspect_and_usage() {
    let state = containers_state(vec![row("c1", "web", "running", false)]);
    let (mut h, mut ends) = harness(Lang::En, state);
    h.get_by_label("Logs web").click();
    h.run_steps(4);
    requests(&mut ends);
    let state = &mut h.state_mut().0;
    state.apply_reply(Reply::LogLine {
        id: "c1".into(),
        line: "hello".into(),
    });
    state.apply_reply(Reply::LogsEnded { id: "c1".into() });
    h.run_steps(4);
    let sent = requests(&mut ends);
    assert_eq!(
        sent,
        [
            Request::Logs {
                profile: "dev".into(),
                id: "c1".into(),
                follow: true
            },
            Request::Inspect {
                profile: "dev".into(),
                id: "c1".into()
            },
            Request::Usage {
                profile: "dev".into(),
                id: "c1".into()
            },
        ]
    );
    let detail = h.state().0.containers.detail.as_ref().unwrap();
    assert_eq!((detail.id.as_str(), detail.follow), ("c1", true));
    assert!(h.state().0.containers.probe.is_none());
    h.get_by_label("Follow");
    h.get_by_label("Refresh usage");
    h.get_by_label("Close");
}

#[test]
fn logs_without_lines_show_a_dialog_instead_of_the_detail() {
    let state = containers_state(vec![row("c1", "web", "running", false)]);
    let (mut h, mut ends) = harness(Lang::En, state);
    h.get_by_label("Logs web").click();
    h.run_steps(4);
    requests(&mut ends);
    h.state_mut()
        .0
        .apply_reply(Reply::LogsEnded { id: "c1".into() });
    h.run_steps(4);
    assert!(requests(&mut ends).is_empty());
    assert!(h.state().0.containers.detail.is_none());
    h.get_by_label("web has no logs to show.");
    assert!(h.query_by_label("Follow").is_none());
    h.get_by_label("Close").click();
    h.run_steps(4);
    assert!(h.query_by_label("web has no logs to show.").is_none());
    assert!(h.state().0.containers.no_logs.is_none());
}

#[test]
fn the_no_logs_dialog_is_in_japanese_too() {
    let state = containers_state(vec![row("c1", "web", "running", false)]);
    let (mut h, _ends) = harness(Lang::Ja, state);
    h.get_by_label("ログ web").click();
    h.run_steps(4);
    h.state_mut()
        .0
        .apply_reply(Reply::LogsEnded { id: "c1".into() });
    h.run_steps(4);
    h.get_by_label("web には表示できるログがありません。");
    h.get_by_label("閉じる");
}

#[test]
fn the_end_of_another_containers_logs_does_not_settle_the_probe() {
    let state = containers_state(vec![row("c1", "web", "running", false)]);
    let (mut h, mut ends) = harness(Lang::En, state);
    h.get_by_label("Logs web").click();
    h.run_steps(4);
    requests(&mut ends);
    let state = &mut h.state_mut().0;
    state.apply_reply(Reply::LogLine {
        id: "other".into(),
        line: "stray".into(),
    });
    state.apply_reply(Reply::LogsEnded { id: "other".into() });
    h.run_steps(4);
    let probe = h.state().0.containers.probe.as_ref().unwrap();
    assert_eq!((probe.seen, probe.ended), (false, false));
    assert!(h.state().0.containers.no_logs.is_none());
}

#[test]
fn settle_decides_only_once_the_read_has_ended() {
    let probe = |seen, ended| LogProbe {
        id: "c1".into(),
        name: "web".into(),
        seen,
        ended,
    };
    let mut pending = Some(probe(true, false));
    assert_eq!(settle(&mut pending), None);
    assert_eq!(pending, Some(probe(true, false)));
    let mut with_lines = Some(probe(true, true));
    assert_eq!(
        settle(&mut with_lines),
        Some((probe(true, true), ProbeOutcome::Open))
    );
    assert_eq!(with_lines, None);
    let mut empty = Some(probe(false, true));
    assert_eq!(
        settle(&mut empty),
        Some((probe(false, true), ProbeOutcome::NoLogs))
    );
    assert_eq!(settle(&mut None), None);
}

/// Logsを押し、1行あるログを読み終えて詳細を開いたところ。
fn open_detail_harness() -> (
    egui_kittest::Harness<'static, super::super::test_util::Fixture>,
    WorkerEnds,
) {
    let state = containers_state(vec![row("c1", "web", "running", false)]);
    let (mut h, mut ends) = harness(Lang::En, state);
    h.get_by_label("Logs web").click();
    h.run_steps(4);
    let state = &mut h.state_mut().0;
    state.apply_reply(Reply::LogLine {
        id: "c1".into(),
        line: "first".into(),
    });
    state.apply_reply(Reply::LogsEnded { id: "c1".into() });
    h.run_steps(4);
    requests(&mut ends);
    (h, ends)
}

#[test]
fn detail_shows_log_lines_inspect_and_usage_from_replies() {
    let (mut h, _ends) = open_detail_harness();
    let state = &mut h.state_mut().0;
    state.apply_reply(Reply::LogLine {
        id: "c1".into(),
        line: "hello from web".into(),
    });
    state.apply_reply(Reply::Inspected {
        id: "c1".into(),
        text: "{\"Id\": \"c1\"}".into(),
    });
    state.apply_reply(Reply::UsageSample {
        id: "c1".into(),
        usage: Usage {
            cpu_percent: 12.34,
            mem_bytes: 3 << 20,
            mem_limit: 1 << 30,
        },
    });
    h.run_steps(4);
    h.get_by_label("hello from web");
    h.get_by_label("CPU 12.3% / memory 3.0 MiB of 1.0 GiB");
    h.get_by_label("Inspect").click();
    h.run_steps(4);
    h.get_by_label("{\"Id\": \"c1\"}");
}

#[test]
fn replies_for_another_container_do_not_touch_the_detail() {
    let (mut h, _ends) = open_detail_harness();
    let state = &mut h.state_mut().0;
    state.apply_reply(Reply::LogLine {
        id: "other".into(),
        line: "stray".into(),
    });
    state.apply_reply(Reply::Inspected {
        id: "other".into(),
        text: "stray".into(),
    });
    state.apply_reply(Reply::UsageSample {
        id: "other".into(),
        usage: Usage {
            cpu_percent: 1.0,
            mem_bytes: 1,
            mem_limit: 1,
        },
    });
    let d = state.containers.detail.as_ref().unwrap();
    assert!(d.log.is_empty() && d.inspect.is_none() && d.usage.is_none());
}

#[test]
fn follow_toggle_restarts_the_log_stream() {
    let (mut h, mut ends) = open_detail_harness();
    h.state_mut().0.apply_reply(Reply::LogLine {
        id: "c1".into(),
        line: "old".into(),
    });
    h.get_by_label("Follow").click();
    h.run_steps(4);
    assert_eq!(
        requests(&mut ends),
        [Request::Logs {
            profile: "dev".into(),
            id: "c1".into(),
            follow: false
        }]
    );
    let d = h.state().0.containers.detail.as_ref().unwrap();
    assert!(!d.follow);
    assert!(d.log.is_empty());
}

#[test]
fn refresh_usage_asks_again() {
    let (mut h, mut ends) = open_detail_harness();
    h.get_by_label("Refresh usage").click();
    h.run_steps(4);
    assert_eq!(
        requests(&mut ends),
        [Request::Usage {
            profile: "dev".into(),
            id: "c1".into()
        }]
    );
}

#[test]
fn close_stops_the_log_stream_and_hides_the_detail() {
    let (mut h, mut ends) = open_detail_harness();
    h.get_by_label("Close").click();
    h.run_steps(4);
    assert_eq!(requests(&mut ends), [Request::StopLogs { id: "c1".into() }]);
    assert!(h.state().0.containers.detail.is_none());
    assert!(h.query_by_label("Close").is_none());
}

#[test]
fn opening_another_detail_stops_the_previous_stream() {
    let state = containers_state(vec![
        row("c1", "web", "running", false),
        row("c2", "db", "running", false),
    ]);
    let (mut h, mut ends) = harness(Lang::En, state);
    let read = |h: &mut egui_kittest::Harness<'static, super::super::test_util::Fixture>,
                id: &str| {
        let state = &mut h.state_mut().0;
        state.apply_reply(Reply::LogLine {
            id: id.into(),
            line: "x".into(),
        });
        state.apply_reply(Reply::LogsEnded { id: id.into() });
        h.run_steps(4);
    };
    h.get_by_label("Logs web").click();
    h.run_steps(4);
    read(&mut h, "c1");
    requests(&mut ends);
    h.get_by_label("Logs db").click();
    h.run_steps(4);
    let sent = requests(&mut ends);
    assert_eq!(sent[0], Request::StopLogs { id: "c1".into() });
    assert!(matches!(&sent[1], Request::Logs { id, follow: false, .. } if id == "c2"));
    assert!(h.state().0.containers.detail.is_none());
    read(&mut h, "c2");
    assert_eq!(h.state().0.containers.detail.as_ref().unwrap().id, "c2");
}

fn form(image: &str, name: &str, ports: &str) -> RunForm {
    RunForm {
        image: image.into(),
        name: name.into(),
        ports: ports.into(),
        ..Default::default()
    }
}

#[test]
fn run_form_takes_env_mounts_restart_and_command() {
    use crate::docker::actions::{Mount, RestartPolicy};
    let spec = RunForm {
        env: "A=1\n\n  B=x=y  \n".into(),
        mounts: "/host:/data:ro\nvol:/v\n".into(),
        restart: RestartPolicy::OnFailure,
        command: " sh  -c date ".into(),
        ..form("alpine", "", "")
    }
    .spec()
    .unwrap();
    assert_eq!(
        spec.env,
        [("A".into(), "1".into()), ("B".into(), "x=y".into())]
    );
    assert_eq!(
        spec.mounts,
        [
            Mount {
                source: "/host".into(),
                target: "/data".into(),
                read_only: true
            },
            Mount {
                source: "vol".into(),
                target: "/v".into(),
                read_only: false
            }
        ]
    );
    assert_eq!(spec.restart, RestartPolicy::OnFailure);
    assert_eq!(
        spec.cmd,
        Some(vec!["sh".into(), "-c".into(), "date".into()])
    );
    let bare = form("alpine", "", "").spec().unwrap();
    assert_eq!(bare.cmd, None);
    assert!(bare.env.is_empty() && bare.mounts.is_empty());
    assert_eq!(bare.restart, RestartPolicy::No);
}

#[test]
fn run_form_rejects_a_bad_env_or_mount() {
    let with = |env: &str, mounts: &str| RunForm {
        env: env.into(),
        mounts: mounts.into(),
        ..form("alpine", "", "")
    };
    assert_eq!(
        with("noequals", "").spec(),
        Err(crate::docker::DockerError::InvalidEnv("noequals".into()))
    );
    assert_eq!(
        with("", "src:relative").spec(),
        Err(crate::docker::DockerError::InvalidMount(
            "src:relative".into()
        ))
    );
}

#[test]
fn run_form_fields_are_labelled_in_both_languages() {
    for lang in [Lang::En, Lang::Ja] {
        let (h, _ends) = open_run_form(lang, ("", "", ""));
        for msg in [
            Msg::FieldEnv,
            Msg::FieldMounts,
            Msg::FieldRestart,
            Msg::FieldCommand,
        ] {
            h.get_by_label(msg.text(lang));
        }
    }
}

#[test]
fn run_form_sends_the_restart_policy_chosen_in_the_list() {
    use crate::docker::actions::RestartPolicy;
    let (mut h, mut ends) = open_run_form(Lang::En, ("alpine", "", ""));
    h.get_by_label("Restart policy").click();
    h.run_steps(4);
    h.get_by_label("unless-stopped").click();
    h.run_steps(4);
    h.get_by_label("Run").click();
    h.run_steps(4);
    let sent = requests(&mut ends);
    let [Request::Run { spec, .. }] = sent.as_slice() else {
        panic!("{sent:?}");
    };
    assert_eq!(spec.restart, RestartPolicy::UnlessStopped);
}

#[test]
fn run_form_builds_a_spec() {
    let spec = form(" alpine:latest ", " web-1 ", "8080:80, 5353:53/udp 9000:90")
        .spec()
        .unwrap();
    assert_eq!(spec.image, "alpine:latest");
    assert_eq!(spec.name.as_deref(), Some("web-1"));
    assert_eq!(spec.ports.len(), 3);
    assert_eq!((spec.ports[0].host, spec.ports[0].container), (8080, 80));
    let bare = form("alpine", "", "").spec().unwrap();
    assert_eq!(bare.name, None);
    assert!(bare.ports.is_empty());
}

#[test]
fn run_form_rejects_bad_input() {
    assert!(form("", "", "").spec().is_err());
    assert!(form("-evil", "", "").spec().is_err());
    assert!(form("alpine", "a/b", "").spec().is_err());
    assert!(form("alpine", "x", "").spec().is_err());
    assert!(form("alpine", "", "notaport").spec().is_err());
}

fn open_run_form(
    lang: Lang,
    fields: (&str, &str, &str),
) -> (
    egui_kittest::Harness<'static, super::super::test_util::Fixture>,
    WorkerEnds,
) {
    let (mut h, ends) = harness(lang, containers_state(vec![]));
    h.get_by_label(Msg::RunTitle.text(lang)).click();
    h.run_steps(4);
    h.state_mut().0.containers.run = form(fields.0, fields.1, fields.2);
    h.run_steps(4);
    (h, ends)
}

#[test]
fn run_form_sends_the_spec_and_marks_the_form_busy() {
    let (mut h, mut ends) = open_run_form(Lang::En, ("alpine:latest", "wk-run", "8080:80"));
    h.get_by_label("Run").click();
    h.run_steps(4);
    let sent = requests(&mut ends);
    let [Request::Run { profile, spec }] = sent.as_slice() else {
        panic!("{sent:?}");
    };
    assert_eq!(profile, "dev");
    assert_eq!(spec.image, "alpine:latest");
    assert!(h.state().0.busy_ops.contains(&run_key("dev")));
    assert!(h.get_by_label("Run").accesskit_node().is_disabled());
}

#[test]
fn run_form_is_blocked_and_explains_a_bad_port() {
    let (mut h, mut ends) = open_run_form(Lang::En, ("alpine", "", "notaport"));
    assert!(h.get_by_label("Run").accesskit_node().is_disabled());
    h.get_by_label("Run").click();
    h.run_steps(4);
    assert!(requests(&mut ends).is_empty());
    let reason = form("alpine", "", "notaport")
        .spec()
        .unwrap_err()
        .message(Lang::En);
    h.get_by_label(&reason);
}

#[test]
fn empty_run_form_is_blocked_without_a_complaint() {
    let (h, _ends) = open_run_form(Lang::En, ("", "", ""));
    assert!(h.get_by_label("Run").accesskit_node().is_disabled());
    assert!(h.query_by_label_contains("invalid").is_none());
}

#[test]
fn other_pages_do_not_draw_the_container_list() {
    let mut state = containers_state(vec![row("c1", "web", "running", false)]);
    state.page = Page::Images;
    let (h, _ends) = harness(Lang::En, state);
    assert!(h.query_by_label("Stop web").is_none());
}

#[test]
fn delete_of_an_instance_is_not_confused_with_a_container() {
    assert_eq!(
        PendingAction::Colima {
            action: Action::Delete,
            profile: "dev".into()
        }
        .into_request(),
        Request::Colima {
            action: Action::Delete,
            profile: "dev".into()
        }
    );
}

#[test]
fn rename_opens_a_form_and_sends_the_new_name() {
    let (mut h, mut ends) = harness(
        Lang::En,
        containers_state(vec![row("c1", "web", "running", false)]),
    );
    h.get_by_label("Rename web").click();
    h.run_steps(4);
    assert!(requests(&mut ends).is_empty());
    assert_eq!(
        h.state().0.containers.rename.as_ref().unwrap().new_name,
        "web"
    );
    h.state_mut().0.containers.rename.as_mut().unwrap().new_name = " web-2 ".into();
    h.run_steps(4);
    h.get_by_label("Apply").click();
    h.run_steps(4);
    assert_eq!(
        requests(&mut ends),
        [Request::Rename {
            profile: "dev".into(),
            id: "c1".into(),
            name: "web-2".into()
        }]
    );
    assert!(h.state().0.busy_ops.contains("c1"));
    assert!(h.state().0.containers.rename.is_none());
}

#[test]
fn rename_blocks_a_bad_name_and_can_be_cancelled() {
    for lang in [Lang::En, Lang::Ja] {
        let (mut h, mut ends) = harness(
            lang,
            containers_state(vec![row("c1", "web", "exited", false)]),
        );
        let rename = format!("{} web", Msg::RowRename.text(lang));
        h.get_by_label(&rename).click();
        h.run_steps(4);
        h.get_by_label(Msg::FieldNewName.text(lang));
        h.state_mut().0.containers.rename.as_mut().unwrap().new_name = "a/b".into();
        h.run_steps(4);
        let apply = Msg::ApplyRename.text(lang);
        assert!(h.get_by_label(apply).accesskit_node().is_disabled());
        let reason = crate::docker::DockerError::InvalidId("a/b".into()).message(lang);
        h.get_by_label(&reason);
        h.get_by_label(apply).click();
        h.run_steps(4);
        h.get_by_label(Msg::Cancel.text(lang)).click();
        h.run_steps(4);
        assert!(h.state().0.containers.rename.is_none());
        assert!(requests(&mut ends).is_empty());
    }
}

#[test]
fn a_long_log_line_stays_on_one_line() {
    let (mut h, _ends) = open_detail_harness();
    let state = &mut h.state_mut().0;
    for line in ["short", &format!("long{}", "x".repeat(2_000))] {
        state.apply_reply(Reply::LogLine {
            id: "c1".into(),
            line: line.to_string(),
        });
    }
    h.run_steps(4);
    let short = h.get_by_label("short").rect();
    let long = h.get_by_label_contains("longxxxx").rect();
    // 切り詰めた行は1行ぶん以下の高さになる。折り返すと何行ぶんにもなる。
    assert!(long.height() <= short.height() + 0.5, "{long:?} {short:?}");
}
