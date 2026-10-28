// OnsetGuard: onset-protected zero-phase spectral dynamics with spectral clipping.
// Derived from Spectral Compressor, Copyright (C) 2021-2024 Robbert van der Helm.
// OnsetGuard additions Copyright (C) 2026 OGII.
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use atomic_float::AtomicF32;
use nih_plug::prelude::*;
use nih_plug_vizia::ViziaState;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use triple_buffer::TripleBuffer;

pub mod analyzer;
pub mod dsp;
mod editor;
pub mod help;
pub mod presets;

use dsp::dynamics::Weighting;
use dsp::{Engine, Settings, SubMode};

pub struct OnsetGuard {
    params: Arc<OnsetGuardParams>,
    engine: Option<Engine>,
    meters: Arc<Meters>,
    analyzer_in: triple_buffer::Input<analyzer::AnalyzerData>,
    analyzer_out: Arc<Mutex<triple_buffer::Output<analyzer::AnalyzerData>>>,
    sample_rate: f32,
    max_block: usize,
    num_channels: usize,
}

/// Values shared with the editor for metering.
pub struct Meters {
    /// Gain reduction of the spectral (per-bin ceiling) stage, dB <= 0.
    pub gr_spec_db: AtomicF32,
    /// Gain reduction of the post soft clipper, dB <= 0.
    pub gr_post_db: AtomicF32,
    /// K-weighted level of the dry and processed signals (LUFS-style, 3 s window).
    pub dry_lufs: AtomicF32,
    pub wet_lufs: AtomicF32,
    /// Perceived loudness (sones) of the dry signal at unity and of the processed signal.
    pub dry_sone: AtomicF32,
    pub wet_sone: AtomicF32,
    /// Current matched-bypass gain, dB.
    pub match_db: AtomicF32,
}

