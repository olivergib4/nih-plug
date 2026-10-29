//! Spectrum analyzer: envelope spectrum (what the detector sees), threshold curve, and the
//! per-frequency gain applied by the two spectral dynamics stages. Data is sampled on a fixed
//! log-frequency grid so both resolutions draw as one curve.

use crate::dsp::eq::{EqSettings, PreEq, NODES};
use crate::OnsetGuardParams;
use nih_plug::prelude::Param;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::vizia::vg;
use nih_plug_vizia::widgets::RawParamEvent;
use std::sync::{Arc, Mutex};

pub const GRID: usize = 320;
pub const F_LO: f32 = 30.0;
pub const F_HI: f32 = 20_000.0;

#[derive(Clone)]
pub struct AnalyzerData {
    pub env_db: [f32; GRID],
    pub gain_db: [f32; GRID],
    pub thr_db: [f32; GRID],
    /// 1 while a protection window is active (the processed signal is faded toward the reference).
    pub protect: f32,
}

impl Default for AnalyzerData {
    fn default() -> Self {
        AnalyzerData {
            env_db: [-100.0; GRID],
            gain_db: [0.0; GRID],
            thr_db: [-24.0; GRID],
            protect: 0.0,
        }
    }
}

const SPECTRUM_FILL: vg::Color = vg::Color::rgbaf(0.55, 0.60, 0.66, 0.85);
const THRESHOLD_COLOR: vg::Color = vg::Color::rgbaf(0.95, 0.55, 0.25, 0.95);
const GAIN_DOWN_COLOR: vg::Color = vg::Color::rgbaf(0.30, 0.65, 1.0, 0.75);
const GAIN_UP_COLOR: vg::Color = vg::Color::rgbaf(0.35, 0.85, 0.55, 0.75);
const PROTECT_COLOR: vg::Color = vg::Color::rgbaf(1.0, 0.85, 0.3, 0.9);
const GRID_COLOR: vg::Color = vg::Color::rgbaf(0.5, 0.5, 0.5, 0.35);
const EQ_COLOR: vg::Color = vg::Color::rgbaf(0.80, 0.55, 1.0, 0.95);
const EQ_FILL: vg::Color = vg::Color::rgbaf(0.80, 0.55, 1.0, 0.12);
/// The pre-EQ curve is drawn around the vertical middle: +-EQ_SPAN_DB spans the full height.
const EQ_SPAN_DB: f32 = 15.0;
const NODE_RADIUS: f32 = 6.0;
const EQ_SR: f32 = 48000.0;

const DB_TOP: f32 = 12.0;
const DB_BOTTOM: f32 = -84.0;

pub struct Analyzer {
    data: Arc<Mutex<triple_buffer::Output<AnalyzerData>>>,
    params: Arc<OnsetGuardParams>,
    /// Node being dragged, if any.
    drag: Option<usize>,
    /// Node under the cursor (for highlighting).
    hover: Option<usize>,
}

impl Analyzer {
    pub fn new<L, P>(cx: &mut Context, data: L, params: P) -> Handle<'_, Self>
    where
        L: Lens<Target = Arc<Mutex<triple_buffer::Output<AnalyzerData>>>>,
        P: Lens<Target = Arc<OnsetGuardParams>>,
    {
        Self { data: data.get(cx), params: params.get(cx), drag: None, hover: None }.build(cx, |_| ())
    }

    fn eq_settings(&self) -> EqSettings {
        let mut s = EqSettings::default();
        for n in 0..NODES {
            s.freq_hz[n] = self.params.eq.freq(n).value();
            s.gain_db[n] = self.params.eq.gain(n).value();
        }
        s
    }

    /// Node positions in the view's coordinate frame.
    fn node_xy(&self, b: &BoundingBox) -> [(f32, f32); NODES] {
        let mut out = [(0.0, 0.0); NODES];
        for (n, o) in out.iter_mut().enumerate() {
            let f = self.params.eq.freq(n).value().clamp(F_LO, F_HI);
            let g = self.params.eq.gain(n).value();
            let tx = (f.ln() - F_LO.ln()) / (F_HI.ln() - F_LO.ln());
            *o = (b.x + b.w * tx, b.y + b.h * (0.5 - g / (2.0 * EQ_SPAN_DB)));
        }
        out
    }

