use nih_plug::prelude::*;
use onsetguard::OnsetGuard;

fn main() {
    nih_export_standalone::<OnsetGuard>();
}
