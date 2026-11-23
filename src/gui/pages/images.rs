//! イメージ画面。一覧、pull、行ごとのごみ箱、片付け。

use super::{cleanup_button, format_bytes, inspect_button, inspect_panel, trash_button};
use crate::docker::actions::PullProgress;
use crate::docker::check_image_ref;
use crate::gui::bridge::{Bridge, Cleanup, Request, ResourceKind, image_key, pull_key, tag_key};
use crate::gui::confirm::{Confirm, PendingAction};
use crate::gui::state::GuiState;
use crate::i18n::{Lang, Msg};
use crate::snapshot::{ContainerRow, DockerSummary, ImageRow};
use eframe::egui;
use std::collections::BTreeMap;

/// 一覧の1行。タグごとに1行で、タグのないイメージはIDで1行。
#[derive(Debug, Clone, PartialEq)]
pub struct Entry<'a> {
    pub image: &'a ImageRow,
    /// 削除に渡すタグか、タグのないイメージのID。
    pub target: String,
    pub name: String,
}

fn short_id(id: &str) -> String {
    id.strip_prefix("sha256:")
        .unwrap_or(id)
        .chars()
        .take(12)
        .collect()
}

pub fn entries(images: &[ImageRow]) -> Vec<Entry<'_>> {
    images
        .iter()
        .flat_map(|image| {
            let tags: Vec<&String> = image
                .repo_tags
                .iter()
                .filter(|t| t.as_str() != "<none>:<none>")
                .collect();
            if tags.is_empty() {
                return vec![Entry {
                    image,
                    target: image.id.clone(),
                    name: format!("<none> {}", short_id(&image.id)),
                }];
            }
            tags.into_iter()
                .map(|tag| Entry {
                    image,
                    target: tag.clone(),
                    name: tag.clone(),
                })
                .collect()
        })
        .collect()
}

/// タグを省いて作ったコンテナのイメージは、`:latest`として数える。
fn with_tag(reference: &str) -> String {
    let last = reference.rsplit('/').next().unwrap_or(reference);
    if reference.is_empty() || last.contains([':', '@']) {
        reference.to_string()
    } else {
        format!("{reference}:latest")
    }
}

/// 短すぎるIDの前方一致は、別のイメージに当たるので使わない。
const MIN_ID_PREFIX: usize = 12;

fn uses(entry: &Entry, image: &str) -> bool {
    if image.is_empty() {
        return false;
    }
    if with_tag(image) == entry.target {
        return true;
    }
    let want = image.strip_prefix("sha256:").unwrap_or(image);
    let id = entry
        .image
        .id
        .strip_prefix("sha256:")
        .unwrap_or(&entry.image.id);
    want.len() >= MIN_ID_PREFIX && id.starts_with(want)
}

/// 停止中を含め、コンテナが使っているイメージ。使用中は、Dockerが`force`なしの削除を断る。
/// IDがあればIDだけで決める。参照の書き方（`docker.io/library/…`、`@sha256:…`）に左右されず、
/// タグが別のイメージへ移ったあとも元のイメージを指すため。
pub fn in_use(entry: &Entry, containers: &[ContainerRow]) -> bool {
    containers.iter().any(|c| {
        if c.image_id.is_empty() {
            uses(entry, &c.image)
        } else {
            c.image_id == entry.image.id
        }
    })
}

/// 作成日時を`utc_offset`秒ずらした日付（YYYY-MM-DD）。`created`が0以下なら「不明」。
pub fn date_text(created: i64, utc_offset: i64, lang: Lang) -> String {
    if created <= 0 {
        return Msg::Unknown.text(lang).into();
    }
    let (y, m, d) = civil_from_days((created + utc_offset).div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// 1970-01-01からの日数を暦の日付にする。Howard Hinnant の `civil_from_days`（chrono-Compatible
/// Low-Level Date Algorithms）と同じ計算。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// その時刻のこのMacの時差（秒）。取れなければ0（UTC）。
pub fn local_offset(t: i64) -> i64 {
    let time = t as libc::time_t;
    // SAFETY: `tm`はゼロで埋めた値で、`localtime_r`が書き込む。どちらのポインタも呼び出しの間だけ有効な
    // スタック上の値を指す。`localtime_r`は再入可能で、共有の静的な領域を使わない。
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&time, &mut tm).is_null() };
    if ok { tm.tm_gmtoff } else { 0 }
}

/// 層ごとの（現在値, 総量）から全体の割合。総量が分からなければ`None`。
pub fn overall_progress(layers: &[(u64, u64)]) -> Option<f32> {
    let total: u64 = layers.iter().map(|l| l.1).sum();
    if total == 0 {
        return None;
    }
    let done: u64 = layers.iter().map(|l| l.0.min(l.1)).sum();
    Some(done as f32 / total as f32)
}

#[derive(Debug, Default)]
pub struct PullState {
    layers: BTreeMap<String, (u64, u64)>,
    pub status: String,
}

