//! Budget: shows how loud the growls and the sub may get before they take room from the clap.
//!
//! Put one copy last on the drums, one on the growls and one on the sub (or a single "Growls + Sub bus"
//! copy after the clip the two share), set "This is" on each, and feed
//! the clap into the sidechain of all three (the same signal Clap Space listens to). The
//! copies share what they measure, so every one of them shows the whole table: each row is a level
//! relative to the clap, its budget (how far under the clap it has to stay) and how much is left.
pub mod meter;

use meter::{Meter, Readings, Role};
use nih_plug::prelude::*;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::widgets::*;
use nih_plug_vizia::{assets, create_vizia_editor, ViziaState, ViziaTheming};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

const NONE: i32 = i32::MIN;

/// What one bus last measured, in tenths of a dB, and when (milliseconds on the shared clock).
struct BusSlot {
    during: AtomicI32,
    before: AtomicI32,
    after: AtomicI32,
    rest: AtomicI32,
    stamp: AtomicU64,
}

impl BusSlot {
    const fn new() -> Self {
        BusSlot { during: AtomicI32::new(NONE), before: AtomicI32::new(NONE), after: AtomicI32::new(NONE), rest: AtomicI32::new(NONE), stamp: AtomicU64::new(0) }
    }
    fn store(&self, r: &Readings, trim_db: f32, now: u64) {
        let t = |v: Option<f32>| v.map(|v| ((v + trim_db) * 10.0).round() as i32).unwrap_or(NONE);
        self.during.store(t(r.during), Ordering::Relaxed);
        self.before.store(t(r.before), Ordering::Relaxed);
        self.after.store(t(r.after), Ordering::Relaxed);
        self.rest.store(t(r.rest), Ordering::Relaxed);
        self.stamp.store(now, Ordering::Relaxed);
    }
    /// A value, unless this bus has not reported for two seconds (its copy is gone or the transport stopped).
    fn get(&self, which: &AtomicI32, now: u64) -> Option<f32> {
        let v = which.load(Ordering::Relaxed);
        let fresh = now.saturating_sub(self.stamp.load(Ordering::Relaxed)) < 2000;
        (v != NONE && fresh).then(|| v as f32 / 10.0)
    }
}

/// Shared by every copy of the plugin in the project (they all live in the same process).
static DRUMS: BusSlot = BusSlot::new();
static GROWLS: BusSlot = BusSlot::new();
static SUB: BusSlot = BusSlot::new();

fn clock_ms() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum Bus {
    #[name = "Drums"]
    Drums,
    #[name = "Growls"]
    Growls,
    #[name = "Sub"]
    Sub,
    /// A bus that carries the growls and the sub together (after its clip): above 150 Hz counts as the
    /// growls, below as the sub.
    #[name = "Growls + Sub bus"]
    Bed,
}

#[derive(Params)]
pub struct BudgetParams {
    #[persist = "editor-state"]
    editor_state: Arc<ViziaState>,
    #[id = "bus"]
    pub bus: EnumParam<Bus>,
    #[id = "thr"]
    pub thr: FloatParam,
    /// Whatever changes this bus's level after this copy (its fader, a later clip's drive).
    #[id = "trim"]
    pub trim: FloatParam,
    /// A clip at full scale follows after that level change (the bus clip the growls and the sub run into).
    #[id = "clip_after"]
    pub clip_after: BoolParam,
    #[id = "g_after"]
    pub g_after: FloatParam,
    #[id = "g_rest"]
    pub g_rest: FloatParam,
    #[id = "g_before"]
    pub g_before: FloatParam,
    #[id = "s_under"]
    pub s_under: FloatParam,
    #[id = "s_rest"]
    pub s_rest: FloatParam,
}

fn db(name: &str, default: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min, max })
        .with_unit(" dB")
        .with_step_size(0.5)
        .with_value_to_string(formatters::v2s_f32_rounded(1))
}

impl Default for BudgetParams {
    fn default() -> Self {
        BudgetParams {
            editor_state: ViziaState::new(|| (700, 565)),
            bus: EnumParam::new("This is", Bus::Drums),
            thr: db("Trigger within", 12.0, 3.0, 40.0),
            trim: db("Level change after this", 0.0, -24.0, 24.0),
            clip_after: BoolParam::new("Then clipped", false),
            // how far under the clap each one has to stay; the defaults are what sounded right in the
            // tough session (growls) and what the reference does (sub)
            g_after: db("Budget: growls right after the clap", 8.0, 0.0, 30.0),
            g_rest: db("Budget: growls rest of the bar", 3.0, 0.0, 30.0),
            g_before: db("Budget: growls just before the clap", 20.0, 0.0, 60.0),
            s_under: db("Budget: sub under the clap", 12.0, 0.0, 60.0),
            s_rest: db("Budget: sub rest of the bar", 0.0, -6.0, 30.0),
        }
    }
}

