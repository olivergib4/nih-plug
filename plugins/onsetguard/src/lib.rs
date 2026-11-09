// Bloater 1.0: bus clipper with hit analysis and reference comparison.
// Copyright (C) 2026 OGII. Derived from the nih-plug examples (Robbert van der Helm).
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

//! The plugin is a fixed 1x hard clipper at 0 dBFS with a pre-stage (drive, tilt/bell EQ, band
//! leveler, optional sub split) and an analyser that measures the clipped output the way the
//! reference tracks were measured: clap level and rail profile over the first 200 ms of each hit,
//! drums-vs-rest per band, 500-6 kHz spread, pinned share inside/between hits, phon. A reference
//! file dropped on the editor is analysed with the same code and shown as deltas.

use nih_plug::prelude::*;
use nih_plug_vizia::ViziaState;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use triple_buffer::TripleBuffer;

pub mod analysis;
pub mod dsp;
mod editor;
pub mod reference;

use analysis::{Analyzer, HitKind, HitProfile, Report};
use dsp::busclip::{BusClip, BusSettings};
use dsp::onset::OnsetDetector;
use dsp::spectrum::{Spectrum, NPTS};

/// Snapshot of the live analysis for the editor.
#[derive(Clone, Copy)]
pub struct Snapshot {
    pub report: Report,
    pub last: HitProfile,
    pub has_last: bool,
    /// Pre-clip peak of the last block, dBFS (how hard the clipper is being hit).
    pub in_peak_db: f32,
    /// Share of output samples at the rail in the last block, 0..1.
    pub block_pinned: f32,
    pub lev_gr_db: f32,
    pub hits: u32,
    /// Live spectrum (fast ballistics) and its average since play, dBFS per 1/6-octave point.
    pub spec_fast: [f32; NPTS],
    pub spec_avg: [f32; NPTS],
}

impl Default for Snapshot {
    fn default() -> Self {
        Snapshot {
            report: Report::default(),
            last: HitProfile::default(),
            has_last: false,
            in_peak_db: -120.0,
            block_pinned: 0.0,
            lev_gr_db: 0.0,
            hits: 0,
            spec_fast: [-120.0; NPTS],
            spec_avg: [f32::NAN; NPTS],
        }
    }
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum Mode {
    /// On a bus: the readouts describe this bus on its own.
    Bus,
    /// Last on the master: the readouts describe the finished mix, where the reference applies.
    Master,
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum HitPick {
    /// Profile the hits with the most 2-8 kHz energy (claps/snares); the rest only count as
    /// hits for the drums-vs-rest mask.
    #[name = "Loud HF hits"]
    LoudHf,
    /// Profile every detected hit.
    #[name = "All hits"]
    All,
}

#[derive(Params)]
pub struct BloaterParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<ViziaState>,
    /// JSON of the last analysed reference (`reference::ReferenceData`), kept with the project.
    #[persist = "reference"]
    pub reference_json: Arc<Mutex<String>>,

    #[id = "mode"]
    pub mode: EnumParam<Mode>,
    #[id = "drive"]
    pub drive: FloatParam,
    #[id = "clip"]
    pub clip: BoolParam,
    #[id = "soft"]
    pub soft: BoolParam,
    #[id = "soft_knee"]
    pub soft_knee: FloatParam,
    #[id = "soft_push"]
    pub soft_push: FloatParam,

    #[id = "tilt"]
    pub tilt: FloatParam,
    #[id = "bell"]
    pub bell: BoolParam,
    #[id = "bell_hz"]
    pub bell_hz: FloatParam,
    #[id = "bell_db"]
    pub bell_db: FloatParam,

    #[id = "lev"]
    pub lev: BoolParam,
    #[id = "lev_lo"]
    pub lev_lo: FloatParam,
    #[id = "lev_hi"]
    pub lev_hi: FloatParam,
    #[id = "lev_thr"]
    pub lev_thr: FloatParam,
    #[id = "lev_ratio"]
    pub lev_ratio: FloatParam,
    #[id = "lev_att"]
    pub lev_att: FloatParam,
    #[id = "lev_rel"]
    pub lev_rel: FloatParam,

    #[id = "sub"]
    pub sub: BoolParam,
    #[id = "sub_hz"]
    pub sub_hz: FloatParam,
    #[id = "sub_ceil"]
    pub sub_ceil: FloatParam,

    #[id = "sens"]
    pub sens: FloatParam,
    #[id = "pick"]
    pub pick: EnumParam<HitPick>,
    /// Measure per playback: statistics restart when the transport starts and hold when it
    /// stops, so one pass over a section gives one stable set of numbers.
    #[id = "per_play"]
    pub per_play: BoolParam,
}

fn db_param(name: &str, default: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min, max })
        .with_unit(" dB")
        .with_step_size(0.1)
        .with_value_to_string(formatters::v2s_f32_rounded(1))
}