impl Default for Meters {
    fn default() -> Self {
        Meters {
            gr_spec_db: AtomicF32::new(0.0),
            gr_post_db: AtomicF32::new(0.0),
            dry_lufs: AtomicF32::new(-100.0),
            wet_lufs: AtomicF32::new(-100.0),
            dry_sone: AtomicF32::new(0.0),
            wet_sone: AtomicF32::new(0.0),
            match_db: AtomicF32::new(0.0),
        }
    }
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum DetectorWeighting {
    #[id = "off"]
    #[name = "Flat"]
    Off,
    #[id = "k"]
    #[name = "K-weighted"]
    K,
    #[id = "equal_loudness"]
    #[name = "Equal loudness"]
    EqualLoudness,
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum SubSplit {
    #[id = "off"]
    #[name = "Off"]
    Off,
    #[id = "protect"]
    #[name = "Protect (clean sub)"]
    Protect,
    #[id = "separate"]
    #[name = "Clip separately"]
    Separate,
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum Oversample {
    #[id = "1x"]
    #[name = "Off"]
    X1,
    #[id = "2x"]
    #[name = "2x"]
    X2,
    #[id = "4x"]
    #[name = "4x"]
    X4,
    #[id = "8x"]
    #[name = "8x"]
    X8,
}

#[derive(Params)]
pub struct OnsetGuardParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<ViziaState>,

    #[nested(group = "Global")]
    pub global: Arc<GlobalParams>,
    #[nested(group = "Resolution")]
    pub resolution: Arc<ResolutionParams>,
    #[nested(group = "Threshold")]
    pub threshold: Arc<ThresholdParams>,
    #[nested(group = "Dynamics")]
    pub dynamics: Arc<DynamicsParams>,
    #[nested(group = "Onset Protection")]
    pub protect: Arc<ProtectParams>,
    #[nested(group = "Clip")]
    pub clip: Arc<ClipParams>,
}

#[derive(Params)]
pub struct GlobalParams {
    #[id = "output"]
    pub output_gain: FloatParam,
    #[id = "mix"]
    pub mix: FloatParam,
    #[id = "matched_bypass"]
    pub matched_bypass: BoolParam,
    #[id = "ref_level"]
    pub ref_spl: FloatParam,
}

#[derive(Params)]
pub struct ResolutionParams {
    #[id = "long_window"]
    pub long_order: IntParam,
    #[id = "short_window"]
    pub short_order: IntParam,
    #[id = "crossover"]
    pub crossover_hz: FloatParam,
    #[id = "mono_below"]
    pub mono_below_hz: FloatParam,
}

#[derive(Params)]
pub struct ThresholdParams {
    #[id = "weighting"]
    pub weighting: EnumParam<DetectorWeighting>,
    #[id = "threshold"]
    pub threshold_db: FloatParam,
    #[id = "center"]
    pub center_hz: FloatParam,
    #[id = "slope"]
    pub slope_db_oct: FloatParam,
    #[id = "curve"]
    pub curve_db_oct2: FloatParam,
}

#[derive(Params)]
pub struct DynamicsParams {
    #[id = "down_ratio"]
    pub down_ratio: FloatParam,
    #[id = "up_ratio"]
    pub up_ratio: FloatParam,
    #[id = "up_range"]
    pub up_range_db: FloatParam,
    #[id = "attack"]
    pub attack_ms: FloatParam,
    #[id = "release"]
    pub release_ms: FloatParam,
    #[id = "smooth"]
    pub smooth_oct: FloatParam,
    #[id = "stereo_link"]
    pub stereo_link: FloatParam,
}

#[derive(Params)]
pub struct ProtectParams {
    #[id = "protect"]
    pub protect_on: BoolParam,
    #[id = "sens"]
    pub onset_sens: FloatParam,
    #[id = "pre"]
    pub pre_ms: FloatParam,
    #[id = "hold"]
    pub hold_ms: FloatParam,
    #[id = "post"]
    pub post_ms: FloatParam,
    #[id = "trans_boost"]
    pub trans_boost_db: FloatParam,
    #[id = "trans_len"]
    pub trans_boost_ms: FloatParam,
}

#[derive(Params)]
pub struct ClipParams {
    #[id = "clip"]
    pub clip_on: BoolParam,
    #[id = "drive"]
    pub drive_db: FloatParam,
    #[id = "bin_ceil"]
    pub bin_ceil_db: FloatParam,
    #[id = "ceiling"]
    pub ceil_db: FloatParam,
    #[id = "hardness"]
    pub hardness: FloatParam,
    #[id = "trans_clip"]
    pub trans_clip: FloatParam,
    #[id = "clip_link"]
    pub clip_link: FloatParam,
    #[id = "oversample"]
    pub oversample: EnumParam<Oversample>,
    #[id = "hf_preserve"]
    pub hf_preserve: BoolParam,
    #[id = "hf_hz"]
    pub hf_hz: FloatParam,
    #[id = "sub_split"]
    pub sub_split: EnumParam<SubSplit>,
    #[id = "sub_hz"]
    pub sub_hz: FloatParam,
    #[id = "sub_ceil"]
    pub sub_ceil_db: FloatParam,
    #[id = "band_protect"]
    pub band_protect: FloatParam,
}

fn db_param(name: &str, default: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min, max })
        .with_unit(" dB")
        .with_step_size(0.1)
}

fn ms_param(name: &str, default: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(
        name,
        default,
        FloatRange::Skewed {
            min,
            max,
            factor: FloatRange::skew_factor(-1.5),
        },
    )
    .with_unit(" ms")
    .with_step_size(0.1)
}

fn pct_param(name: &str, default: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min: 0.0, max: 1.0 })
        .with_unit("%")
        .with_value_to_string(formatters::v2s_f32_percentage(0))
        .with_string_to_value(formatters::s2v_f32_percentage())
}

