use nih_plug::prelude::Editor;
use nih_plug::prelude::{Param, ParamPtr, Params};
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::widgets::*;
use nih_plug_vizia::{assets, create_vizia_editor, ViziaState, ViziaTheming};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::analyzer::{Analyzer, AnalyzerData, LoudBar, LoudSource};
use crate::help;
use crate::presets::PRESETS;
use crate::{Meters, OnsetGuardParams};

const COLUMN_WIDTH: Units = Pixels(340.0);
const CONTENT_WIDTH: f32 = 1360.0;
/// Fits a 13" MacBook (900 px tall minus the menu bar and the host's window chrome).
const GUI_WIDTH: u32 = 1380;
const GUI_HEIGHT: u32 = 800;
/// Measured: one generic-UI row is 26 px tall at our stylesheet.
const ROW_PX: f32 = 26.0;
/// Height of the three meter panels under the analyzer.
const PANEL_H: f32 = 116.0;

#[derive(Clone, Lens)]
pub struct Data {
    pub params: Arc<OnsetGuardParams>,
    pub meters: Arc<Meters>,
    pub analyzer: Arc<Mutex<triple_buffer::Output<AnalyzerData>>>,
}

impl Model for Data {}

/// Simple / Advanced view. Simple shows the controls you touch per track; Advanced shows all.
#[derive(Lens)]
pub struct ViewModel {
    pub advanced: bool,
    params: Arc<OnsetGuardParams>,
}

pub enum ViewEvent {
    Toggle,
}

impl Model for ViewModel {
    fn event(&mut self, _cx: &mut EventContext, event: &mut Event) {
        event.map(|e, _| match e {
            ViewEvent::Toggle => {
                self.advanced = !self.advanced;
                self.params.advanced.store(self.advanced, Ordering::Relaxed);
            }
        });
    }
}

/// Parameters shown in the Simple view.
const SIMPLE_IDS: &[&str] = &[
    "clip", "drive", "hit_drive", "ceiling", "trans_clip", "hf_preserve", "hf_amount", "hf_hits", "sub_split", "band_protect", "oversample", "rotate", "rotate_hits",
    "autopilot", "target", "crest_floor",
    "weighting", "threshold", "slope",
    "down_ratio", "up_ratio", "up_range", "up_depth", "body", "attack", "release",
    "protect", "sens", "trans_boost",
    "mono_below", "width", "guard", "corr_floor",
    "balance",
    "output", "mix", "matched_bypass", "ref_level", "meter_thru_clip",
];

fn shown(id: &str, advanced: bool) -> bool {
    advanced || SIMPLE_IDS.contains(&id)
}

