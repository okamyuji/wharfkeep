use crate::i18n::Lang;
use eframe::egui;
use egui::epaint::text::{FallbackRequest, FontData, FontFamily, FontInsert, FontProvider};
use egui_system_font_provider::SystemFontProvider;
use std::sync::Arc;

/// 描けない文字（日本語など）をOSのフォントで補う。言語は`WHARFKEEP_LANG`に従い、OSの言語には従わない。
pub(crate) fn install_fonts(ctx: &egui::Context, lang: Lang) {
    let system = SystemFontProvider::new().with_locale(Some(lang.tag()));
    ctx.set_font_providers(vec![Arc::new(AlignedFonts { system })]);
}

/// OSのフォントを、ラテン文字のフォントと基準線がそろうように補正して渡す。
///
/// epaintは、字族の違うフォントを混ぜた行で基準線をそろえず、各フォントの行箱（lineGapを含む）の
/// 中心をそろえる。ヒラギノはlineGapが0.5emあるので、そのままでは数字より上に浮く。
/// また主フォントを決めずにおくと、最初に描いた文字のフォントが主フォントになり、行の高さが変わる。
struct AlignedFonts {
    system: SystemFontProvider,
}

impl AlignedFonts {
    /// 字族の主フォント。ラテン文字を描くOSのフォント（macOSではHelveticaとCourier）にする。
    fn primary(&self, family: &FontFamily) -> Option<FontInsert> {
        self.system.font_for(&FallbackRequest {
            cluster: "A",
            family,
        })
    }
}

impl FontProvider for AlignedFonts {
    fn fonts_for_family(&self, family: &FontFamily) -> Vec<FontInsert> {
        self.primary(family).into_iter().collect()
    }

    fn font_for(&self, request: &FallbackRequest<'_>) -> Option<FontInsert> {
        let mut insert = self.system.font_for(request)?;
        let shift = self.primary(request.family).and_then(|primary| {
            Some(baseline_shift(
                v_metrics(&primary.data)?,
                v_metrics(&insert.data)?,
            ))
        });
        if let Some(shift) = shift {
            insert.data.tweak.y_offset_factor = shift;
            // 同じフォントでも字族ごとに主フォントが違い、補正量も違う。epaintは名前で使い回すので名前を分ける。
            insert.name = format!("{}:{:?}", insert.name, request.family);
        }
        Some(insert)
    }
}

/// フォントの縦の計量値。単位はem。
#[derive(Clone, Copy, Debug, PartialEq)]
struct VMetrics {
    ascent: f32,
    /// 基準線より下を負で表す。
    descent: f32,
    line_gap: f32,
}

fn v_metrics(data: &FontData) -> Option<VMetrics> {
    use skrifa::MetadataProvider as _;
    let font = skrifa::FontRef::from_index(data.bytes(), data.index).ok()?;
    // epaintも既定の位置の計量値で行箱を決める。
    let m = font.metrics(
        skrifa::instance::Size::unscaled(),
        skrifa::instance::LocationRef::default(),
    );
    in_em(m.ascent, m.descent, m.leading, m.units_per_em)
}

/// 壊れたフォントのunits_per_em = 0で割ると、補正量がNaNになり字が消える。
fn in_em(ascent: f32, descent: f32, line_gap: f32, units_per_em: u16) -> Option<VMetrics> {
    let em = f32::from(units_per_em);
    (em > 0.0).then(|| VMetrics {
        ascent: ascent / em,
        descent: descent / em,
        line_gap: line_gap / em,
    })
}