#[derive(Default)]
pub struct Status {
    pub claps: AtomicU32,
}

pub struct Budget {
    params: Arc<BudgetParams>,
    status: Arc<Status>,
    meter: Option<Meter>,
    /// The low half of a "Growls + Sub bus" copy.
    lows: Option<Meter>,
}

impl Default for Budget {
    fn default() -> Self {
        Budget { params: Arc::new(BudgetParams::default()), status: Arc::new(Status::default()), meter: None, lows: None }
    }
}

#[derive(Lens)]
struct Data {
    params: Arc<BudgetParams>,
    status: Arc<Status>,
}

impl Model for Data {}

fn setting<P, FMap>(cx: &mut Context, label: &str, map: FMap)
where
    P: Param + 'static,
    FMap: Fn(&Arc<BudgetParams>) -> &P + Copy + 'static,
{
    HStack::new(cx, |cx| {
        Label::new(cx, label).width(Pixels(190.0)).child_top(Stretch(1.0)).child_bottom(Stretch(1.0));
        ParamSlider::new(cx, Data::params, map).width(Pixels(170.0));
    })
    .height(Pixels(32.0))
    .col_between(Pixels(8.0));
}

/// "how far under the clap it is now" and "how much of the budget is left" for one row.
fn now_and_left(level: Option<f32>, clap: Option<f32>, budget: f32) -> (String, String) {
    match (level, clap) {
        (Some(l), Some(c)) => {
            let under = c - l;
            let left = under - budget;
            let now = if under >= 0.0 { format!("{under:.1} dB under") } else { format!("{:.1} dB OVER the clap", -under) };
            let left = if left >= 0.0 { format!("{left:.1} dB left") } else { format!("OVER by {:.1} dB", -left) };
            (now, left)
        }
        (None, _) => (String::from("-"), String::from("no reading")),
        (_, None) => (String::from("-"), String::from("no clap level")),
    }
}

/// One line of the table: what, where it is now, its budget (a slider) and what is left.
fn line(cx: &mut Context, label: &str, slot: &'static BusSlot, which: fn(&'static BusSlot) -> &'static AtomicI32, map: fn(&Arc<BudgetParams>) -> &FloatParam) {
    HStack::new(cx, |cx| {
        Label::new(cx, label).width(Pixels(150.0)).child_top(Stretch(1.0)).child_bottom(Stretch(1.0));
        Label::new(
            cx,
            Data::params.map(move |p| {
                let now = clock_ms();
                now_and_left(slot.get(which(slot), now), DRUMS.get(&DRUMS.during, now), map(p).value()).0
            }),
        )
        .width(Pixels(150.0))
        .child_top(Stretch(1.0))
        .child_bottom(Stretch(1.0));
        ParamSlider::new(cx, Data::params, map).width(Pixels(130.0));
        Label::new(
            cx,
            Data::params.map(move |p| {
                let now = clock_ms();
                now_and_left(slot.get(which(slot), now), DRUMS.get(&DRUMS.during, now), map(p).value()).1
            }),
        )
        .width(Pixels(140.0))
        .child_top(Stretch(1.0))
        .child_bottom(Stretch(1.0));
    })
    .height(Pixels(32.0))
    .col_between(Pixels(8.0));
}

