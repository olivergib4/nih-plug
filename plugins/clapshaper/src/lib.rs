//! Clap Shaper: shapes the clap itself after each hit. Put it last on the clap's bus, before the drum bus clip:
//! it lifts the centre of the body (150 Hz to 1.5 kHz) while the clap sounds and dips the tail afterwards.
//! It triggers from its own input, so no sidechain; 20 ms of latency for the phase-free body filter. See `clapdsp::space` (`ClapTail`) for the details.
use clapdsp::space::{Space, SpaceRole, SpaceSettings};
use nih_plug::prelude::*;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::widgets::*;
use nih_plug_vizia::{assets, create_vizia_editor, ViziaState, ViziaTheming};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::Arc;

#[derive(Params)]
pub struct ClapShaperParams {
    #[persist = "editor-state"]
    editor_state: Arc<ViziaState>,
    #[id = "on"]
    pub on: BoolParam,
    #[id = "thr"]
    pub thr: FloatParam,
    #[id = "body_db"]
    pub body_db: FloatParam,
    #[id = "body_side_db"]
    pub body_side_db: FloatParam,
    #[id = "body_from"]
    pub body_from: FloatParam,
    #[id = "body_to"]
    pub body_to: FloatParam,
    #[id = "lean_db"]
    pub lean_db: FloatParam,
    #[id = "tone_db"]
    pub tone_db: FloatParam,
    #[id = "centre_db"]
    pub centre_db: FloatParam,
    #[id = "push_db"]
    pub push_db: FloatParam,
    #[id = "push_from"]
    pub push_from: FloatParam,
    #[id = "push_to"]
    pub push_to: FloatParam,
    #[id = "tail_db"]
    pub tail_db: FloatParam,
    #[id = "tail_from"]
    pub tail_from: FloatParam,
    #[id = "tail_to"]
    pub tail_to: FloatParam,
    #[id = "split"]
    pub split: FloatParam,
}

fn db(name: &str, default: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min, max })
        .with_unit(" dB")
        .with_step_size(0.1)
        .with_value_to_string(formatters::v2s_f32_rounded(1))
}

fn ms(name: &str, default: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Skewed { min, max, factor: FloatRange::skew_factor(-1.5) })
        .with_unit(" ms")
        .with_step_size(0.1)
        .with_value_to_string(formatters::v2s_f32_rounded(1))
}

impl Default for ClapShaperParams {
    fn default() -> Self {
        ClapShaperParams {
            editor_state: ViziaState::new(|| (380, 826)),
            on: BoolParam::new("On", true),
            thr: db("Trigger within", 12.0, 3.0, 40.0),
            body_db: db("Body lift", 8.0, 0.0, 12.0),
            body_side_db: db("Body sides lift", 8.0, 0.0, 18.0),
            body_from: ms("Body from", 20.0, 5.0, 100.0),
            body_to: ms("Body to", 95.0, 20.0, 300.0),
            lean_db: db("Lean", 0.0, 0.0, 6.0),
            tone_db: db("Left/right tone split", 0.0, -6.0, 6.0),
            centre_db: db("Tail to centre", 4.0, 0.0, 9.0),
            push_db: db("Push", 0.0, 0.0, 12.0),
            push_from: ms("Push from", 23.0, 5.0, 100.0),
            push_to: ms("Push to", 63.0, 20.0, 300.0),
            tail_db: db("Tail dip", 7.0, -18.0, 24.0),
            tail_from: ms("Tail from", 150.0, 40.0, 500.0),
            tail_to: ms("Tail to", 250.0, 60.0, 800.0),
            split: FloatParam::new("Tail above", 150.0, FloatRange::Skewed { min: 60.0, max: 1000.0, factor: FloatRange::skew_factor(-2.0) })
                .with_unit(" Hz")
                .with_step_size(1.0)
                .with_value_to_string(formatters::v2s_f32_rounded(0)),
        }
    }
}

impl ClapShaperParams {
    pub fn settings(&self) -> SpaceSettings {
        SpaceSettings {
            role: if self.on.value() { SpaceRole::ClapTail } else { SpaceRole::Off },
            thr_db: self.thr.value(),
            split_hz: self.split.value(),
            tail_db: self.tail_db.value(),
            tail_from_ms: self.tail_from.value(),
            tail_to_ms: self.tail_to.value(),
            body_db: self.body_db.value(),
            body_side_db: self.body_side_db.value(),
            body_from_ms: self.body_from.value(),
            body_to_ms: self.body_to.value(),
            lean_db: self.lean_db.value(),
            tone_split_db: self.tone_db.value(),
            centre_db: self.centre_db.value(),
            push_db: self.push_db.value(),
            push_from_ms: self.push_from.value(),
            push_to_ms: self.push_to.value(),
            ..SpaceSettings::default()
        }
    }
}

/// What the editor shows about the audio thread.
#[derive(Default)]
pub struct Status {
    pub claps: AtomicU32,
    /// Lowest gain of the last block, in tenths of a dB.
    pub gain_tenths: AtomicI32,
}

