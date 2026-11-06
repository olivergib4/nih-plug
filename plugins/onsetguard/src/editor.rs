//! Editor: controls on the left, hit analysis on the right (hit profile graph, readout table with
//! the reference and deltas), reference loading.

use nih_plug::prelude::Editor;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::vizia::vg;
use nih_plug_vizia::widgets::*;
use nih_plug_vizia::{assets, create_vizia_editor, ViziaState, ViziaTheming};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::analysis::{Report, BANDS, LEVEL_WINS_MS, RAIL_WINS_MS};
use crate::reference::ReferenceData;
use crate::{BloaterParams, Control, Snapshot};

const GUI_WIDTH: u32 = 1000;
const GUI_HEIGHT: u32 = 640;
const LABEL_W: f32 = 108.0;
const SLIDER_W: f32 = 176.0;

#[derive(Clone, Default)]
pub struct Row {
    pub name: String,
    pub live: String,
    pub reference: String,
    pub delta: String,
}

#[derive(Lens)]
pub struct Data {
    pub params: Arc<BloaterParams>,
    pub control: Arc<Control>,
    pub snap: Arc<Mutex<triple_buffer::Output<Snapshot>>>,
    pub live: Snapshot,
    pub reference: Option<ReferenceData>,
    pub ref_status: String,
    pub rows: Vec<Row>,
    pub busy: bool,
    pub tick: u64,
}

pub enum Msg {
    Tick,
    LoadReference,
    ReferenceDone(Result<ReferenceData, String>),
    ClearReference,
    ResetStats,
}

fn fmt(v: f32, digits: usize, unit: &str) -> String {
    if v.is_nan() {
        "–".into()
    } else {
        format!("{v:.*}{unit}", digits)
    }
}

fn delta(live: f32, reference: Option<f32>, digits: usize, unit: &str) -> String {
    match reference {
        Some(r) if !live.is_nan() && !r.is_nan() => format!("{:+.*}{unit}", digits, live - r),
        _ => String::new(),
    }
}

fn rows(live: &Report, reference: Option<&Report>) -> Vec<Row> {
    let mut out = Vec::new();
    let mut push = |name: &str, f: &dyn Fn(&Report) -> f32, digits: usize, unit: &str| {
        let l = f(live);
        let r = reference.map(f);
        out.push(Row {
            name: name.into(),
            live: fmt(l, digits, unit),
            reference: r.map(|v| fmt(v, digits, unit)).unwrap_or_default(),
            delta: delta(l, r, digits, unit),
        });
    };
    push("pinned, total", &|r| r.pinned_pct, 1, " %");
    push("pinned, inside hits", &|r| r.pinned_hits_pct, 1, " %");
    push("pinned, between hits", &|r| r.pinned_rest_pct, 1, " %");
    push("loudness, mean", &|r| r.phon, 1, " phon");
    push("loudness at claps", &|r| r.phon_hits, 1, " phon");
    push("loudness between", &|r| r.phon_between, 1, " phon");
    for b in 2..BANDS.len() {
        push(&format!("drums over rest {}", BANDS[b].0), &move |r| r.band_delta(b), 1, " dB");
    }
    for b in 2..BANDS.len() {
        push(&format!("rest level {}", BANDS[b].0), &move |r| r.band_rest_db[b], 1, " dB");
    }
    push("500-6k spread (p90-p10)", &|r| r.spread_db, 1, " dB");
    push("250-1k peakiness (p99-med)", &|r| r.lowmid_peak_db, 1, " dB");
    for b in 2..BANDS.len() {
        push(&format!("level {}", BANDS[b].0), &move |r| r.band_all_db[b], 1, " dB");
    }
    push("clap 5-10k at 25-60 ms", &|r| r.tail_db, 1, " dB");
    out
}