impl PullState {
    /// 取得の割合だけを数える。展開（Extracting）の進捗は、割合を戻すので無視する。
    pub fn update(&mut self, p: &PullProgress) {
        if !p.status.is_empty() {
            self.status = p.status.clone();
        }
        let Some(id) = &p.id else { return };
        match (p.status.as_str(), p.current, p.total) {
            ("Downloading", Some(current), Some(total)) => {
                self.layers.insert(id.clone(), (current, total));
            }
            ("Download complete" | "Pull complete" | "Already exists", ..) => {
                if let Some(layer) = self.layers.get_mut(id) {
                    layer.0 = layer.1;
                }
            }
            _ => {}
        }
    }

    pub fn progress(&self) -> Option<f32> {
        let layers: Vec<(u64, u64)> = self.layers.values().copied().collect();
        overall_progress(&layers)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageCleanup {
    #[default]
    Keep,
    Dangling,
    Unused,
}

#[derive(Debug, Default)]
pub struct CleanupDialog {
    pub images: ImageCleanup,
    pub build_cache: bool,
}

impl CleanupDialog {
    /// 何も選んでいない。
    pub fn is_empty(&self) -> bool {
        self.images == ImageCleanup::Keep && !self.build_cache
    }

    pub fn requests(&self, profile: &str) -> Vec<Request> {
        let mut requests = Vec::new();
        if self.images != ImageCleanup::Keep {
            requests.push(Request::PruneImages {
                profile: profile.to_string(),
                dangling: self.images == ImageCleanup::Dangling,
            });
        }
        if self.build_cache {
            requests.push(Request::PruneBuildCache {
                profile: profile.to_string(),
            });
        }
        requests
    }
}

/// イメージに付ける新しいタグの入力。
#[derive(Debug)]
pub struct TagForm {
    /// 付け元のタグか、タグのないイメージのID。
    pub source: String,
    pub name: String,
    pub repo: String,
    pub tag: String,
}

impl TagForm {
    pub fn new(source: String, name: String) -> Self {
        TagForm {
            source,
            name,
            repo: String::new(),
            tag: "latest".into(),
        }
    }

    /// 付けられる入力か。`docker tag`と同じ文法（リポジトリは小文字のパス、タグは英数字か`_`で始まる）。
    pub fn check(&self) -> Result<(), crate::docker::DockerError> {
        crate::docker::check_repo(self.repo.trim())?;
        crate::docker::check_tag(self.tag.trim())
    }
}

#[derive(Debug, Default)]
pub struct View {
    pub tag: Option<TagForm>,
    pub reference: String,
    pub pull: Option<PullState>,
    pub cleanup: Option<CleanupDialog>,
}

pub fn show(
    ui: &mut egui::Ui,
    state: &mut GuiState,
    bridge: &Bridge,
    lang: Lang,
    profile: &str,
    summary: &DockerSummary,
) {
    pull_form(ui, state, bridge, lang, profile);
    let cleaning = [Cleanup::Images, Cleanup::BuildCache]
        .iter()
        .any(|c| state.busy_ops.contains(&c.key(profile)));
    if cleanup_button(ui, lang, !cleaning) {
        state.images.cleanup = Some(CleanupDialog::default());
    }
    tag_form(ui, state, bridge, lang, profile);
    cleanup_dialog(ui, state, bridge, lang, profile, summary.disk.build_cache);
    inspect_panel(ui, state, lang, ResourceKind::Image);
    let list = entries(&summary.images);
    if list.is_empty() {
        ui.label(Msg::NoImages.text(lang));
        return;
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("image-rows")
            .striped(true)
            .num_columns(5)
            .show(ui, |ui| {
                for entry in &list {
                    row_ui(
                        ui,
                        state,
                        (bridge, lang, profile),
                        entry,
                        &summary.containers,
                    );
                    ui.end_row();
                }
            });
    });
}

fn row_ui(
    ui: &mut egui::Ui,
    state: &mut GuiState,
    (bridge, lang, profile): (&Bridge, Lang, &str),
    entry: &Entry,
    containers: &[ContainerRow],
) {
    let busy = state.busy_ops.contains(&image_key(&entry.target));
    ui.horizontal(|ui| {
        if trash_button(ui, lang, &entry.name, busy) {
            state.confirm = Some(Confirm {
                message: Msg::ConfirmDeleteImage
                    .text(lang)
                    .replace("{name}", &entry.name),
                action: PendingAction::Image {
                    profile: profile.to_string(),
                    id: entry.target.clone(),
                    force: false,
                    in_use: in_use(entry, containers),
                    size: entry.image.size_bytes,
                },
            });
        }
        let tag_label = format!("{} {}", Msg::RowTag.text(lang), entry.name);
        let tag = ui.button("🏷").on_hover_text(&tag_label);
        super::name_widget(ui, &tag, &tag_label);
        if tag.clicked() {
            state.images.tag = Some(TagForm::new(entry.target.clone(), entry.name.clone()));
        }
        let target = (
            ResourceKind::Image,
            entry.target.as_str(),
            entry.name.as_str(),
        );
        inspect_button(ui, state, bridge, lang, profile, target);
    });
    ui.horizontal(|ui| {
        ui.strong(&entry.name);
        if busy {
            ui.spinner();
        }
    });
    ui.weak(short_id(&entry.image.id));
    ui.label(format_bytes(entry.image.size_bytes));
    let created = entry.image.created;
    ui.weak(date_text(created, local_offset(created), lang));
}

fn tag_form(ui: &mut egui::Ui, state: &mut GuiState, bridge: &Bridge, lang: Lang, profile: &str) {
    let key = tag_key(profile);
    let busy = state.busy_ops.contains(&key);
    let Some(form) = state.images.tag.as_mut() else {
        return;
    };
    let check = form.check();
    let (mut apply, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        ui.strong(format!("{} {}", Msg::RowTag.text(lang), form.name));
        for (msg, field) in [
            (Msg::FieldRepo, &mut form.repo),
            (Msg::FieldTagName, &mut form.tag),
        ] {
            let caption = ui.label(msg.text(lang));
            ui.add(egui::TextEdit::singleline(field).desired_width(160.0))
                .labelled_by(caption.id);
        }
        apply = ui
            .add_enabled(
                !busy && check.is_ok(),
                egui::Button::new(Msg::ApplyTag.text(lang)),
            )
            .clicked();
        cancel = ui.button(Msg::Cancel.text(lang)).clicked();
    });
    if let Err(e) = &check
        && !form.repo.trim().is_empty()
    {
        ui.colored_label(crate::gui::WARN, e.message(lang));
    }
    if apply {
        state.busy_ops.insert(key);
        bridge.send(Request::TagImage {
            profile: profile.to_string(),
            id: form.source.clone(),
            repo: form.repo.trim().to_string(),
            tag: form.tag.trim().to_string(),
        });
    }
    if apply || cancel {
        state.images.tag = None;
    }
}

