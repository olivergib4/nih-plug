//! Clap Space: makes room for the clap on another track. Put it on the growls or on the sub and feed the clap
//! into its sidechain. See `clapdsp::space` for what the two jobs do and where the default values come from.
use clapdsp::space::{Space, SpaceRole, SpaceSettings};
use nih_plug::prelude::*;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::widgets::*;
use nih_plug_vizia::{assets, create_vizia_editor, ViziaState, ViziaTheming};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::Arc;

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum Job {
    /// On the growls: everything above the split goes down from the clap until "Growls back at".
    #[name = "Growls: duck"]
    Growls,
    /// On the sub: drops out at the clap, down until "Sub down until", full at "Sub full at".
    #[name = "Sub: dip"]
    Sub,
}

#[derive(Params)]
pub struct ClapSpaceParams {
    #[persist = "editor-state"]
    editor_state: Arc<ViziaState>,
    #[id = "on"]
    pub on: BoolParam,
    #[id = "job"]
    pub job: EnumParam<Job>,
    #[id = "thr"]
    pub thr: FloatParam,
    #[id = "before"]
    pub before: FloatParam,
    #[id = "depth"]
    pub depth: FloatParam,
    #[id = "back"]
    pub back: FloatParam,
    #[id = "split"]
    pub split: FloatParam,
    #[id = "sub_hold"]
    pub sub_hold: FloatParam,
    #[id = "sub_back"]
    pub sub_back: FloatParam,
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

impl Default for ClapSpaceParams {
    fn default() -> Self {
        ClapSpaceParams {
            editor_state: ViziaState::new(|| (380, 502)),
            on: BoolParam::new("On", true),
            job: EnumParam::new("Job", Job::Growls),
            thr: db("Trigger within", 12.0, 3.0, 40.0),
            before: FloatParam::new("Down before clap", 40.0, FloatRange::Linear { min: 0.0, max: 58.0 })
                .with_unit(" ms")
                .with_step_size(1.0)
                .with_value_to_string(formatters::v2s_f32_rounded(0)),
            depth: db("Growl duck", 24.0, 0.0, 48.0),
            back: ms("Growls back at", 250.0, 50.0, 600.0),
            split: FloatParam::new("Growl split", 150.0, FloatRange::Skewed { min: 60.0, max: 1000.0, factor: FloatRange::skew_factor(-2.0) })
                .with_unit(" Hz")
                .with_step_size(1.0)
                .with_value_to_string(formatters::v2s_f32_rounded(0)),
            sub_hold: ms("Sub down until", 55.0, 6.0, 300.0),
            sub_back: ms("Sub full at", 130.0, 10.0, 600.0),
        }
    }
}

impl ClapSpaceParams {
    pub fn settings(&self) -> SpaceSettings {
        SpaceSettings {
            role: if !self.on.value() {
                SpaceRole::Off
            } else {
                match self.job.value() {
                    Job::Growls => SpaceRole::Growls,
                    Job::Sub => SpaceRole::Sub,
                }
            },
            thr_db: self.thr.value(),
            split_hz: self.split.value(),
            before_ms: self.before.value(),
            depth_db: self.depth.value(),
            back_ms: self.back.value(),
            sub_hold_ms: self.sub_hold.value(),
            sub_back_ms: self.sub_back.value(),
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

pub struct ClapSpace {
    params: Arc<ClapSpaceParams>,
    status: Arc<Status>,
    space: Option<Space>,
    latency: u32,
}

impl Default for ClapSpace {
    fn default() -> Self {
        ClapSpace { params: Arc::new(ClapSpaceParams::default()), status: Arc::new(Status::default()), space: None, latency: 0 }
    }
}

#[derive(Lens)]
struct Data {
    params: Arc<ClapSpaceParams>,
    status: Arc<Status>,
}

impl Model for Data {}

fn row<P, FMap>(cx: &mut Context, label: &str, map: FMap)
where
    P: Param + 'static,
    FMap: Fn(&Arc<ClapSpaceParams>) -> &P + Copy + 'static,
{
    HStack::new(cx, |cx| {
        Label::new(cx, label).width(Pixels(130.0)).child_top(Stretch(1.0)).child_bottom(Stretch(1.0));
        ParamSlider::new(cx, Data::params, map).width(Pixels(200.0));
    })
    .height(Pixels(32.0))
    .col_between(Pixels(8.0));
}

impl Plugin for ClapSpace {
    const NAME: &'static str = "Clap Space";
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
                Label::new(cx, "Clap Space")
                    .font_family(vec![FamilyOwned::Name(String::from(assets::NOTO_SANS))])
                    .font_weight(FontWeightKeyword::Thin)
                    .font_size(28.0)
                    .height(Pixels(44.0));
                Label::new(cx, "Put the clap on the sidechain.").height(Pixels(26.0));
                row(cx, "On", |p| &p.on);
                row(cx, "Job", |p| &p.job);
                row(cx, "Trigger within", |p| &p.thr);
                row(cx, "Down before clap", |p| &p.before);
                Label::new(cx, "Growls").height(Pixels(26.0)).top(Pixels(8.0));
                row(cx, "Duck by", |p| &p.depth);
                row(cx, "Back at", |p| &p.back);
                row(cx, "Above", |p| &p.split);
                Label::new(cx, "Sub").height(Pixels(26.0)).top(Pixels(8.0));
                row(cx, "Down until", |p| &p.sub_hold);
                row(cx, "Full at", |p| &p.sub_back);
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

    fn process(&mut self, buffer: &mut Buffer, aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
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
        let side = aux.inputs.first().map(|b| b.as_slice_immutable()).filter(|sc| !sc.is_empty() && sc[0].len() >= len);
        let sc: Option<[&[f32]; 2]> = side.map(|sc| [&sc[0][..len], &sc[if sc.len() > 1 { 1 } else { 0 }][..len]]);
        space.process(buffer.as_slice(), sc, len);
        self.status.claps.store(space.triggers, Ordering::Relaxed);
        self.status.gain_tenths.store((space.gain_db * 10.0).round() as i32, Ordering::Relaxed);
        ProcessStatus::Normal
    }
}

impl ClapPlugin for ClapSpace {
    const CLAP_ID: &'static str = "com.ogii.clap-space";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Makes room for the clap on the growls or the sub");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Utility];
}

impl Vst3Plugin for ClapSpace {
    const VST3_CLASS_ID: [u8; 16] = *b"OGIIClapSpace001";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Dynamics];
}

nih_export_clap!(ClapSpace);
nih_export_vst3!(ClapSpace);
