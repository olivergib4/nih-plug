use nih_plug::prelude::Editor;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::widgets::*;
use nih_plug_vizia::{assets, create_vizia_editor, ViziaState, ViziaTheming};
use std::sync::Arc;

use crate::OnsetGuardParams;

const COLUMN_WIDTH: Units = Pixels(340.0);
const GUI_WIDTH: u32 = 1040;
const GUI_HEIGHT: u32 = 640;

#[derive(Clone, Lens)]
pub struct Data {
    pub params: Arc<OnsetGuardParams>,
}

impl Model for Data {}

pub(crate) fn default_state() -> Arc<ViziaState> {
    ViziaState::new(|| (GUI_WIDTH, GUI_HEIGHT))
}

pub(crate) fn create(
    params: Arc<OnsetGuardParams>,
    editor_state: Arc<ViziaState>,
) -> Option<Box<dyn Editor>> {
    create_vizia_editor(editor_state, ViziaTheming::Custom, move |cx, _| {
        assets::register_noto_sans_light(cx);
        assets::register_noto_sans_thin(cx);

        Data {
            params: params.clone(),
        }
        .build(cx);

        VStack::new(cx, |cx| {
            HStack::new(cx, |cx| {
                Label::new(cx, "OnsetGuard")
                    .font_family(vec![FamilyOwned::Name(String::from(assets::NOTO_SANS))])
                    .font_weight(FontWeightKeyword::Thin)
                    .font_size(30.0)
                    .left(Pixels(10.0));
                Label::new(cx, "zero-phase spectral dynamics · onset protection · spectral clip")
                    .font_size(12.0)
                    .left(Pixels(14.0))
                    .top(Pixels(14.0));
            })
            .height(Pixels(40.0))
            .top(Pixels(6.0));

            HStack::new(cx, |cx| {
                make_column(cx, "Threshold", |cx| {
                    GenericUi::new(cx, Data::params.map(|p| p.threshold.clone()));
                });
                make_column(cx, "Dynamics", |cx| {
                    GenericUi::new(cx, Data::params.map(|p| p.dynamics.clone()));
                });
                make_column(cx, "Onset Protection", |cx| {
                    GenericUi::new(cx, Data::params.map(|p| p.protect.clone()));
                });
            })
            .size(Auto);

            HStack::new(cx, |cx| {
                make_column(cx, "Clip", |cx| {
                    GenericUi::new(cx, Data::params.map(|p| p.clip.clone()));
                });
                make_column(cx, "Resolution", |cx| {
                    GenericUi::new(cx, Data::params.map(|p| p.resolution.clone()));
                    Label::new(
                        cx,
                        "Latency = long + 2×short windows + lookahead. Ableton compensates; \
                         bounce for live use.",
                    )
                    .text_wrap(true)
                    .font_size(11.0)
                    .left(Pixels(15.0))
                    .right(Pixels(8.0))
                    .width(Stretch(1.0));
                });
                make_column(cx, "Output", |cx| {
                    GenericUi::new(cx, Data::params.map(|p| p.global.clone()));
                    Label::new(
                        cx,
                        "Matched Bypass plays the dry signal at the processed signal's \
                         K-weighted level, so you compare shape, not volume.",
                    )
                    .text_wrap(true)
                    .font_size(11.0)
                    .left(Pixels(15.0))
                    .right(Pixels(8.0))
                    .width(Stretch(1.0));
                });
            })
            .size(Auto);
        })
        .row_between(Pixels(10.0))
        .child_left(Stretch(1.0))
        .child_right(Stretch(1.0));

        ResizeHandle::new(cx);
    })
}

fn make_column(cx: &mut Context, title: &str, contents: impl FnOnce(&mut Context)) {
    VStack::new(cx, |cx| {
        Label::new(cx, title)
            .font_family(vec![FamilyOwned::Name(String::from(assets::NOTO_SANS))])
            .font_weight(FontWeightKeyword::Thin)
            .font_size(21.0)
            .left(Stretch(1.0))
            .right(Pixels(7.0))
            .bottom(Pixels(-10.0));

        contents(cx);
    })
    .width(COLUMN_WIDTH)
    .height(Auto);
}