impl Default for OnsetGuardParams {
    fn default() -> Self {
        let d = Settings::default();
        OnsetGuardParams {
            editor_state: editor::default_state(),
            global: Arc::new(GlobalParams {
            output_gain: FloatParam::new(
                "Output Gain",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-30.0),
                    max: util::db_to_gain(30.0),
                    factor: FloatRange::gain_skew_factor(-30.0, 30.0),
                },
            )
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
            mix: pct_param("Mix", 1.0),
            matched_bypass: BoolParam::new("Matched Bypass", false),
            ref_spl: FloatParam::new(
                "Ref Level (0 dBFS)",
                d.ref_spl,
                FloatRange::Linear { min: 70.0, max: 115.0 },
            )
            .with_unit(" dB SPL")
            .with_step_size(1.0),
            }),
            resolution: Arc::new(ResolutionParams {
            long_order: IntParam::new(
                "Long Window (lows)",
                d.long_order as i32,
                IntRange::Linear {
                    min: dsp::MIN_LONG_ORDER as i32,
                    max: dsp::MAX_LONG_ORDER as i32,
                },
            )
            .with_value_to_string(formatters::v2s_i32_power_of_two())
            .with_string_to_value(formatters::s2v_i32_power_of_two()),
            short_order: IntParam::new(
                "Short Window (highs)",
                d.short_order as i32,
                IntRange::Linear {
                    min: dsp::MIN_SHORT_ORDER as i32,
                    max: dsp::MAX_SHORT_ORDER as i32,
                },
            )
            .with_value_to_string(formatters::v2s_i32_power_of_two())
            .with_string_to_value(formatters::s2v_i32_power_of_two()),
            crossover_hz: FloatParam::new(
                "Crossover",
                d.dyn_.crossover_hz,
                FloatRange::Skewed {
                    min: 60.0,
                    max: 4000.0,
                    factor: FloatRange::skew_factor(-1.5),
                },
            )
            .with_value_to_string(formatters::v2s_f32_hz_then_khz(0))
            .with_string_to_value(formatters::s2v_f32_hz_then_khz()),
            mono_below_hz: FloatParam::new(
                "Mono Below",
                d.dyn_.mono_below_hz,
                FloatRange::Linear { min: 0.0, max: 300.0 },
            )
            .with_unit(" Hz")
            .with_step_size(1.0),
            }),
            threshold: Arc::new(ThresholdParams {
            weighting: EnumParam::new("Detector Weighting", DetectorWeighting::Off),
            threshold_db: db_param("Threshold", d.dyn_.threshold_db, -80.0, 0.0),
            center_hz: FloatParam::new(
                "Threshold Center",
                d.dyn_.center_hz,
                FloatRange::Skewed {
                    min: 20.0,
                    max: 20_000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_value_to_string(formatters::v2s_f32_hz_then_khz(0))
            .with_string_to_value(formatters::s2v_f32_hz_then_khz()),
            slope_db_oct: FloatParam::new(
                "Threshold Slope",
                d.dyn_.slope_db_oct,
                FloatRange::Linear { min: -12.0, max: 6.0 },
            )
            .with_unit(" dB/oct")
            .with_step_size(0.1),
            curve_db_oct2: FloatParam::new(
                "Threshold Curve",
                d.dyn_.curve_db_oct2,
                FloatRange::Linear { min: -6.0, max: 6.0 },
            )
            .with_unit(" dB/oct²")
            .with_step_size(0.1),
            }),
            dynamics: Arc::new(DynamicsParams {
            down_ratio: FloatParam::new(
                "Downward Ratio",
                d.dyn_.down_ratio,
                FloatRange::Skewed {
                    min: 1.0,
                    max: 50.0,
                    factor: FloatRange::skew_factor(-1.5),
                },
            )
            .with_step_size(0.1)
            .with_value_to_string(formatters::v2s_compression_ratio(1))
            .with_string_to_value(formatters::s2v_compression_ratio()),
            up_ratio: FloatParam::new(
                "Upward Ratio",
                d.dyn_.up_ratio,
                FloatRange::Skewed {
                    min: 1.0,
                    max: 20.0,
                    factor: FloatRange::skew_factor(-1.5),
                },
            )
            .with_step_size(0.1)
            .with_value_to_string(formatters::v2s_compression_ratio(1))
            .with_string_to_value(formatters::s2v_compression_ratio()),
            up_range_db: db_param("Upward Range", d.dyn_.up_range_db, 0.0, 40.0),
            attack_ms: ms_param("Attack", d.dyn_.attack_ms, 0.1, 500.0),
            release_ms: ms_param("Release", d.dyn_.release_ms, 5.0, 3000.0),
            smooth_oct: FloatParam::new(
                "Frequency Smoothing",
                d.dyn_.smooth_oct,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_unit(" oct")
            .with_step_size(0.01),
            stereo_link: pct_param("Stereo Link", d.dyn_.stereo_link),
            }),
            protect: Arc::new(ProtectParams {
            protect_on: BoolParam::new("Onset Protection", d.protect_on),
            onset_sens: pct_param("Onset Sensitivity", d.onset_sens),
            pre_ms: ms_param("Protect Pre", d.pre_ms, 0.0, dsp::MAX_PRE_MS),
            hold_ms: ms_param("Protect Hold", d.hold_ms, 0.0, 30.0),
            post_ms: ms_param("Protect Post", d.post_ms, 0.0, 120.0),
            trans_boost_db: db_param("Transient Boost", d.trans_boost_db, 0.0, 12.0),
            trans_boost_ms: ms_param("Boost Length", d.trans_boost_ms, 2.0, 60.0),
            }),
            clip: Arc::new(ClipParams {
            clip_on: BoolParam::new("Clip", d.clip_on),
            drive_db: db_param("Clip Drive", d.drive_db, 0.0, 24.0),
            bin_ceil_db: db_param("Spectral Ceiling", d.bin_ceil_db, -40.0, 0.0),
            ceil_db: db_param("Output Ceiling", d.ceil_db, -12.0, 0.0),
            hardness: pct_param("Clip Hardness", d.hardness),
            trans_clip: pct_param("Transient Clip", d.trans_clip),
            clip_link: pct_param("Clip Stereo Link", d.clip_link),
            oversample: EnumParam::new("Oversample", Oversample::X4),
            hf_preserve: BoolParam::new("Preserve Highs", d.hf_preserve),
            hf_hz: FloatParam::new(
                "Preserve Above",
                d.hf_hz,
                FloatRange::Skewed {
                    min: 2000.0,
                    max: 16000.0,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_value_to_string(formatters::v2s_f32_hz_then_khz(1))
            .with_string_to_value(formatters::s2v_f32_hz_then_khz()),
            sub_split: EnumParam::new("Sub Split", SubSplit::Off),
            sub_hz: FloatParam::new(
                "Sub Split Freq",
                d.sub_hz,
                FloatRange::Skewed {
                    min: 30.0,
                    max: 250.0,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_unit(" Hz")
            .with_step_size(1.0),
            sub_ceil_db: db_param("Sub Ceiling", d.sub_ceil_db, -24.0, 0.0),
            band_protect: pct_param("Protect Amount", d.band_protect),
            }),
        }
    }
}

impl OnsetGuardParams {
    fn settings(&self) -> Settings {
        let mut s = Settings::default();
        s.dyn_.weighting = match self.threshold.weighting.value() {
            DetectorWeighting::Off => Weighting::Off,
            DetectorWeighting::K => Weighting::K,
            DetectorWeighting::EqualLoudness => Weighting::EqualLoudness,
        };
        s.dyn_.threshold_db = self.threshold.threshold_db.value();
        s.dyn_.center_hz = self.threshold.center_hz.value();
        s.dyn_.slope_db_oct = self.threshold.slope_db_oct.value();
        s.dyn_.curve_db_oct2 = self.threshold.curve_db_oct2.value();
        s.dyn_.down_ratio = self.dynamics.down_ratio.value();
        s.dyn_.up_ratio = self.dynamics.up_ratio.value();
        s.dyn_.up_range_db = self.dynamics.up_range_db.value();
        s.dyn_.attack_ms = self.dynamics.attack_ms.value();
        s.dyn_.release_ms = self.dynamics.release_ms.value();
        s.dyn_.smooth_oct = self.dynamics.smooth_oct.value();
        s.dyn_.stereo_link = self.dynamics.stereo_link.value();
        s.dyn_.crossover_hz = self.resolution.crossover_hz.value();
        s.dyn_.mono_below_hz = self.resolution.mono_below_hz.value();
        s.long_order = self.resolution.long_order.value() as usize;
        s.short_order = self.resolution.short_order.value() as usize;
        s.protect_on = self.protect.protect_on.value();
        s.onset_sens = self.protect.onset_sens.value();
        s.pre_ms = self.protect.pre_ms.value();
        s.hold_ms = self.protect.hold_ms.value();
        s.post_ms = self.protect.post_ms.value();
        s.trans_boost_db = self.protect.trans_boost_db.value();
        s.trans_boost_ms = self.protect.trans_boost_ms.value();
        s.clip_on = self.clip.clip_on.value();
        s.drive_db = self.clip.drive_db.value();
        s.bin_ceil_db = self.clip.bin_ceil_db.value();
        s.ceil_db = self.clip.ceil_db.value();
        s.hardness = self.clip.hardness.value();
        s.trans_clip = self.clip.trans_clip.value();
        s.clip_link = self.clip.clip_link.value();
        s.sub_mode = match self.clip.sub_split.value() {
            SubSplit::Off => SubMode::Off,
            SubSplit::Protect => SubMode::Protect,
            SubSplit::Separate => SubMode::Separate,
        };
        s.sub_hz = self.clip.sub_hz.value();
        s.hf_preserve = self.clip.hf_preserve.value();
        s.hf_hz = self.clip.hf_hz.value();
        s.sub_ceil_db = self.clip.sub_ceil_db.value();
        s.band_protect = self.clip.band_protect.value();
        s.oversample = match self.clip.oversample.value() {
            Oversample::X1 => 1,
            Oversample::X2 => 2,
            Oversample::X4 => 4,
            Oversample::X8 => 8,
        };
        s.mix = self.global.mix.value();
        s.output_gain = self.global.output_gain.value();
        s.matched_bypass = self.global.matched_bypass.value();
        s.ref_spl = self.global.ref_spl.value();
        s
    }
}

impl Default for OnsetGuard {
    fn default() -> Self {
        let (analyzer_in, analyzer_out) = TripleBuffer::default().split();
        OnsetGuard {
            params: Arc::new(OnsetGuardParams::default()),
            engine: None,
            meters: Arc::new(Meters::default()),
            analyzer_in,
            analyzer_out: Arc::new(Mutex::new(analyzer_out)),
            sample_rate: 48000.0,
            max_block: 0,
            num_channels: 2,
        }
    }
}

impl Plugin for OnsetGuard {
    const NAME: &'static str = "OnsetGuard";
    const VENDOR: &'static str = "OGII";
    const URL: &'static str = env!("CARGO_PKG_HOMEPAGE");
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            ..AudioIOLayout::const_default()
        },
    ];

    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(
            self.params.clone(),
            self.meters.clone(),
            self.analyzer_out.clone(),
            self.params.editor_state.clone(),
        )
    }

    fn initialize(
        &mut self,
        audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        self.max_block = buffer_config.max_buffer_size as usize;
        self.num_channels = audio_io_layout
            .main_output_channels
            .map(|c| c.get() as usize)
            .unwrap_or(2);
        let mut engine = Engine::new(self.num_channels, self.sample_rate, self.max_block.max(1));
        let s = self.params.settings();
        engine.set_windows(s.long_order, s.short_order, s.oversample);
        context.set_latency_samples(engine.latency() as u32);
        self.engine = Some(engine);
        true
    }

    fn reset(&mut self) {
        if let Some(e) = &mut self.engine {
            e.reset();
        }
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let Some(engine) = &mut self.engine else {
            return ProcessStatus::Normal;
        };
        let s = self.params.settings();
        let before = engine.latency();
        let len = buffer.samples();
        if len == 0 || buffer.channels() == 0 {
            return ProcessStatus::Normal;
        }
        engine.process(buffer.as_slice(), len, &s);
        if engine.latency() != before {
            context.set_latency_samples(engine.latency() as u32);
        }
        if self.params.editor_state.is_open() {
            let (gr_spec, gr_post) = engine.gain_reduction();
            let (match_gain, dry, wet) = engine.meters();
            let (dry_sone, wet_sone) = engine.sones();
            self.meters.dry_sone.store(dry_sone, Ordering::Relaxed);
            self.meters.wet_sone.store(wet_sone, Ordering::Relaxed);
            self.meters.match_db.store(20.0 * match_gain.max(1e-4).log10(), Ordering::Relaxed);
            self.meters.gr_spec_db.store(gr_spec, Ordering::Relaxed);
            self.meters.gr_post_db.store(gr_post, Ordering::Relaxed);
            self.meters.dry_lufs.store(dry, Ordering::Relaxed);
            self.meters.wet_lufs.store(wet, Ordering::Relaxed);
            let a = self.analyzer_in.input_buffer();
            engine.analyzer(analyzer::F_LO, analyzer::F_HI, &mut a.env_db, &mut a.gain_db, &mut a.thr_db);
            a.protect = engine.protect_activity();
            self.analyzer_in.publish();
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for OnsetGuard {
    const CLAP_ID: &'static str = "com.ogii.onsetguard";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Onset-protected zero-phase spectral dynamics with spectral clipping");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Mono,
        ClapFeature::Compressor,
        ClapFeature::Custom("nih:spectral"),
    ];
}

impl Vst3Plugin for OnsetGuard {
    const VST3_CLASS_ID: [u8; 16] = *b"OGIIOnsetGuard01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[
        Vst3SubCategory::Fx,
        Vst3SubCategory::Dynamics,
        Vst3SubCategory::Custom("Spectral"),
    ];
}

nih_export_clap!(OnsetGuard);
nih_export_vst3!(OnsetGuard);