    fn node_at(&self, b: &BoundingBox, x: f32, y: f32) -> Option<usize> {
        let pts = self.node_xy(b);
        let mut best = None;
        let mut best_d = (NODE_RADIUS * 2.5).powi(2);
        for (n, (px, py)) in pts.iter().enumerate() {
            let d = (px - x).powi(2) + (py - y).powi(2);
            if d < best_d {
                best_d = d;
                best = Some(n);
            }
        }
        best
    }

    fn set(&self, cx: &mut EventContext, n: usize, freq: Option<f32>, gain: Option<f32>) {
        if let Some(f) = freq {
            let p = self.params.eq.freq(n);
            let ptr = p.as_ptr();
            let norm = p.preview_normalized(f.clamp(20.0, 20000.0));
            cx.emit(RawParamEvent::BeginSetParameter(ptr));
            cx.emit(RawParamEvent::SetParameterNormalized(ptr, norm));
            cx.emit(RawParamEvent::EndSetParameter(ptr));
        }
        if let Some(g) = gain {
            let p = self.params.eq.gain(n);
            let ptr = p.as_ptr();
            let norm = p.preview_normalized(g.clamp(-12.0, 12.0));
            cx.emit(RawParamEvent::BeginSetParameter(ptr));
            cx.emit(RawParamEvent::SetParameterNormalized(ptr, norm));
            cx.emit(RawParamEvent::EndSetParameter(ptr));
        }
    }
}

#[inline]
fn db_t(db: f32) -> f32 {
    ((db - DB_BOTTOM) / (DB_TOP - DB_BOTTOM)).clamp(0.0, 1.0)
}