fn visible_rows<Ps: Params>(ps: &Ps, advanced: bool) -> usize {
    ps.param_map().iter().filter(|(id, _, _)| shown(id, advanced)).count()
}

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
    pub mono: String,
    pub matched: String,
    pub tr_full: String,
    pub tr_laptop: String,
    pub tr_phone: String,
    pub tr_mono: String,
    pub tr_flat: String,
    pub tr_dist: String,
    pub auto_drive: String,
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
                let (md, mw) = (self.meters.mono_dry_sone.load(Ordering::Relaxed), self.meters.mono_wet_sone.load(Ordering::Relaxed));
                self.dry = format!("dry  {d:.1} sone · {:.0} phon", phon(d));
                self.wet = format!("processed  {w:.1} sone · {:.0} phon", phon(w));
                self.mono = format!("mono sum  {mw:.1} sone · {:.0} phon", phon(mw));
                self.matched = format!("match {m:+.1} dB");
                let t: Vec<f32> = self.meters.tr_sone.iter().map(|v| v.load(Ordering::Relaxed)).collect();
                let row = |name: &str, dry: f32, wet: f32| {
                    let delta = phon(wet) - phon(dry);
                    format!("{name}: {dry:.1} → {wet:.1} sone ({delta:+.1} phon)")
                };
                self.tr_full = row("full", d, w);
                self.tr_laptop = row("laptop", t[2], t[3]);
                self.tr_phone = row("phone", t[0], t[1]);
                self.tr_mono = format!("mono: {md:.1} → {mw:.1} sone ({:+.1} phon) · {:+.1} vs stereo", phon(mw) - phon(md), phon(mw) - phon(w));
                let (fd, fw) = (self.meters.flat_dry.load(Ordering::Relaxed), self.meters.flat_wet.load(Ordering::Relaxed));
                self.tr_flat = format!("noise-likeness: {:.0}% → {:.0}%  (0 = tones, 100 = white noise)", fd * 100.0, fw * 100.0);
                let (cd, cw, rp) = (self.meters.crest_dry.load(Ordering::Relaxed), self.meters.crest_wet.load(Ordering::Relaxed), self.meters.rail_pct.load(Ordering::Relaxed));
                self.tr_dist = format!("crest: {cd:.1} → {cw:.1} dB · at ceiling: {rp:.0}% of samples");
                let locked = self.meters.auto_locked.load(Ordering::Relaxed);
                self.auto_drive = format!("auto drive {:+.1} dB{}", self.meters.auto_drive_db.load(Ordering::Relaxed), if locked { "  (settled, holding)" } else { "" });
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
        ViewModel { advanced: params.advanced.load(Ordering::Relaxed), params: params.clone() }.build(cx);
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
            mono: String::from("mono sum"),
            matched: String::new(),
            tr_full: String::new(),
            tr_laptop: String::new(),
            tr_phone: String::new(),
            tr_mono: String::new(),
            tr_flat: String::new(),
            tr_dist: String::new(),
            auto_drive: String::new(),
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
                Label::new(cx, "Bloater")
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
                Button::new(
                    cx,
                    |cx| cx.emit(ViewEvent::Toggle),
                    |cx| Label::new(cx, ViewModel::advanced.map(|a| if *a { "Advanced view" } else { "Simple view" })),
                )
                .on_hover(|cx| cx.emit(HelpEvent::Show(help::VIEW_HELP)))
                .top(Pixels(3.0))
                .left(Stretch(1.0))
                .right(Pixels(2.0));
            })
            .height(Pixels(32.0))
            .width(Pixels(CONTENT_WIDTH));

            // --- analyzer ---
            HelpArea::new(cx, help::ANALYZER_HELP, |cx| {
                Analyzer::new(cx, Data::analyzer, Data::params).width(Pixels(CONTENT_WIDTH)).height(Pixels(124.0));
            })
            .width(Pixels(CONTENT_WIDTH))
            .height(Pixels(124.0));

            // --- meters row: loudness | translation | help, three matching panels ---
            HStack::new(cx, |cx| {
                HelpArea::new(cx, help::SONE_HELP, |cx| {
                    Label::new(cx, "Loudness").class("paneltitle");
                    loud_row(cx, MeterModel::dry, LoudSource::Dry);
                    loud_row(cx, MeterModel::wet, LoudSource::Wet);
                    loud_row(cx, MeterModel::mono, LoudSource::MonoWet);
                    Label::new(cx, MeterModel::matched).font_size(11.0).height(Pixels(16.0));
                })
                .layout_type(LayoutType::Column)
                .width(Pixels(330.0))
                .height(Pixels(PANEL_H))
                .class("panel");
                HelpArea::new(cx, help::TRANSLATION_HELP, |cx| {
                    Label::new(cx, "Translation  (sone, dry → processed)").class("paneltitle");
                    Label::new(cx, MeterModel::tr_full).font_size(11.0).height(Pixels(14.0));
                    Label::new(cx, MeterModel::tr_laptop).font_size(11.0).height(Pixels(14.0));
                    Label::new(cx, MeterModel::tr_phone).font_size(11.0).height(Pixels(14.0));
                    Label::new(cx, MeterModel::tr_mono).font_size(11.0).height(Pixels(14.0));
                    Label::new(cx, MeterModel::tr_flat).font_size(11.0).height(Pixels(14.0));
                    Label::new(cx, MeterModel::tr_dist).font_size(11.0).height(Pixels(14.0));
                })
                .layout_type(LayoutType::Column)
                .width(Pixels(360.0))
                .height(Pixels(PANEL_H))
                .class("panel");
                Label::new(cx, HelpModel::text)
                    .text_wrap(true)
                    .font_size(11.0)
                    .width(Stretch(1.0))
                    .height(Pixels(PANEL_H))
                    .class("panel");
            })
            .col_between(Pixels(8.0))
            .height(Pixels(PANEL_H))
            .width(Pixels(CONTENT_WIDTH));

            // --- controls: four columns, rebuilt when the view switches ---
            Binding::new(cx, ViewModel::advanced, |cx, adv| {
                let advanced = adv.get(cx);
                let p = Data::params.get(cx);
                HStack::new(cx, |cx| {
                    VStack::new(cx, |cx| {
                        group(cx, "Clip", visible_rows(p.clip.as_ref(), advanced), |cx| params_with_help(cx, Data::params.map(|p| p.clip.clone()), advanced));
                        HStack::new(cx, |cx| {
                            meter(cx, "Clip GR", Data::meters.map(|m| m.gr_post_db.load(Ordering::Relaxed)), help::GR_POST_HELP);
                        })
                        .size(Auto)
                        .top(Pixels(4.0));
                    })
                    .width(COLUMN_WIDTH)
                    .height(Auto);

                    VStack::new(cx, |cx| {
                        group(cx, "Threshold", visible_rows(p.threshold.as_ref(), advanced), |cx| params_with_help(cx, Data::params.map(|p| p.threshold.clone()), advanced));
                        group(cx, "Dynamics", visible_rows(p.dynamics.as_ref(), advanced), |cx| params_with_help(cx, Data::params.map(|p| p.dynamics.clone()), advanced));
                    })
                    .width(COLUMN_WIDTH)
                    .height(Auto);

                    VStack::new(cx, |cx| {
                        group(cx, "Onset Protection", visible_rows(p.protect.as_ref(), advanced), |cx| params_with_help(cx, Data::params.map(|p| p.protect.clone()), advanced));
                        if advanced {
                            group(cx, "Resolution", visible_rows(p.resolution.as_ref(), advanced), |cx| params_with_help(cx, Data::params.map(|p| p.resolution.clone()), advanced));
                        }
                        group(cx, "Output", visible_rows(p.global.as_ref(), advanced), |cx| params_with_help(cx, Data::params.map(|p| p.global.clone()), advanced));
                    })
                    .width(COLUMN_WIDTH)
                    .height(Auto);

                    VStack::new(cx, |cx| {
                        group(cx, "Stereo", visible_rows(p.stereo.as_ref(), advanced), |cx| params_with_help(cx, Data::params.map(|p| p.stereo.clone()), advanced));
                        group(cx, "Loudness Balance", visible_rows(p.balance.as_ref(), advanced), |cx| params_with_help(cx, Data::params.map(|p| p.balance.clone()), advanced));
                        group(cx, "Phase Rotate", visible_rows(p.rotate.as_ref(), advanced), |cx| params_with_help(cx, Data::params.map(|p| p.rotate.clone()), advanced));
                        group(cx, "Autopilot", visible_rows(p.autopilot.as_ref(), advanced), |cx| params_with_help(cx, Data::params.map(|p| p.autopilot.clone()), advanced));
                        HStack::new(cx, |cx| {
                            HelpArea::new(cx, help::AUTO_DRIVE_HELP, |cx| {
                                Label::new(cx, MeterModel::auto_drive).font_size(11.0).height(Pixels(16.0)).top(Pixels(5.0));
                            })
                            .height(Pixels(26.0))
                            .width(Pixels(230.0))
                            .left(Pixels(15.0));
                            let params = Data::params.get(cx);
                            let meters = Data::meters.get(cx);
                            Button::new(
                                cx,
                                move |cx| freeze_autopilot(cx, params.as_ref(), meters.as_ref()),
                                |cx| Label::new(cx, "Freeze"),
                            )
                            .on_hover(|cx| cx.emit(HelpEvent::Show(help::FREEZE_HELP)))
                            .height(Pixels(22.0))
                            .top(Pixels(2.0));
                        })
                        .height(Pixels(26.0))
                        .width(COLUMN_WIDTH);
                    })
                    .width(COLUMN_WIDTH)
                    .height(Auto);
                })
                .size(Auto);
            });
        })
        .row_between(Pixels(6.0))
        .child_top(Pixels(4.0))
        .child_left(Stretch(1.0))
        .child_right(Stretch(1.0));

        ResizeHandle::new(cx);
    })
}