impl Model for Data {
    fn event(&mut self, cx: &mut EventContext, event: &mut Event) {
        event.map(|e, _| match e {
            Msg::Tick => {
                let s = { *self.snap.lock().unwrap().read() };
                self.live = s;
                self.tick = self.tick.wrapping_add(1);
                self.rows = rows(&s.report, self.reference.as_ref().map(|r| &r.report));
            }
            Msg::ResetStats => {
                self.control.reset_stats.store(true, Ordering::Relaxed);
            }
            Msg::ClearReference => {
                self.reference = None;
                *self.params.reference_json.lock().unwrap() = String::new();
                self.ref_status = "no reference".into();
            }
            Msg::LoadReference => {
                if self.busy {
                    return;
                }
                self.busy = true;
                self.ref_status = "choose a file…".into();
                cx.spawn(|proxy| {
                    let result = match pick_file() {
                        Some(path) => {
                            let _ = proxy.emit(Msg::ReferenceDone(Err(format!("analysing {}…", path.file_name().and_then(|n| n.to_str()).unwrap_or("")))));
                            crate::reference::analyze_file(&path)
                        }
                        None => Err(String::new()),
                    };
                    let _ = proxy.emit(Msg::ReferenceDone(result));
                });
            }
            Msg::ReferenceDone(res) => match res {
                Ok(r) => {
                    self.busy = false;
                    self.ref_status = format!("{}: {} claps, half-bar {:.3} s, from {:.0} s", r.name, r.claps, r.period_s, r.window_start_s);
                    if let Ok(j) = serde_json::to_string(r) {
                        *self.params.reference_json.lock().unwrap() = j;
                    }
                    self.reference = Some(r.clone());
                    self.rows = rows(&self.live.report, Some(&r.report));
                }
                Err(msg) if msg.starts_with("analysing") => {
                    self.ref_status = msg.clone();
                }
                Err(msg) => {
                    self.busy = false;
                    self.ref_status = if msg.is_empty() { "no file chosen".into() } else { format!("reference failed: {msg}") };
                }
            },
        });
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn pick_file() -> Option<std::path::PathBuf> {
    rfd::FileDialog::new()
        .set_title("Reference track")
        .add_filter("audio", &["wav", "mp3", "flac", "aif", "aiff", "m4a", "ogg"])
        .pick_file()
}

#[cfg(not(any(windows, target_os = "macos")))]
fn pick_file() -> Option<std::path::PathBuf> {
    std::env::var("BLOATER_REFERENCE").ok().map(std::path::PathBuf::from)
}

/// The hit profile graph: clap level per window (left) and rail share per window (right), live
/// median filled, latest hit outlined, reference as a marker line per bar.
pub struct ProfileGraph;

impl ProfileGraph {
    pub fn new(cx: &mut Context) -> Handle<'_, Self> {
        Self.build(cx, |cx| {
            // redraw on every analysis tick
            Binding::new(cx, Data::tick, |cx, _| cx.needs_redraw());
        })
    }
}

const DB_LO: f32 = -24.0;
const BAR_FILL: vg::Color = vg::Color::rgbaf(0.42, 0.58, 0.78, 0.9);
const BAR_LAST: vg::Color = vg::Color::rgbaf(0.9, 0.9, 0.9, 0.8);
const REF_COLOR: vg::Color = vg::Color::rgbaf(1.0, 0.72, 0.25, 1.0);
const GRID: vg::Color = vg::Color::rgbaf(0.5, 0.5, 0.5, 0.3);

impl View for ProfileGraph {
    fn element(&self) -> Option<&'static str> {
        Some("profilegraph")
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &mut Canvas) {
        let b = cx.bounds();
        if b.w <= 0.0 || b.h <= 0.0 {
            return;
        }
        let live = Data::live.get(cx);
        let reference = Data::reference.get(cx);
        let mut bg = vg::Path::new();
        bg.rect(b.x, b.y, b.w, b.h);
        canvas.fill_path(&bg, &vg::Paint::color(vg::Color::rgbaf(0.08, 0.08, 0.09, 1.0)));
        let gap = 24.0;
        let half = (b.w - gap) / 2.0;
        let panels = [(b.x, half), (b.x + half + gap, half)];
        let has_profile = live.report.claps > 0;
        for (p, (px, pw)) in panels.iter().enumerate() {
            let n = if p == 0 { LEVEL_WINS_MS.len() } else { RAIL_WINS_MS.len() };
            let bw = pw / n as f32;
            // grid: 6 dB / 25 % lines
            let lines = if p == 0 { 4 } else { 4 };
            let mut g = vg::Path::new();
            for k in 0..=lines {
                let y = b.y + b.h * k as f32 / lines as f32;
                g.move_to(*px, y);
                g.line_to(px + pw, y);
            }
            canvas.stroke_path(&g, &vg::Paint::color(GRID).with_line_width(1.0));
            let t_of = |v: f32| -> f32 {
                if p == 0 {
                    ((v - DB_LO) / (0.0 - DB_LO)).clamp(0.0, 1.0)
                } else {
                    (v / 100.0).clamp(0.0, 1.0)
                }
            };
            for k in 0..n {
                let x0 = px + bw * k as f32 + 3.0;
                let w = bw - 6.0;
                let med = if p == 0 { live.report.level_db[k] } else { live.report.rail_pct[k] };
                let last = if p == 0 { live.last.level_db[k] } else { live.last.rail_pct[k] };
                if has_profile && !med.is_nan() {
                    let h = b.h * t_of(med);
                    let mut bar = vg::Path::new();
                    bar.rect(x0, b.y + b.h - h, w, h);
                    canvas.fill_path(&bar, &vg::Paint::color(BAR_FILL));
                }
                if live.has_last && !last.is_nan() {
                    let y = b.y + b.h - b.h * t_of(last);
                    let mut l = vg::Path::new();
                    l.move_to(x0, y);
                    l.line_to(x0 + w, y);
                    canvas.stroke_path(&l, &vg::Paint::color(BAR_LAST).with_line_width(1.0));
                }
                if let Some(r) = &reference {
                    let v = if p == 0 { r.report.level_db[k] } else { r.report.rail_pct[k] };
                    if !v.is_nan() {
                        let y = b.y + b.h - b.h * t_of(v);
                        let mut l = vg::Path::new();
                        l.move_to(x0 - 1.0, y);
                        l.line_to(x0 + w + 1.0, y);
                        canvas.stroke_path(&l, &vg::Paint::color(REF_COLOR).with_line_width(2.5));
                    }
                }
            }
        }
        let mut border = vg::Path::new();
        border.rect(b.x + 0.5, b.y + 0.5, b.w - 1.0, b.h - 1.0);
        canvas.stroke_path(&border, &vg::Paint::color(vg::Color::rgbaf(0.3, 0.3, 0.32, 1.0)).with_line_width(1.0));
    }
}