impl View for Analyzer {
    fn element(&self) -> Option<&'static str> {
        Some("analyzer")
    }

    fn event(&mut self, cx: &mut EventContext, event: &mut Event) {
        let b = cx.bounds();
        event.map(|window_event, meta| match window_event {
            WindowEvent::MouseMove(x, y) => {
                if let Some(n) = self.drag {
                    let tx = ((x - b.x) / b.w.max(1.0)).clamp(0.0, 1.0);
                    let f = (F_LO.ln() + tx * (F_HI.ln() - F_LO.ln())).exp();
                    let g = (0.5 - (y - b.y) / b.h.max(1.0)) * 2.0 * EQ_SPAN_DB;
                    self.set(cx, n, Some(f), Some(g));
                    cx.needs_redraw();
                } else {
                    let h = self.node_at(&b, *x, *y);
                    if h != self.hover {
                        self.hover = h;
                        cx.needs_redraw();
                    }
                }
            }
            WindowEvent::MouseDown(MouseButton::Left) => {
                let (x, y) = (cx.mouse().left.pos_down.0, cx.mouse().left.pos_down.1);
                if let Some(n) = self.node_at(&b, x, y) {
                    self.drag = Some(n);
                    cx.capture();
                    cx.focus_with_visibility(false);
                    meta.consume();
                }
            }
            WindowEvent::MouseUp(MouseButton::Left) => {
                if self.drag.take().is_some() {
                    cx.release();
                    cx.needs_redraw();
                }
            }
            WindowEvent::MouseDoubleClick(MouseButton::Left) => {
                let (x, y) = (cx.mouse().left.pos_down.0, cx.mouse().left.pos_down.1);
                if let Some(n) = self.node_at(&b, x, y) {
                    self.set(cx, n, None, Some(0.0));
                    self.drag = None;
                    cx.release();
                    cx.needs_redraw();
                    meta.consume();
                }
            }
            _ => {}
        });
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &mut Canvas) {
        let b = cx.bounds();
        if b.w <= 0.0 || b.h <= 0.0 {
            return;
        }
        let data = {
            let mut guard = self.data.lock().unwrap();
            guard.read().clone()
        };
        let x_of = |i: usize| b.x + b.w * i as f32 / (GRID - 1) as f32;
        let y_of = |db: f32| b.y + b.h * (1.0 - db_t(db));

        // background
        let mut bg = vg::Path::new();
        bg.rect(b.x, b.y, b.w, b.h);
        canvas.fill_path(&bg, &vg::Paint::color(vg::Color::rgbaf(0.08, 0.08, 0.09, 1.0)));

        // frequency grid lines (decades + 2/5 multiples) and dB lines
        let mut grid = vg::Path::new();
        for f in [50.0f32, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0] {
            let t = (f.ln() - F_LO.ln()) / (F_HI.ln() - F_LO.ln());
            if t > 0.0 && t < 1.0 {
                grid.move_to(b.x + b.w * t, b.y);
                grid.line_to(b.x + b.w * t, b.y + b.h);
            }
        }
        for db in [-72.0f32, -60.0, -48.0, -36.0, -24.0, -12.0, 0.0] {
            grid.move_to(b.x, y_of(db));
            grid.line_to(b.x + b.w, y_of(db));
        }
        canvas.stroke_path(&grid, &vg::Paint::color(GRID_COLOR).with_line_width(1.0));

        // spectrum mesh
        let mut mesh = vg::Path::new();
        mesh.move_to(b.x, b.y + b.h);
        for i in 0..GRID {
            mesh.line_to(x_of(i), y_of(data.env_db[i]));
        }
        mesh.line_to(b.x + b.w, b.y + b.h);
        mesh.close();
        canvas.fill_path(&mesh, &vg::Paint::color(SPECTRUM_FILL).with_anti_alias(false));

        // gain overlay: bars from the threshold line (0 dB change) up or down
        let mut up = vg::Path::new();
        let mut down = vg::Path::new();
        let mid = b.y + b.h * 0.5;
        let px = b.w / (GRID - 1) as f32;
        for i in 0..GRID {
            let g = data.gain_db[i];
            if g.abs() < 0.15 {
                continue;
            }
            // 30 dB of gain change spans half the height
            let dy = (g / 30.0).clamp(-1.0, 1.0) * b.h * 0.5;
            let x = x_of(i) - px * 0.5;
            if g < 0.0 {
                down.rect(x, mid, px + 0.5, -dy);
            } else {
                up.rect(x, mid - dy, px + 0.5, dy);
            }
        }
        canvas.fill_path(&down, &vg::Paint::color(GAIN_DOWN_COLOR).with_anti_alias(false));
        canvas.fill_path(&up, &vg::Paint::color(GAIN_UP_COLOR).with_anti_alias(false));

        // threshold curve
        let mut thr = vg::Path::new();
        for i in 0..GRID {
            if i == 0 {
                thr.move_to(x_of(i), y_of(data.thr_db[i]));
            } else {
                thr.line_to(x_of(i), y_of(data.thr_db[i]));
            }
        }
        canvas.scissor(b.x, b.y, b.w, b.h);
        canvas.stroke_path(&thr, &vg::Paint::color(THRESHOLD_COLOR).with_line_width(2.0));
        canvas.reset_scissor();

        // pre-EQ curve and nodes (around the vertical middle; +-EQ_SPAN_DB spans the height)
        {
            let settings = self.eq_settings();
            let mut eq = PreEq::new(EQ_SR, 1);
            eq.update(&settings);
            let mid = b.y + b.h * 0.5;
            let y_eq = |g: f32| mid - g / (2.0 * EQ_SPAN_DB) * b.h;
            let mut curve = vg::Path::new();
            let mut fill = vg::Path::new();
            fill.move_to(b.x, mid);
            for i in 0..GRID {
                let t = i as f32 / (GRID - 1) as f32;
                let f = (F_LO.ln() + t * (F_HI.ln() - F_LO.ln())).exp();
                let g = 20.0 * eq.magnitude_at(f).max(1e-6).log10();
                let (x, y) = (x_of(i), y_eq(g));
                if i == 0 {
                    curve.move_to(x, y);
                } else {
                    curve.line_to(x, y);
                }
                fill.line_to(x, y);
            }
            fill.line_to(b.x + b.w, mid);
            fill.close();
            canvas.scissor(b.x, b.y, b.w, b.h);
            if eq.active() {
                canvas.fill_path(&fill, &vg::Paint::color(EQ_FILL));
            }
            canvas.stroke_path(&curve, &vg::Paint::color(EQ_COLOR).with_line_width(1.5));
            for (n, (px, py)) in self.node_xy(&b).iter().enumerate() {
                let lit = self.drag == Some(n) || self.hover == Some(n);
                let mut dot = vg::Path::new();
                dot.circle(*px, *py, if lit { NODE_RADIUS + 1.5 } else { NODE_RADIUS });
                canvas.fill_path(&dot, &vg::Paint::color(if lit { vg::Color::rgbaf(1.0, 1.0, 1.0, 0.95) } else { EQ_COLOR }));
                canvas.stroke_path(&dot, &vg::Paint::color(vg::Color::rgbaf(0.08, 0.08, 0.09, 1.0)).with_line_width(1.5));
            }
            canvas.reset_scissor();
        }

        // protection indicator: a bar along the top edge while a hit is being guarded
        if data.protect > 0.01 {
            let mut p = vg::Path::new();
            p.rect(b.x, b.y, b.w * data.protect, 4.0);
            canvas.fill_path(&p, &vg::Paint::color(PROTECT_COLOR));
        }

        // border
        let mut border = vg::Path::new();
        border.rect(b.x + 0.5, b.y + 0.5, b.w - 1.0, b.h - 1.0);
        canvas.stroke_path(&border, &vg::Paint::color(vg::Color::rgbaf(0.3, 0.3, 0.32, 1.0)).with_line_width(1.0));
    }
}