/// One loudness row: label + bar.
fn loud_row(cx: &mut Context, text: impl Lens<Target = String>, src: LoudSource) {
    HStack::new(cx, |cx| {
        Label::new(cx, text).font_size(11.0).width(Pixels(170.0)).top(Pixels(3.0));
        LoudBar::new(cx, Data::meters, src).width(Stretch(1.0)).height(Pixels(20.0));
    })
    .height(Pixels(22.0))
    .col_between(Pixels(6.0));
}

/// Write Clip Drive + auto drive into Clip Drive and switch Autopilot off: the learned
/// setting becomes static.
fn freeze_autopilot(cx: &mut EventContext, params: &OnsetGuardParams, meters: &Meters) {
    let auto = meters.auto_drive_db.load(Ordering::Relaxed);
    if params.autopilot.on.value() {
        let total = params.clip.drive_db.value() + auto;
        let drive = &params.clip.drive_db;
        set_param(cx, drive.as_ptr(), drive.preview_normalized(total));
        let on = &params.autopilot.on;
        set_param(cx, on.as_ptr(), on.preview_normalized(false));
    }
}

/// Set every parameter: listed ones to the preset value, the rest to their defaults.
fn apply_preset(cx: &mut EventContext, params: &OnsetGuardParams, values: &[(&str, f32)]) {
    for (id, ptr, _) in params.param_map() {
        // never let a preset change level, bypass, the meter calibration, or the mix
        // decisions (stereo, loudness balance, pre-EQ)
        if matches!(id.as_str(), "output" | "mix" | "matched_bypass" | "ref_level" | "width_hz" | "width" | "guard")
            || id.starts_with("balance")
            || id.starts_with("eq")
            || id.starts_with("uz")
            || matches!(id.as_str(), "rotate" | "rotate_hz" | "rotate_hits" | "autopilot" | "target" | "crest_floor" | "corr_floor")
        {
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
fn params_with_help<L, PsRef, Ps>(cx: &mut Context, params: L, advanced: bool)
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
        let id = ids.get(&param_ptr).map(|s| s.as_str()).unwrap_or("");
        if !shown(id, advanced) {
            return;
        }
        let help_text = help::for_param(id).unwrap_or("");
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