fn hz_param(name: &str, default: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Skewed { min, max, factor: FloatRange::skew_factor(-2.0) })
        .with_unit(" Hz")
        .with_step_size(1.0)
        .with_value_to_string(formatters::v2s_f32_rounded(0))
}

fn ms_param(name: &str, default: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Skewed { min, max, factor: FloatRange::skew_factor(-1.5) })
        .with_unit(" ms")
        .with_step_size(0.1)
        .with_value_to_string(formatters::v2s_f32_rounded(1))
}

impl Default for BloaterParams {
    fn default() -> Self {
        BloaterParams {
            editor_state: editor::default_state(),
            reference_json: Arc::new(Mutex::new(String::new())),
            mode: EnumParam::new("Mode", Mode::Bus),
            drive: db_param("Drive", 0.0, -12.0, 24.0),
            clip: BoolParam::new("Clip", true),
            soft: BoolParam::new("Soft Stage", false),
            soft_knee: FloatParam::new("Soft Knee", 0.5, FloatRange::Linear { min: 0.1, max: 0.95 })
                .with_step_size(0.01)
                .with_value_to_string(formatters::v2s_f32_rounded(2)),
            // fine steps: the pinned share moves quickly with this (the soft stage crowds samples
            // just under the ceiling)
            soft_push: FloatParam::new("Soft Push", 0.22, FloatRange::Skewed { min: 0.0, max: 6.0, factor: FloatRange::skew_factor(-2.0) })
                .with_unit(" dB")
                .with_step_size(0.01)
                .with_value_to_string(formatters::v2s_f32_rounded(2)),
            tilt: db_param("Tilt", 0.0, -6.0, 6.0),
            bell: BoolParam::new("Bell", false),
            bell_hz: hz_param("Bell Freq", 2000.0, 100.0, 12000.0),
            bell_db: db_param("Bell Gain", 3.0, -12.0, 12.0),
            lev: BoolParam::new("Leveler", false),
            lev_lo: hz_param("Leveler Low", 250.0, 40.0, 2000.0),
            lev_hi: hz_param("Leveler High", 1000.0, 100.0, 8000.0),
            lev_thr: db_param("Leveler Threshold", -22.0, -50.0, 0.0),
            lev_ratio: FloatParam::new("Leveler Ratio", 3.0, FloatRange::Linear { min: 1.0, max: 10.0 })
                .with_unit(":1")
                .with_step_size(0.1)
                .with_value_to_string(formatters::v2s_f32_rounded(1)),
            lev_att: ms_param("Leveler Attack", 3.0, 0.1, 50.0),
            lev_rel: ms_param("Leveler Release", 80.0, 5.0, 1000.0),
            sub: BoolParam::new("Sub Split", false),
            sub_hz: hz_param("Sub Freq", 90.0, 40.0, 300.0),
            sub_ceil: db_param("Sub Ceiling", -3.0, -12.0, 0.0),
            sens: FloatParam::new("Hit Sensitivity", 0.5, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_step_size(0.01)
                .with_value_to_string(formatters::v2s_f32_rounded(2)),
            pick: EnumParam::new("Profile", HitPick::LoudHf),
            per_play: BoolParam::new("Measure per playback", true),
        }
    }
}

impl BloaterParams {
    pub fn bus_settings(&self) -> BusSettings {
        BusSettings {
            drive_db: self.drive.value(),
            tilt_db: self.tilt.value(),
            tilt_hz: 1000.0,
            bell_on: self.bell.value(),
            bell_hz: self.bell_hz.value(),
            bell_db: self.bell_db.value(),
            lev_on: self.lev.value(),
            lev_lo_hz: self.lev_lo.value(),
            lev_hi_hz: self.lev_hi.value().max(self.lev_lo.value() * 1.2),
            lev_thr_db: self.lev_thr.value(),
            lev_ratio: self.lev_ratio.value(),
            lev_attack_ms: self.lev_att.value(),
            lev_release_ms: self.lev_rel.value(),
            sub_on: self.sub.value(),
            sub_hz: self.sub_hz.value(),
            sub_ceil_db: self.sub_ceil.value(),
            clip_on: self.clip.value(),
            soft_on: self.soft.value(),
            soft_knee: self.soft_knee.value(),
            soft_push_db: self.soft_push.value(),
        }
    }
}

/// Requests from the editor to the audio thread.
#[derive(Default)]
pub struct Control {
    pub reset_stats: AtomicBool,
    /// Bumped by the editor when a new reference has been stored in `reference_json`.
    pub reference_version: AtomicU32,
}

pub struct Bloater {
    params: Arc<BloaterParams>,
    control: Arc<Control>,
    bus: Option<BusClip>,
    onset: Option<OnsetDetector>,
    analyzer: Option<Analyzer>,
    spectrum: Option<Spectrum>,
    snap_in: triple_buffer::Input<Snapshot>,
    snap_out: Arc<Mutex<triple_buffer::Output<Snapshot>>>,
    sample_rate: f32,
    since_report: usize,
    was_playing: bool,
    hits: u32,
    snapshot: Snapshot,
}

impl Default for Bloater {
    fn default() -> Self {
        let (snap_in, snap_out) = TripleBuffer::new(&Snapshot::default()).split();
        Bloater {
            params: Arc::new(BloaterParams::default()),
            control: Arc::new(Control::default()),
            bus: None,
            onset: None,
            analyzer: None,
            spectrum: None,
            snap_in,
            snap_out: Arc::new(Mutex::new(snap_out)),
            sample_rate: 44100.0,
            since_report: 0,
            was_playing: false,
            hits: 0,
            snapshot: Snapshot::default(),
        }
    }
}

impl Plugin for Bloater {
    const NAME: &'static str = "Bloater";
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
        editor::create(self.params.clone(), self.control.clone(), self.snap_out.clone(), self.params.editor_state.clone())
    }

