//! 画面の試験で使う部品。

use crate::colima::{Instance, InstanceStatus};
use crate::gui::bridge::{Bridge, WorkerEnds};
use crate::gui::confirm;
use crate::gui::state::{GuiState, Page};
use crate::i18n::Lang;
use crate::snapshot::{ContainerRow, DockerState, DockerSummary, Snapshot};
use egui_kittest::Harness;
use std::collections::BTreeMap;
use std::time::SystemTime;

pub fn instance(name: &str, status: InstanceStatus, cpus: u32, mem_gib: u64) -> Instance {
    Instance {
        name: name.into(),
        status,
        arch: "aarch64".into(),
        cpus,
        memory_bytes: mem_gib << 30,
        disk_bytes: 60 << 30,
        runtime: "docker".into(),
    }
}

pub fn row(id: &str, name: &str, state: &str, failed: bool) -> ContainerRow {
    ContainerRow {
        id: id.into(),
        name: name.into(),
        image: "alpine:latest".into(),
        image_id: String::new(),
        state: state.into(),
        status: String::new(),
        ports: String::new(),
        failed,
    }
}

/// 実行中の`dev`を選んだ状態。`summary`がそのDockerの中身。
pub fn state_with(summary: DockerSummary, page: Page) -> GuiState {
    let mut state = GuiState::new();
    state.apply_snapshot(Some(Snapshot {
        updated_at: SystemTime::now(),
        instances: vec![instance("dev", InstanceStatus::Running, 4, 8)],
        docker: BTreeMap::from([("dev".to_string(), DockerState::Ready(summary))]),
    }));
    state.page = page;
    state
}

/// Readyの`dev`と`prod`があり、`prod`を選んだ状態。`summary`は`prod`のDockerの中身。
pub fn state_on_prod(summary: DockerSummary, page: Page) -> GuiState {
    let mut state = GuiState::new();
    state.apply_snapshot(Some(Snapshot {
        updated_at: SystemTime::now(),
        instances: vec![
            instance("dev", InstanceStatus::Running, 4, 8),
            instance("prod", InstanceStatus::Running, 4, 8),
        ],
        docker: BTreeMap::from([
            (
                "dev".to_string(),
                DockerState::Ready(DockerSummary::default()),
            ),
            ("prod".to_string(), DockerState::Ready(summary)),
        ]),
    }));
    state.selected = Some("prod".into());
    state.page = page;
    state
}

pub fn containers_state(rows: Vec<ContainerRow>) -> GuiState {
    state_with(
        DockerSummary {
            containers: rows,
            ..Default::default()
        },
        Page::Containers,
    )
}

pub type Fixture = (GuiState, Bridge);

pub fn harness(lang: Lang, state: GuiState) -> (Harness<'static, Fixture>, WorkerEnds) {
    harness_sized(lang, state, 900.0)
}

pub fn harness_sized(
    lang: Lang,
    state: GuiState,
    width: f32,
) -> (Harness<'static, Fixture>, WorkerEnds) {
    harness_with_size(lang, state, [width, 600.0])
}

pub fn harness_with_size(
    lang: Lang,
    state: GuiState,
    size: [f32; 2],
) -> (Harness<'static, Fixture>, WorkerEnds) {
    let (bridge, ends) = Bridge::pair();
    let mut h = Harness::builder()
        .with_size(size)
        .with_context_setup(move |ctx| crate::gui::install_fonts(ctx, lang))
        .build_ui_state(
            move |ui, (state, bridge): &mut Fixture| {
                super::show(ui, state, bridge, lang);
                if let Some(action) = confirm::show(ui, &mut state.confirm, lang) {
                    let request = state.start_pending(action);
                    bridge.send(request);
                }
            },
            (state, bridge),
        );
    h.run_steps(4);
    (h, ends)
}

pub fn requests(ends: &mut WorkerEnds) -> Vec<crate::gui::bridge::Request> {
    std::iter::from_fn(|| ends.requests.try_recv().ok()).collect()
}
