use nih_plug::prelude::Editor;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::widgets::*;
use nih_plug_vizia::{assets, create_vizia_editor, ViziaState, ViziaTheming};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::analyzer::{Analyzer, AnalyzerData, SoneBars};
use crate::help;
use crate::presets::PRESETS;
use nih_plug::prelude::{ParamPtr, Params};
use crate::{Meters, OnsetGuardParams};

const COLUMN_WIDTH: Units = Pixels(340.0);
const GUI_WIDTH: u32 = 1040;
const GUI_HEIGHT: u32 = 960;

#[derive(Clone, Lens)]
pub struct Data {
    pub params: Arc<OnsetGuardParams>,
    pub meters: Arc<Meters>,
    pub analyzer: Arc<Mutex<triple_buffer::Output<AnalyzerData>>>,
}

impl Model for Data {}

pub(crate) fn default_state() -> Arc<ViziaState> {
    ViziaState::new(|| (GUI_WIDTH, GUI_HEIGHT))
}

pub(crate) fn create(
    params: Arc<OnsetGuardParams>,
    meters: Arc<Meters>,
    analyzer: Arc<Mutex<triple_buffer::Output<AnalyzerData>>>,
    editor_state: Arc<ViziaState>,
) -> Option<Box<dyn Editor>> {
    create_vizia_editor(editor_state, ViziaTheming::Custom, move |cx, _| {
        assets::register_noto_sans_light(cx);
        assets::register_noto_sans_thin(cx);
        if let Err(err) = cx.add_stylesheet(include_style!("src/theme.css")) {
            nih_plug::nih_error!("Failed to load stylesheet: {err:?}")
        }

        Data {
            params: params.clone(),
            meters: meters.clone(),
            analyzer: analyzer.clone(),
        }
        .build(cx);

        VStack::new(cx, |cx| {
            HStack::new(cx, |cx| {
                Label::new(cx, "OnsetGuard")
                    .font_family(vec![FamilyOwned::Name(String::from(assets::NOTO_SANS))])
                    .font_weight(FontWeightKeyword::Thin)
                    .font_size(30.0)
                    .left(Pixels(10.0));
                Label::new(cx, "zero-phase spectral dynamics · onset protection · spectral clip")
                    .font_size(12.0)
                    .left(Pixels(14.0))
                    .top(Pixels(14.0));
            })
            .height(Pixels(40.0))
            .top(Pixels(6.0));

            // --- presets ---
            HStack::new(cx, |cx| {
                Label::new(cx, "Presets").font_size(12.0).top(Pixels(6.0)).right(Pixels(8.0));
                for preset in PRESETS {
                    let params = params.clone();
                    Button::new(
                        cx,
                        move |cx| apply_preset(cx, params.as_ref(), preset.values),
                        |cx| Label::new(cx, preset.name),
                    )
                    .tooltip(|cx| {
                        Label::new(cx, help::PRESET_HELP).text_wrap(true).class("help");
                    })
                    .right(Pixels(6.0));
                }
                Label::new(cx, "(sets every control; the host sees it as parameter changes)")
                    .font_size(11.0)
                    .top(Pixels(6.0))
                    .left(Pixels(8.0));
            })
            .height(Pixels(30.0))
            .child_left(Stretch(1.0))
            .child_right(Stretch(1.0));

            // --- analyzer: detector spectrum, threshold curve, applied gain ---
            Analyzer::new(cx, Data::analyzer)
                .width(Pixels(1010.0))
                .height(Pixels(190.0));
            Label::new(
                cx,
                "grey: what the detector hears · orange: threshold curve · blue/green bars: \
                 cut/boost per frequency · yellow strip: a hit is being protected",
            )
            .font_size(11.0)
            .left(Stretch(1.0))
            .right(Stretch(1.0));

            HStack::new(cx, |cx| {
                make_column(cx, "Threshold", |cx| {
                    params_with_help(cx, Data::params.map(|p| p.threshold.clone()));
                });
                make_column(cx, "Dynamics", |cx| {
                    params_with_help(cx, Data::params.map(|p| p.dynamics.clone()));
                });
                make_column(cx, "Onset Protection", |cx| {
                    params_with_help(cx, Data::params.map(|p| p.protect.clone()));
                });
            })
            .size(Auto);

            HStack::new(cx, |cx| {
                make_column(cx, "Clip", |cx| {
                    params_with_help(cx, Data::params.map(|p| p.clip.clone()));
                });
                make_column(cx, "Resolution", |cx| {
                    params_with_help(cx, Data::params.map(|p| p.resolution.clone()));
                    Label::new(
                        cx,
                        "Latency = long + 2×short windows + lookahead. Ableton compensates; \
                         bounce for live use.",
                    )
                    .text_wrap(true)
                    .font_size(11.0)
                    .left(Pixels(15.0))
                    .right(Pixels(8.0))
                    .width(Stretch(1.0));
                });
                make_column(cx, "Output", |cx| {
                    params_with_help(cx, Data::params.map(|p| p.global.clone()));
                    Label::new(
                        cx,
                        "Matched Bypass plays the dry signal at the processed signal's \
                         perceived loudness (sones), so you compare shape, not volume.",
                    )
                    .text_wrap(true)
                    .font_size(11.0)
                    .left(Pixels(15.0))
                    .right(Pixels(8.0))
                    .width(Stretch(1.0));
                });
            })
            .size(Auto);

            // --- perceived loudness ---
            SoneBars::new(cx, Data::meters)
                .width(Pixels(1010.0))
                .height(Pixels(56.0))
                .tooltip(|cx| {
                    Label::new(cx, help::SONE_HELP).text_wrap(true).class("help");
                });
            Label::new(
                cx,
                "Zwicker-style loudness in sones (needs Reference Level: what 0 dBFS plays at on \
                 your system). Matched Bypass now matches in sones.",
            )
            .font_size(11.0)
            .left(Stretch(1.0))
            .right(Stretch(1.0));

            // --- meters: how much each clip stage is doing ---
            HStack::new(cx, |cx| {
                meter(cx, "Spectral clip GR", Data::meters.map(|m| m.gr_spec_db.load(Ordering::Relaxed)), help::GR_SPEC_HELP);
                meter(cx, "Post clip GR", Data::meters.map(|m| m.gr_post_db.load(Ordering::Relaxed)), help::GR_POST_HELP);
                meter(cx, "Dry K-level (LUFS-ish)", Data::meters.map(|m| m.dry_lufs.load(Ordering::Relaxed)), help::K_HELP);
                meter(cx, "Processed K-level (LUFS-ish)", Data::meters.map(|m| m.wet_lufs.load(Ordering::Relaxed)), help::K_HELP);
            })
            .col_between(Pixels(16.0))
            .child_left(Stretch(1.0))
            .child_right(Stretch(1.0))
            .size(Auto);

            Label::new(
                cx,
                "If Post clip GR is doing most of the work, lower Spectral Ceiling or Drive: \
                 the spectral stage should take the bulk of the reduction.",
            )
            .font_size(11.0)
            .left(Stretch(1.0))
            .right(Stretch(1.0));
        })
        .row_between(Pixels(10.0))
        .child_left(Stretch(1.0))
        .child_right(Stretch(1.0));

        ResizeHandle::new(cx);
    })
}