    fn initialize(
        &mut self,
        audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        let nch = audio_io_layout.main_output_channels.map(|c| c.get() as usize).unwrap_or(2);
        let mut bus = BusClip::new(self.sample_rate, nch);
        bus.update(&self.params.bus_settings());
        self.bus = Some(bus);
        self.onset = Some(OnsetDetector::new(self.sample_rate));
        self.analyzer = Some(Analyzer::new(self.sample_rate));
        self.spectrum = Some(Spectrum::new(self.sample_rate));
        context.set_latency_samples(0);
        true
    }

    fn reset(&mut self) {
        if let Some(b) = &mut self.bus {
            b.reset();
        }
        if let Some(o) = &mut self.onset {
            o.reset();
        }
        if let Some(a) = &mut self.analyzer {
            a.reset();
        }
        if let Some(sp) = &mut self.spectrum {
            sp.reset();
        }
        self.hits = 0;
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let (Some(bus), Some(onset), Some(an), Some(spec)) = (&mut self.bus, &mut self.onset, &mut self.analyzer, &mut self.spectrum) else {
            return ProcessStatus::Normal;
        };
        if self.control.reset_stats.swap(false, Ordering::Relaxed) {
            an.clear_accumulators();
            spec.clear();
            self.hits = 0;
        }
        bus.update(&self.params.bus_settings());
        let len = buffer.samples();
        let slices = buffer.as_slice();
        bus.process(slices, len);
        // analysis on the clipped output
        let sens = self.params.sens.value();
        let kind = match self.params.pick.value() {
            HitPick::LoudHf => HitKind::Auto,
            HitPick::All => HitKind::Clap,
        };
        let nch = slices.len();
        // per-playback measurement: restart on play, hold (no analysis) while stopped
        let per_play = self.params.per_play.value();
        let playing = context.transport().playing;
        let mut force_report = false;
        if per_play {
            if playing && !self.was_playing {
                an.clear_accumulators();
                spec.clear();
                self.hits = 0;
            }
            if !playing && self.was_playing {
                an.flush();
                force_report = true;
            }
        }
        self.was_playing = playing;
        let analyse = !per_play || playing;
        let mut spec_frame = false;
        for i in 0..len {
            let l = slices[0][i];
            let r = if nch > 1 { slices[1][i] } else { l };
            // the live spectrum always runs; its average only while measuring
            spec_frame |= spec.push(0.5 * (l + r), analyse);
            if !analyse {
                continue;
            }
            an.push(l, r);
            if let Some(t) = onset.push(0.5 * (l + r), sens) {
                an.onset(t, kind);
                self.hits = self.hits.wrapping_add(1);
            }
        }
        self.since_report += len;
        let full = force_report || (analyse && self.since_report >= 4096);
        if full || spec_frame {
            self.snapshot.spec_fast = spec.fast_db;
            spec.average_db(&mut self.snapshot.spec_avg);
        }
        if !full && spec_frame {
            self.snap_in.write(self.snapshot);
        }
        if full {
            self.since_report = 0;
            self.snapshot.report = an.report();
            if let Some(p) = an.last_profile() {
                self.snapshot.last = *p;
                self.snapshot.has_last = true;
            }
            self.snapshot.in_peak_db = 20.0 * bus.in_peak.max(1e-6).log10();
            self.snapshot.block_pinned = bus.out_pinned;
            self.snapshot.lev_gr_db = bus.lev_gr_db;
            self.snapshot.hits = self.hits;
            self.snap_in.write(self.snapshot);
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for Bloater {
    const CLAP_ID: &'static str = "com.ogii.onsetguard";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Bus clipper with hit analysis and reference comparison");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Mono,
        ClapFeature::Distortion,
        ClapFeature::Analyzer,
    ];
}

impl Vst3Plugin for Bloater {
    const VST3_CLASS_ID: [u8; 16] = *b"OGIIOnsetGuard01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Distortion, Vst3SubCategory::Analyzer];
}

nih_export_clap!(Bloater);
nih_export_vst3!(Bloater);