pub(crate) fn default_state() -> Arc<ViziaState> {
    ViziaState::new(|| (GUI_WIDTH, GUI_HEIGHT))
}

fn section(cx: &mut Context, title: &str) {
    Label::new(cx, title).class("section");
}

fn row<P, FMap>(cx: &mut Context, name: &str, map: FMap)
where
    P: nih_plug::prelude::Param + 'static,
    FMap: Fn(&Arc<BloaterParams>) -> &P + Copy + 'static,
{
    HStack::new(cx, |cx| {
        Label::new(cx, name).class("plabel").width(Pixels(LABEL_W));
        ParamSlider::new(cx, Data::params, map).width(Pixels(SLIDER_W));
    })
    .height(Pixels(24.0))
    .col_between(Pixels(6.0));
}

fn toggle_row<P, FMap>(cx: &mut Context, name: &str, map: FMap)
where
    P: nih_plug::prelude::Param + 'static,
    FMap: Fn(&Arc<BloaterParams>) -> &P + Copy + 'static,
{
    HStack::new(cx, |cx| {
        Label::new(cx, name).class("plabel").width(Pixels(LABEL_W));
        ParamButton::new(cx, Data::params, map).width(Pixels(SLIDER_W));
    })
    .height(Pixels(24.0))
    .col_between(Pixels(6.0));
}

