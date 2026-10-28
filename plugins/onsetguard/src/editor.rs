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

const COLUMN_WIDTH: Units = Pixels(340.0);
const CONTENT_WIDTH: f32 = 1020.0;
/// Fits a 13" MacBook (900 px tall minus the menu bar and the host's window chrome).
const GUI_WIDTH: u32 = 1040;
const GUI_HEIGHT: u32 = 780;
/// Measured: one generic-UI row is 26 px tall at our stylesheet.
const ROW_PX: f32 = 26.0;

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
            HelpEvent::Show(t) => {
                if self.text != *t {
                    self.text = (*t).to_string();
                }
            }
        });
    }
}

/// Text readouts for the meters. Updated by a background ticker (Vizia's baseview backend has
/// no timers, and canvas text has no font), ten times a second while the editor is open.
#[derive(Lens)]
pub struct MeterModel {
    pub dry: String,
    pub wet: String,
    pub matched: String,
    meters: Arc<Meters>,
}

pub enum MeterEvent {
    Tick,
}

fn phon(s: f32) -> f32 {
    if s <= 1e-3 {
        0.0
    } else {
        40.0 + 10.0 * s.log2()
    }
}

impl Model for MeterModel {
    fn event(&mut self, _cx: &mut EventContext, event: &mut Event) {
        event.map(|e, _| match e {
            MeterEvent::Tick => {
                let d = self.meters.dry_sone.load(Ordering::Relaxed);
                let w = self.meters.wet_sone.load(Ordering::Relaxed);
                let m = self.meters.match_db.load(Ordering::Relaxed);
                self.dry = format!("dry  {d:.1} sone · {:.0} phon", phon(d));
                self.wet = format!("processed  {w:.1} sone · {:.0} phon", phon(w));
                self.matched = format!("match {m:+.1} dB");
            }
        });
    }
}

/// A container that shows `text` in the help panel whenever the mouse moves anywhere inside it.
pub struct HelpArea {
    text: &'static str,
}

impl HelpArea {
    pub fn new<'a>(cx: &'a mut Context, text: &'static str, content: impl FnOnce(&mut Context)) -> Handle<'a, Self> {
        Self { text }.build(cx, content)
    }
}