pub struct ClapShaper {
    params: Arc<ClapShaperParams>,
    status: Arc<Status>,
    space: Option<Space>,
    latency: u32,
}

impl Default for ClapShaper {
    fn default() -> Self {
        ClapShaper { params: Arc::new(ClapShaperParams::default()), status: Arc::new(Status::default()), space: None, latency: 0 }
    }
}

#[derive(Lens)]
struct Data {
    params: Arc<ClapShaperParams>,
    status: Arc<Status>,
}

impl Model for Data {}

fn row<P, FMap>(cx: &mut Context, label: &str, map: FMap)
where
    P: Param + 'static,
    FMap: Fn(&Arc<ClapShaperParams>) -> &P + Copy + 'static,
{
    HStack::new(cx, |cx| {
        Label::new(cx, label).width(Pixels(130.0)).child_top(Stretch(1.0)).child_bottom(Stretch(1.0));
        ParamSlider::new(cx, Data::params, map).width(Pixels(200.0));
    })
    .height(Pixels(32.0))
    .col_between(Pixels(8.0));
}

impl Plugin for ClapShaper {
    const NAME: &'static str = "Clap Shaper";
    const VENDOR: &'static str = "OGII";
    const URL: &'static str = env!("CARGO_PKG_HOMEPAGE");
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
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
                Label::new(cx, "Clap Shaper")
                    .font_family(vec![FamilyOwned::Name(String::from(assets::NOTO_SANS))])
                    .font_weight(FontWeightKeyword::Thin)
                    .font_size(28.0)
                    .height(Pixels(44.0));
                Label::new(cx, "Last on the clap's bus, before the drum clip.").height(Pixels(26.0));
                row(cx, "On", |p| &p.on);
                row(cx, "Trigger within", |p| &p.thr);
                Label::new(cx, "Body (150 Hz to 1.5 kHz)").height(Pixels(26.0)).top(Pixels(8.0));
                row(cx, "Lift centre by", |p| &p.body_db);
                row(cx, "Lift sides by", |p| &p.body_side_db);
                row(cx, "From", |p| &p.body_from);
                row(cx, "To", |p| &p.body_to);
                Label::new(cx, "Push (everything, into the clip)").height(Pixels(26.0)).top(Pixels(8.0));
                row(cx, "Push by", |p| &p.push_db);
                row(cx, "From", |p| &p.push_from);
                row(cx, "To", |p| &p.push_to);
                Label::new(cx, "Lean (starts left, ends right)").height(Pixels(26.0)).top(Pixels(8.0));
                row(cx, "Lean by", |p| &p.lean_db);
                Label::new(cx, "Left/right tone (clap body only)").height(Pixels(26.0)).top(Pixels(8.0));
                row(cx, "Left mids, right highs", |p| &p.tone_db);
                Label::new(cx, "Tail").height(Pixels(26.0)).top(Pixels(8.0));
                row(cx, "Dip by (minus lifts)", |p| &p.tail_db);
                row(cx, "From", |p| &p.tail_from);
                row(cx, "To", |p| &p.tail_to);
                row(cx, "Above", |p| &p.split);
                row(cx, "To centre by", |p| &p.centre_db);
                Label::new(
                    cx,
                    Data::status.map(|s| {
                        format!("claps seen {}    gain now {:+.1} dB", s.claps.load(Ordering::Relaxed), s.gain_tenths.load(Ordering::Relaxed) as f32 / 10.0)
                    }),
                )
                .height(Pixels(30.0))
                .top(Pixels(8.0));
            })
            .child_left(Pixels(20.0))
            .child_top(Pixels(10.0));
        })
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, buffer_config: &BufferConfig, context: &mut impl InitContext<Self>) -> bool {
        let mut space = Space::new(buffer_config.sample_rate);
        let s = self.params.settings();
        space.update(&s);
        self.latency = space.latency(s.role);
        context.set_latency_samples(self.latency);
        self.space = Some(space);
        true
    }

    fn reset(&mut self) {
        if let Some(s) = &mut self.space {
            s.reset();
        }
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let Some(space) = &mut self.space else { return ProcessStatus::Normal };
        let len = buffer.samples();
        if len == 0 {
            return ProcessStatus::Normal;
        }
        let s = self.params.settings();
        space.update(&s);
        let latency = space.latency(s.role);
        if latency != self.latency {
            self.latency = latency;
            context.set_latency_samples(latency);
        }
        space.process(buffer.as_slice(), None, len);
        self.status.claps.store(space.triggers, Ordering::Relaxed);
        self.status.gain_tenths.store((space.gain_db * 10.0).round() as i32, Ordering::Relaxed);
        ProcessStatus::Normal
    }
}

impl ClapPlugin for ClapShaper {
    const CLAP_ID: &'static str = "com.ogii.clap-shaper";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Lifts the body of the clap and dips its tail");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Utility];
}

impl Vst3Plugin for ClapShaper {
    const VST3_CLASS_ID: [u8; 16] = *b"OGIIClapShaper01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Dynamics];
}

nih_export_clap!(ClapShaper);
nih_export_vst3!(ClapShaper);