/// `font`の字面を下げる量（em）。基準のフォントと基準線がそろう。
///
/// epaintは字面の基準線を「ascent + (主フォントの行高 − 行高) / 2」に置く。主フォントの項は
/// どのフォントにも共通なので、「ascent − 行高 / 2」の差だけ下げれば基準線がそろう。
fn baseline_shift(reference: VMetrics, font: VMetrics) -> f32 {
    let anchor = |m: VMetrics| m.ascent - (m.ascent - m.descent + m.line_gap) / 2.0;
    anchor(reference) - anchor(font)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELVETICA: VMetrics = VMetrics {
        ascent: 0.77,
        descent: -0.23,
        line_gap: 0.0,
    };
    const HIRAGINO: VMetrics = VMetrics {
        ascent: 0.88,
        descent: -0.12,
        line_gap: 0.5,
    };

    #[test]
    fn baseline_shift_lowers_a_font_whose_line_gap_lifts_it() {
        // epaintは行箱の中心をそろえる。lineGapの半分だけ基準線が上がるぶんを下げて戻す。
        let got = baseline_shift(HELVETICA, HIRAGINO);
        assert!((got - 0.14).abs() < 1e-6, "{got}");
    }

    #[test]
    fn in_em_divides_by_units_per_em_and_refuses_zero() {
        assert_eq!(
            in_em(880.0, -120.0, 500.0, 1000),
            Some(VMetrics {
                ascent: 0.88,
                descent: -0.12,
                line_gap: 0.5
            })
        );
        assert_eq!(in_em(1.0, 0.0, 0.0, 1).map(|m| m.ascent), Some(1.0));
        assert_eq!(in_em(880.0, -120.0, 500.0, 0), None);
    }

    #[test]
    fn v_metrics_of_bytes_that_are_not_a_font_is_none() {
        assert_eq!(v_metrics(&FontData::from_static(&[0u8; 4])), None);
    }

    #[test]
    fn fallback_font_is_named_and_shifted_per_family() {
        let fonts = AlignedFonts {
            system: SystemFontProvider::new().with_locale(Some("ja")),
        };
        let get = |family: FontFamily| {
            fonts
                .font_for(&FallbackRequest {
                    cluster: "ロ",
                    family: &family,
                })
                .unwrap()
        };
        let prop = get(FontFamily::Proportional);
        let mono = get(FontFamily::Monospace);
        assert_ne!(prop.name, mono.name);
        assert!(prop.data.tweak.y_offset_factor > 0.0);
        assert_ne!(
            prop.data.tweak.y_offset_factor,
            mono.data.tweak.y_offset_factor
        );
    }

    #[test]
    fn baseline_shift_is_zero_for_the_reference_itself() {
        assert_eq!(baseline_shift(HIRAGINO, HIRAGINO), 0.0);
    }

    #[test]
    fn baseline_shift_is_antisymmetric() {
        let up = baseline_shift(HIRAGINO, HELVETICA);
        assert!((up + 0.14).abs() < 1e-6, "{up}");
    }

    struct Ink {
        height: f32,
        mids: Vec<(char, f32)>,
    }

    /// 各文字の字面の縦の中心（点）と行の高さを測る。
    fn ink(texts: &'static [&'static str]) -> Vec<Ink> {
        let got = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let out = got.clone();
        let mut h = egui_kittest::Harness::builder()
            .with_context_setup(|ctx| install_fonts(ctx, Lang::Ja))
            .build_ui(move |ui| {
                let font = egui::TextStyle::Body.resolve(ui.style());
                let mut rows = Vec::new();
                for t in texts {
                    let g = ui.painter().layout_no_wrap(
                        t.to_string(),
                        font.clone(),
                        egui::Color32::WHITE,
                    );
                    let mids = g.rows[0]
                        .glyphs
                        .iter()
                        .map(|g| (g.chr, g.pos.y + g.uv_rect.offset.y + g.uv_rect.size.y / 2.0))
                        .collect();
                    rows.push(Ink {
                        height: g.size().y,
                        mids,
                    });
                }
                *out.borrow_mut() = rows;
            });
        h.run();
        got.take()
    }

    fn mid(row: &Ink, c: char) -> f32 {
        row.mids.iter().find(|(g, _)| *g == c).unwrap().1
    }

    #[test]
    fn katakana_and_digits_share_a_centre_line() {
        // 先に描く文字で主フォントが変わらないことは別のテストで見る。ここではアプリと同じくラテン文字を先に描く。
        let rows = ink(&["H0", "ロ0", "インスタンス: 1"]);
        assert!((mid(&rows[1], 'ロ') - mid(&rows[1], '0')).abs() <= 1.0);
        assert!((mid(&rows[2], 'ス') - mid(&rows[2], '1')).abs() <= 1.0);
    }

    #[test]
    fn row_height_does_not_depend_on_which_script_is_drawn_first() {
        let latin_first = ink(&["H0", "ロ0"]);
        let japanese_first = ink(&["ロ0", "H0"]);
        let heights = [
            latin_first[0].height,
            latin_first[1].height,
            japanese_first[0].height,
            japanese_first[1].height,
        ];
        assert!(heights.iter().all(|h| *h == heights[0]), "{heights:?}");
    }
}