impl View for HelpArea {
    fn element(&self) -> Option<&'static str> {
        Some("helparea")
    }

    fn event(&mut self, cx: &mut EventContext, event: &mut Event) {
        event.map(|window_event, _| {
            if let WindowEvent::MouseMove(_, _) = window_event {
                cx.emit(HelpEvent::Show(self.text));
            }
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
    let state_for_ticker = editor_state.clone();
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
        MeterModel {
            dry: String::from("dry"),
            wet: String::from("processed"),
            matched: String::new(),
            meters: meters.clone(),
        }
        .build(cx);

        // Ticker: emits MeterEvent::Tick into the window's event queue while the editor is open.
        let state = state_for_ticker.clone();
        cx.spawn(move |proxy| {
            // give the window a moment to open before the first tick
            std::thread::sleep(Duration::from_millis(200));
            while state.is_open() {
                if proxy.emit(MeterEvent::Tick).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });

        VStack::new(cx, |cx| {
            // --- title + presets on one line ---
            HStack::new(cx, |cx| {
                Label::new(cx, "OnsetGuard")
                    .font_family(vec![FamilyOwned::Name(String::from(assets::NOTO_SANS))])
                    .font_weight(FontWeightKeyword::Thin)
                    .font_size(24.0)
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
                    .on_press_down(|cx| cx.emit(HelpEvent::Show(help::PRESET_HELP)))
                    .top(Pixels(3.0))
                    .right(Pixels(6.0));
                }
            })
            .height(Pixels(32.0))
            .width(Pixels(CONTENT_WIDTH));

            // --- analyzer ---
            HelpArea::new(cx, help::ANALYZER_HELP, |cx| {
                Analyzer::new(cx, Data::analyzer).width(Pixels(CONTENT_WIDTH)).height(Pixels(140.0));
            })
            .width(Pixels(CONTENT_WIDTH))
            .height(Pixels(140.0));

            // --- loudness readout + help panel ---
            HStack::new(cx, |cx| {
                HelpArea::new(cx, help::SONE_HELP, |cx| {
                    HStack::new(cx, |cx| {
                        Label::new(cx, MeterModel::dry).font_size(11.0).width(Pixels(230.0));
                        Label::new(cx, MeterModel::wet).font_size(11.0).width(Pixels(230.0));
                    })
                    .height(Pixels(16.0));
                    SoneBars::new(cx, Data::meters).width(Pixels(460.0)).height(Pixels(40.0));
                    Label::new(cx, MeterModel::matched).font_size(11.0).height(Pixels(16.0));
                })
                .layout_type(LayoutType::Column)
                .width(Pixels(460.0))
                .height(Pixels(74.0));
                Label::new(cx, HelpModel::text)
                    .text_wrap(true)
                    .font_size(11.0)
                    .width(Pixels(CONTENT_WIDTH - 472.0))
                    .height(Pixels(74.0))
                    .left(Pixels(12.0))
                    .class("helppanel");
            })
            .height(Pixels(74.0))
            .width(Pixels(CONTENT_WIDTH));

            // --- controls: three columns, groups stacked to balance height ---
            HStack::new(cx, |cx| {
                VStack::new(cx, |cx| {
                    group(cx, "Clip", 13, |cx| params_with_help(cx, Data::params.map(|p| p.clip.clone())));
                    HStack::new(cx, |cx| {
                        meter(cx, "Spectral clip GR", Data::meters.map(|m| m.gr_spec_db.load(Ordering::Relaxed)), help::GR_SPEC_HELP);
                    })
                    .size(Auto)
                    .top(Pixels(4.0));
                })
                .width(COLUMN_WIDTH)
                .height(Auto);

                VStack::new(cx, |cx| {
                    group(cx, "Threshold", 5, |cx| params_with_help(cx, Data::params.map(|p| p.threshold.clone())));
                    group(cx, "Dynamics", 7, |cx| params_with_help(cx, Data::params.map(|p| p.dynamics.clone())));
                    HStack::new(cx, |cx| {
                        meter(cx, "Post clip GR", Data::meters.map(|m| m.gr_post_db.load(Ordering::Relaxed)), help::GR_POST_HELP);
                    })
                    .size(Auto)
                    .top(Pixels(4.0));
                })
                .width(COLUMN_WIDTH)
                .height(Auto);

                VStack::new(cx, |cx| {
                    group(cx, "Onset Protection", 7, |cx| params_with_help(cx, Data::params.map(|p| p.protect.clone())));
                    group(cx, "Resolution", 4, |cx| params_with_help(cx, Data::params.map(|p| p.resolution.clone())));
                    group(cx, "Output", 4, |cx| params_with_help(cx, Data::params.map(|p| p.global.clone())));
                })
                .width(COLUMN_WIDTH)
                .height(Auto);
            })
            .size(Auto);
        })
        .row_between(Pixels(6.0))
        .child_top(Pixels(4.0))
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
    HelpArea::new(cx, help_text, |cx| {
        Label::new(cx, title).font_size(11.0);
        PeakMeter::new(cx, value_db, Some(Duration::from_millis(600))).top(Pixels(2.0));
    })
    .layout_type(LayoutType::Column)
    .width(Pixels(300.0))
    .height(Auto)
    .left(Pixels(15.0));
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
        let help_text = ids.get(&param_ptr).and_then(|id| help::for_param(id)).unwrap_or("");
        HelpArea::new(cx, help_text, |cx| {
            Label::new(cx, unsafe { param_ptr.name() }).class("label");
            GenericUi::draw_widget(cx, params, param_ptr);
        })
        .class("row")
        .layout_type(LayoutType::Row)
        .col_between(Pixels(6.0))
        .height(Auto);
    });
}

/// A titled parameter group with a fixed height (title + `rows` rows) so columns line up.
fn group(cx: &mut Context, title: &str, rows: usize, contents: impl FnOnce(&mut Context)) {
    VStack::new(cx, |cx| {
        Label::new(cx, title)
            .font_family(vec![FamilyOwned::Name(String::from(assets::NOTO_SANS))])
            .font_weight(FontWeightKeyword::Thin)
            .font_size(16.0)
            .left(Stretch(1.0))
            .right(Pixels(7.0))
            .bottom(Pixels(-6.0));

        contents(cx);
    })
    .width(COLUMN_WIDTH)
    .height(Pixels(22.0 + ROW_PX * rows as f32 + 6.0));
}