impl Plugin for Budget {
    const NAME: &'static str = "Budget";
    const VENDOR: &'static str = "OGII";
    const URL: &'static str = env!("CARGO_PKG_HOMEPAGE");
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        aux_input_ports: &[new_nonzero_u32(2)],
        names: PortNames { layout: None, main_input: None, main_output: None, aux_inputs: &["Clap"], aux_outputs: &[] },
        ..AudioIOLayout::const_default()
    }];

    const SAMPLE_ACCURATE_AUTOMATION: bool = false;
    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        let params = self.params.clone();
        let status = self.status.clone();
        create_vizia_editor(self.params.editor_state.clone(), ViziaTheming::Custom, move |cx, _| {
            assets::register_noto_sans_light(cx);
            assets::register_noto_sans_thin(cx);
            Data { params: params.clone(), status: status.clone() }.build(cx);
            VStack::new(cx, |cx| {
                Label::new(cx, "Budget")
                    .font_family(vec![FamilyOwned::Name(String::from(assets::NOTO_SANS))])
                    .font_weight(FontWeightKeyword::Thin)
                    .font_size(28.0)
                    .height(Pixels(44.0));
                Label::new(cx, "One copy last on the drums, one on the growls and one on the sub").height(Pixels(22.0));
                Label::new(cx, "(or one on the bus they share, after its clip).").height(Pixels(22.0));
                Label::new(cx, "Clap on the sidechain of all three.").height(Pixels(26.0));
                setting(cx, "This is", |p| &p.bus);
                setting(cx, "Trigger within", |p| &p.thr);
                setting(cx, "Level change after this", |p| &p.trim);
                setting(cx, "Then clipped at full scale", |p| &p.clip_after);
                Label::new(
                    cx,
                    Data::status.map(|s| {
                        let now = clock_ms();
                        let clap = match DRUMS.get(&DRUMS.during, now) {
                            Some(c) => format!("clap level {c:.1} dBFS"),
                            None => String::from("clap level --  (no Budget on the drums, or nothing playing)"),
                        };
                        format!("{clap}      claps seen by this copy {}", s.claps.load(Ordering::Relaxed))
                    }),
                )
                .height(Pixels(30.0))
                .top(Pixels(8.0));
                HStack::new(cx, |cx| {
                    Label::new(cx, "").width(Pixels(150.0));
                    Label::new(cx, "now").width(Pixels(150.0));
                    Label::new(cx, "budget (under by)").width(Pixels(130.0));
                    Label::new(cx, "left").width(Pixels(140.0));
                })
                .height(Pixels(24.0))
                .col_between(Pixels(8.0));
                Label::new(cx, "Growls").height(Pixels(24.0)).top(Pixels(4.0));
                line(cx, "right after the clap", &GROWLS, |s| &s.after, |p| &p.g_after);
                line(cx, "rest of the bar", &GROWLS, |s| &s.rest, |p| &p.g_rest);
                line(cx, "just before the clap", &GROWLS, |s| &s.before, |p| &p.g_before);
                Label::new(cx, "Sub").height(Pixels(24.0)).top(Pixels(4.0));
                line(cx, "under the clap", &SUB, |s| &s.during, |p| &p.s_under);
                line(cx, "rest of the bar", &SUB, |s| &s.rest, |p| &p.s_rest);
            })
            .child_left(Pixels(20.0))
            .child_top(Pixels(10.0));
        })
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, buffer_config: &BufferConfig, _context: &mut impl InitContext<Self>) -> bool {
        self.meter = Some(Meter::new(buffer_config.sample_rate, Role::Drums));
        self.lows = Some(Meter::new(buffer_config.sample_rate, Role::Lows));
        true
    }

    fn reset(&mut self) {
        if let Some(m) = &mut self.meter {
            m.reset();
        }
        if let Some(m) = &mut self.lows {
            m.reset();
        }
    }

    fn process(&mut self, buffer: &mut Buffer, aux: &mut AuxiliaryBuffers, _context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let Some(meter) = &mut self.meter else { return ProcessStatus::Normal };
        let len = buffer.samples();
        if len == 0 {
            return ProcessStatus::Normal;
        }
        let (role, slot) = match self.params.bus.value() {
            Bus::Drums => (Role::Drums, &DRUMS),
            Bus::Growls => (Role::Growls, &GROWLS),
            Bus::Sub => (Role::Sub, &SUB),
            Bus::Bed => (Role::Growls, &GROWLS),
        };
        if meter.role != role {
            meter.role = role;
            meter.reset();
        }
        meter.thr_db = self.params.thr.value();
        meter.gain_after = 10f32.powf(self.params.trim.value() / 20.0);
        meter.clip_after = self.params.clip_after.value();
        let main = buffer.as_slice_immutable();
        let right = if main.len() > 1 { 1 } else { 0 };
        let side = aux.inputs.first().map(|b| b.as_slice_immutable()).filter(|sc| !sc.is_empty() && sc[0].len() >= len);
        let sc: Option<[&[f32]; 2]> = side.map(|sc| [&sc[0][..len], &sc[if sc.len() > 1 { 1 } else { 0 }][..len]]);
        meter.process([&main[0][..len], &main[right][..len]], sc);
        slot.store(&meter.readings(), 0.0, clock_ms());
        if self.params.bus.value() == Bus::Bed {
            if let Some(lows) = &mut self.lows {
                lows.thr_db = meter.thr_db;
                lows.gain_after = meter.gain_after;
                lows.clip_after = meter.clip_after;
                lows.process([&main[0][..len], &main[right][..len]], sc);
                SUB.store(&lows.readings(), 0.0, clock_ms());
            }
        }
        self.status.claps.store(meter.triggers, Ordering::Relaxed);
        ProcessStatus::Normal
    }
}

impl ClapPlugin for Budget {
    const CLAP_ID: &'static str = "com.ogii.budget";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("How loud the growls and the sub may get before the clap loses room");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Analyzer, ClapFeature::Utility];
}

impl Vst3Plugin for Budget {
    const VST3_CLASS_ID: [u8; 16] = *b"OGIIBudgetMeter1";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Analyzer];
}

nih_export_clap!(Budget);
nih_export_vst3!(Budget);