fn pull_form(ui: &mut egui::Ui, state: &mut GuiState, bridge: &Bridge, lang: Lang, profile: &str) {
    let key = pull_key(profile);
    let busy = state.busy_ops.contains(&key);
    if !busy {
        state.images.pull = None;
    }
    let reference = state.images.reference.trim().to_string();
    let check = check_image_ref(&reference);
    ui.horizontal(|ui| {
        let caption = ui.label(Msg::PullTitle.text(lang));
        ui.add_enabled(
            !busy,
            egui::TextEdit::singleline(&mut state.images.reference).desired_width(260.0),
        )
        .labelled_by(caption.id);
        let pull = ui.add_enabled(
            !busy && check.is_ok(),
            egui::Button::new(Msg::Pull.text(lang)),
        );
        if pull.clicked() {
            state.busy_ops.insert(key);
            state.images.pull = Some(PullState::default());
            bridge.send(Request::Pull {
                profile: profile.to_string(),
                reference: reference.clone(),
            });
        }
    });
    if let Err(e) = &check
        && !reference.is_empty()
    {
        ui.colored_label(crate::gui::WARN, e.message(lang));
    }
    if let Some(pull) = &state.images.pull {
        match pull.progress() {
            Some(p) => ui.add(egui::ProgressBar::new(p).show_percentage()),
            None => ui.add(egui::Label::new(&pull.status)),
        };
    }
}

fn cleanup_dialog(
    ui: &mut egui::Ui,
    state: &mut GuiState,
    bridge: &Bridge,
    lang: Lang,
    profile: &str,
    build_cache: Option<u64>,
) {
    let Some(dialog) = state.images.cleanup.as_mut() else {
        return;
    };
    let (mut cancel, mut accept) = (false, false);
    let response = egui::Modal::new(egui::Id::unique("wharfkeep-cleanup")).show(ui.ctx(), |ui| {
        ui.strong(Msg::CleanupImagesTitle.text(lang));
        for (value, msg) in [
            (ImageCleanup::Keep, Msg::CleanupImagesNone),
            (ImageCleanup::Dangling, Msg::CleanupImagesDangling),
            (ImageCleanup::Unused, Msg::CleanupImagesUnused),
        ] {
            ui.radio_value(&mut dialog.images, value, msg.text(lang));
        }
        let size = super::size_text(build_cache, lang);
        ui.checkbox(
            &mut dialog.build_cache,
            Msg::CleanupBuildCache.text(lang).replace("{size}", &size),
        );
        ui.horizontal(|ui| {
            cancel = ui.button(Msg::Cancel.text(lang)).clicked();
            accept = ui
                .add_enabled(
                    !dialog.is_empty(),
                    egui::Button::new(Msg::Delete.text(lang)),
                )
                .clicked();
        });
    });
    if accept {
        for request in dialog.requests(profile) {
            let kind = match request {
                Request::PruneImages { .. } => Cleanup::Images,
                _ => Cleanup::BuildCache,
            };
            state.busy_ops.insert(kind.key(profile));
            bridge.send(request);
        }
    }
    if accept || cancel || response.should_close() {
        state.images.cleanup = None;
    }
}

#[cfg(test)]
mod tests;