/// Set every parameter: listed ones to the preset value, the rest to their defaults.
fn apply_preset(cx: &mut EventContext, params: &OnsetGuardParams, values: &[(&str, f32)]) {
    for (id, ptr, _) in params.param_map() {
        // skip the editor-only / global output controls so a preset never changes level or bypass
        if matches!(id.as_str(), "output" | "mix" | "matched_bypass" | "ref_level") {
            continue;
        }
        let normalized = match values.iter().find(|(pid, _)| *pid == id.as_str()) {
            Some((_, plain)) => unsafe { ptr.preview_normalized(*plain) },
            None => unsafe { ptr.default_normalized_value() },
        };
        set_param(cx, ptr, normalized);
    }
}

fn set_param(cx: &mut EventContext, ptr: ParamPtr, normalized: f32) {
    cx.emit(RawParamEvent::BeginSetParameter(ptr));
    cx.emit(RawParamEvent::SetParameterNormalized(ptr, normalized));
    cx.emit(RawParamEvent::EndSetParameter(ptr));
}

fn meter(cx: &mut Context, title: &str, value_db: impl Lens<Target = f32>, help_text: &'static str) {
    VStack::new(cx, |cx| {
        Label::new(cx, title).font_size(12.0);
        PeakMeter::new(cx, value_db, Some(Duration::from_millis(600))).top(Pixels(4.0));
    })
    .width(Pixels(230.0))
    .height(Auto)
    .tooltip(move |cx| {
        Label::new(cx, help_text).text_wrap(true).class("help");
    });
}

/// A generic parameter list where every row carries a hover tooltip explaining the control.
fn params_with_help<L, PsRef, Ps>(cx: &mut Context, params: L)
where
    L: Lens<Target = PsRef> + Copy,
    PsRef: AsRef<Ps> + 'static,
    Ps: Params + 'static,
{
    let ids: std::collections::HashMap<ParamPtr, String> = params
        .map(|p| p.as_ref().param_map())
        .get(cx)
        .into_iter()
        .map(|(id, ptr, _)| (ptr, id))
        .collect();
    GenericUi::new_custom(cx, params, move |cx, param_ptr| {
        let help_text = ids.get(&param_ptr).and_then(|id| help::for_param(id));
        let row = HStack::new(cx, |cx| {
            Label::new(cx, unsafe { param_ptr.name() }).class("label");
            GenericUi::draw_widget(cx, params, param_ptr);
        })
        .class("row");
        if let Some(text) = help_text {
            row.tooltip(move |cx| {
                Label::new(cx, text).text_wrap(true).class("help");
            });
        }
    });
}

fn make_column(cx: &mut Context, title: &str, contents: impl FnOnce(&mut Context)) {
    VStack::new(cx, |cx| {
        Label::new(cx, title)
            .font_family(vec![FamilyOwned::Name(String::from(assets::NOTO_SANS))])
            .font_weight(FontWeightKeyword::Thin)
            .font_size(21.0)
            .left(Stretch(1.0))
            .right(Pixels(7.0))
            .bottom(Pixels(-10.0));

        contents(cx);
    })
    .width(COLUMN_WIDTH)
    .height(Auto);
}
