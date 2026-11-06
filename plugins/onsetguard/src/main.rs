use nih_plug::prelude::*;
use onsetguard::Bloater;

fn main() {
    nih_export_standalone::<Bloater>();
}