/// Loudness bars: dry (unity) and processed, in phon (0-120). Text is drawn by Labels in the
/// editor, since canvas text has no font in this Vizia build.
pub struct SoneBars {
    meters: Arc<crate::Meters>,
}

impl SoneBars {
    pub fn new<L>(cx: &mut Context, meters: L) -> Handle<'_, Self>
    where
        L: Lens<Target = Arc<crate::Meters>>,
    {
        Self { meters: meters.get(cx) }.build(cx, |_| ())
    }
}

impl View for SoneBars {
    fn element(&self) -> Option<&'static str> {
        Some("sonebars")
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &mut Canvas) {
        use std::sync::atomic::Ordering;
        let b = cx.bounds();
        if b.w <= 0.0 || b.h <= 0.0 {
            return;
        }
        let dry = self.meters.dry_sone.load(Ordering::Relaxed);
        let wet = self.meters.wet_sone.load(Ordering::Relaxed);
        let phon = |s: f32| if s <= 1e-3 { 0.0 } else { 40.0 + 10.0 * s.log2() };
        let row_h = b.h / 2.0;
        for (row, (sone, color)) in [
            (dry, vg::Color::rgbaf(0.55, 0.60, 0.66, 0.9)),
            (wet, vg::Color::rgbaf(0.95, 0.55, 0.25, 0.9)),
        ]
        .iter()
        .enumerate()
        {
            let y = b.y + row as f32 * row_h;
            let mut bg = vg::Path::new();
            bg.rect(b.x, y + 3.0, b.w, row_h - 6.0);
            canvas.fill_path(&bg, &vg::Paint::color(vg::Color::rgbaf(0.15, 0.15, 0.16, 1.0)));
            let t = (phon(*sone) / 120.0).clamp(0.0, 1.0);
            let mut bar = vg::Path::new();
            bar.rect(b.x, y + 3.0, b.w * t, row_h - 6.0);
            canvas.fill_path(&bar, &vg::Paint::color(*color));
            for tick in [40.0f32, 60.0, 80.0, 100.0] {
                let tx = b.x + b.w * tick / 120.0;
                let mut tp = vg::Path::new();
                tp.move_to(tx, y + 3.0);
                tp.line_to(tx, y + row_h - 3.0);
                canvas.stroke_path(&tp, &vg::Paint::color(vg::Color::rgbaf(0.5, 0.5, 0.5, 0.5)).with_line_width(1.0));
            }
        }
    }
}
