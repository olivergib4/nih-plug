use nih_plug::prelude::Editor;
use nih_plug::prelude::{ParamPtr, Params};
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::widgets::*;
use nih_plug_vizia::{assets, create_vizia_editor, ViziaState, ViziaTheming};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::analyzer::{Analyzer, AnalyzerData, SoneBars};
use crate::help;
use crate::presets::PRESETS;
use crate::{Meters, OnsetGuardParams};

const COLUMN_WIDTH: Units = Pixels(336.0);
const CONTENT_WIDTH: f32 = 1008.0;
/// Fits a 13" MacBook (900 px tall minus menu bar and the host's window chrome).
const GUI_WIDTH: u32 = 1040;
const GUI_HEIGHT: u32 = 790;

#[derive(Clone, Lens)]
pub struct Data {
    pub params: Arc<OnsetGuardParams>,
    pub meters: Arc<Meters>,
    pub analyzer: Arc<Mutex<triple_buffer::Output<AnalyzerData>>>,
}

impl Model for Data {}

/// The help panel's text, updated by hovering any control.
#[derive(Lens)]
pub struct HelpModel {
    pub text: String,
}

pub enum HelpEvent {
    Show(&'static str),
}

impl Model for HelpModel {
    fn event(&mut self, _cx: &mut EventContext, event: &mut Event) {
        event.map(|e, _| match e {
            HelpEvent::Show(t) => self.text = (*t).to_string(),
        });
    }
}

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
        HelpModel {
            text: String::from(
                "Hover any control, preset or meter and its explanation appears here. \
                 Threshold, Downward Ratio, Clip Drive and Sub Split do most of the work.",
            ),
        }
        .build(cx);

        VStack::new(cx, |cx| {
            // --- title + presets on one line ---
            HStack::new(cx, |cx| {
                Label::new(cx, "OnsetGuard")
                    .font_family(vec![FamilyOwned::Name(String::from(assets::NOTO_SANS))])
                    .font_weight(FontWeightKeyword::Thin)
                    .font_size(26.0)
                    .right(Pixels(18.0));
                Label::new(cx, "presets").font_size(12.0).top(Pixels(8.0)).right(Pixels(6.0));
                for preset in PRESETS {
                    let params = params.clone();
                    Button::new(
                        cx,
                        move |cx| apply_preset(cx, params.as_ref(), preset.values),
                        |cx| Label::new(cx, preset.name),
                    )
                    .on_hover(|cx| cx.emit(HelpEvent::Show(help::PRESET_HELP)))
                    .top(Pixels(4.0))
                    .right(Pixels(6.0));
                }
            })
            .height(Pixels(34.0))
            .width(Pixels(CONTENT_WIDTH))
            .child_left(Pixels(0.0));

            // --- analyzer ---
            Analyzer::new(cx, Data::analyzer)
                .width(Pixels(CONTENT_WIDTH))
                .height(Pixels(140.0))
                .on_hover(|cx| cx.emit(HelpEvent::Show(help::ANALYZER_HELP)));

            // --- loudness + help panel side by side ---
            HStack::new(cx, |cx| {
                SoneBars::new(cx, Data::meters)
                    .width(Pixels(500.0))
                    .height(Pixels(60.0))
                    .on_hover(|cx| cx.emit(HelpEvent::Show(help::SONE_HELP)));
                Label::new(cx, HelpModel::text)
                    .text_wrap(true)
                    .font_size(12.0)
                    .width(Pixels(CONTENT_WIDTH - 512.0))
                    .height(Pixels(60.0))
                    .left(Pixels(12.0))
                    .class("helppanel");
            })
            .height(Pixels(60.0))
            .width(Pixels(CONTENT_WIDTH));

            // --- controls, two rows of three columns ---
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
                    HStack::new(cx, |cx| {
                        meter(cx, "Spectral clip GR", Data::meters.map(|m| m.gr_spec_db.load(Ordering::Relaxed)), help::GR_SPEC_HELP);
                    })
                    .size(Auto)
                    .top(Pixels(6.0));
                });
                make_column(cx, "Output", |cx| {
                    params_with_help(cx, Data::params.map(|p| p.global.clone()));
                    HStack::new(cx, |cx| {
                        meter(cx, "Post clip GR", Data::meters.map(|m| m.gr_post_db.load(Ordering::Relaxed)), help::GR_POST_HELP);
                    })
                    .size(Auto)
                    .top(Pixels(6.0));
                });
            })
            .size(Auto);
        })
        .row_between(Pixels(6.0))
        .child_top(Pixels(6.0))
        .child_left(Stretch(1.0))
        .child_right(Stretch(1.0));

        ResizeHandle::new(cx);
    })
}

/// Set every parameter: listed ones to the preset value, the rest to their defaults.
fn apply_preset(cx: &mut EventContext, params: &OnsetGuardParams, values: &[(&str, f32)]) {
    for (id, ptr, _) in params.param_map() {
        // never let a preset change level, bypass or the meter calibration
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
        Label::new(cx, title).font_size(11.0);
        PeakMeter::new(cx, value_db, Some(Duration::from_millis(600))).top(Pixels(2.0));
    })
    .width(Pixels(300.0))
    .height(Auto)
    .left(Pixels(15.0))
    .on_hover(move |cx| cx.emit(HelpEvent::Show(help_text)));
}

/// A generic parameter list where hovering a row shows its explanation in the help panel.
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
            row.on_hover(move |cx| cx.emit(HelpEvent::Show(text)));
        }
    });
}

fn make_column(cx: &mut Context, title: &str, contents: impl FnOnce(&mut Context)) {
    VStack::new(cx, |cx| {
        Label::new(cx, title)
            .font_family(vec![FamilyOwned::Name(String::from(assets::NOTO_SANS))])
            .font_weight(FontWeightKeyword::Thin)
            .font_size(18.0)
            .left(Stretch(1.0))
            .right(Pixels(7.0))
            .bottom(Pixels(-8.0));

        contents(cx);
    })
    .width(COLUMN_WIDTH)
    .height(Auto);
}