pub(crate) fn create(
    params: Arc<BloaterParams>,
    control: Arc<Control>,
    snap: Arc<Mutex<triple_buffer::Output<Snapshot>>>,
    editor_state: Arc<ViziaState>,
) -> Option<Box<dyn Editor>> {
    let state_for_ticker = editor_state.clone();
    create_vizia_editor(editor_state, ViziaTheming::Custom, move |cx, _| {
        assets::register_noto_sans_light(cx);
        assets::register_noto_sans_thin(cx);
        if let Err(err) = cx.add_stylesheet(include_style!("src/theme.css")) {
            nih_plug::nih_error!("Failed to load stylesheet: {err:?}")
        }
        let stored = params.reference_json.lock().unwrap().clone();
        let reference: Option<ReferenceData> = if stored.is_empty() { None } else { serde_json::from_str(&stored).ok() };
        let ref_status = match &reference {
            Some(r) => format!("{}: {} claps, half-bar {:.3} s, from {:.0} s", r.name, r.claps, r.period_s, r.window_start_s),
            None => "no reference loaded".into(),
        };
        Data {
            params: params.clone(),
            control: control.clone(),
            snap: snap.clone(),
            live: Snapshot::default(),
            rows: rows(&Report::default(), reference.as_ref().map(|r| &r.report)),
            reference,
            ref_status,
            busy: false,
            tick: 0,
        }
        .build(cx);

        let state = state_for_ticker.clone();
        cx.spawn(move |proxy| {
            std::thread::sleep(Duration::from_millis(200));
            while state.is_open() {
                if proxy.emit(Msg::Tick).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });

        VStack::new(cx, |cx| {
            // title bar
            HStack::new(cx, |cx| {
                Label::new(cx, "Bloater")
                    .font_family(vec![FamilyOwned::Name(String::from(assets::NOTO_SANS))])
                    .font_weight(FontWeightKeyword::Thin)
                    .font_size(26.0)
                    .right(Pixels(16.0));
                Label::new(cx, "hard clip at 0 dBFS · shape the input, read the output").class("sub").top(Pixels(10.0));
            })
            .height(Pixels(40.0));

            HStack::new(cx, |cx| {
                // ---- controls ----
                VStack::new(cx, |cx| {
                    section(cx, "Clip");
                    row(cx, "Mode", |p| &p.mode);
                    toggle_row(cx, "Clip", |p| &p.clip);
                    row(cx, "Drive", |p| &p.drive);
                    section(cx, "EQ before the clip");
                    row(cx, "Tilt @ 1 kHz", |p| &p.tilt);
                    toggle_row(cx, "Bell", |p| &p.bell);
                    row(cx, "Bell freq", |p| &p.bell_hz);
                    row(cx, "Bell gain", |p| &p.bell_db);
                    section(cx, "Band leveler");
                    toggle_row(cx, "Leveler", |p| &p.lev);
                    row(cx, "Low", |p| &p.lev_lo);
                    row(cx, "High", |p| &p.lev_hi);
                    row(cx, "Threshold", |p| &p.lev_thr);
                    row(cx, "Ratio", |p| &p.lev_ratio);
                    row(cx, "Attack", |p| &p.lev_att);
                    row(cx, "Release", |p| &p.lev_rel);
                    section(cx, "Sub split");
                    toggle_row(cx, "Sub split", |p| &p.sub);
                    row(cx, "Crossover", |p| &p.sub_hz);
                    row(cx, "Sub ceiling", |p| &p.sub_ceil);
                    section(cx, "Hit detection");
                    row(cx, "Sensitivity", |p| &p.sens);
                    row(cx, "Profile", |p| &p.pick);
                })
                .width(Pixels(LABEL_W + SLIDER_W + 20.0))
                .row_between(Pixels(2.0))
                .class("panel");

                // ---- analysis ----
                VStack::new(cx, |cx| {
                    HStack::new(cx, |cx| {
                        Label::new(cx, "Hit profile — clap level (dBFS, −24…0) and rail share (0…100 %) after the hit").class("section");
                        Label::new(cx, Data::live.map(|s| format!("{} hits, {} profiled", s.hits, s.report.claps))).class("sub").left(Stretch(1.0));
                    })
                    .height(Pixels(20.0));
                    ProfileGraph::new(cx).height(Pixels(190.0));
                    HStack::new(cx, |cx| {
                        for (a, b) in LEVEL_WINS_MS {
                            Label::new(cx, &format!("{}-{}", a as i32, b as i32)).class("tick").width(Stretch(1.0));
                        }
                        Label::new(cx, "").width(Pixels(24.0));
                        for (a, b) in RAIL_WINS_MS {
                            Label::new(cx, &format!("{}-{}", a as i32, b as i32)).class("tick").width(Stretch(1.0));
                        }
                    })
                    .height(Pixels(16.0));
                    HStack::new(cx, |cx| {
                        Label::new(cx, "■ live median").class("legend").color(Color::rgb(107, 148, 199));
                        Label::new(cx, "— last hit").class("legend").color(Color::rgb(230, 230, 230));
                        Label::new(cx, "— reference").class("legend").color(Color::rgb(255, 184, 64));
                        Label::new(
                            cx,
                            Data::live.map(|s| format!("into clip {:+.1} dBFS   block pinned {:.0} %   leveler {:+.1} dB", s.in_peak_db, 100.0 * s.block_pinned, s.lev_gr_db)),
                        )
                        .class("sub")
                        .left(Stretch(1.0));
                    })
                    .height(Pixels(18.0))
                    .col_between(Pixels(14.0));

                    // table
                    HStack::new(cx, |cx| {
                        Label::new(cx, "metric").class("th").width(Pixels(210.0));
                        Label::new(cx, "live").class("th").width(Pixels(110.0));
                        Label::new(cx, "reference").class("th").width(Pixels(110.0));
                        Label::new(cx, "live − ref").class("th").width(Pixels(110.0));
                    })
                    .height(Pixels(18.0))
                    .top(Pixels(6.0));
                    List::new(cx, Data::rows, |cx, _, item| {
                        HStack::new(cx, |cx| {
                            Label::new(cx, item.map(|r| r.name.clone())).class("td").width(Pixels(210.0));
                            Label::new(cx, item.map(|r| r.live.clone())).class("td").width(Pixels(110.0));
                            Label::new(cx, item.map(|r| r.reference.clone())).class("td").class("ref").width(Pixels(110.0));
                            Label::new(cx, item.map(|r| r.delta.clone())).class("td").width(Pixels(110.0));
                        })
                        .height(Pixels(17.0));
                    });

                    // reference + actions
                    HStack::new(cx, |cx| {
                        Button::new(cx, |cx| cx.emit(Msg::LoadReference), |cx| Label::new(cx, "Load reference…"));
                        Button::new(cx, |cx| cx.emit(Msg::ClearReference), |cx| Label::new(cx, "Clear"));
                        Button::new(cx, |cx| cx.emit(Msg::ResetStats), |cx| Label::new(cx, "Reset stats"));
                        Label::new(cx, Data::ref_status).class("sub").top(Pixels(6.0)).left(Pixels(8.0));
                    })
                    .height(Pixels(30.0))
                    .top(Pixels(8.0))
                    .col_between(Pixels(8.0));
                })
                .class("panel")
                .width(Stretch(1.0));
            })
            .col_between(Pixels(8.0));
        })
        .child_space(Pixels(10.0))
        .row_between(Pixels(6.0));
    })
}
