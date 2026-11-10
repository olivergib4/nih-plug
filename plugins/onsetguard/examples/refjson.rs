//! Print the reference analysis of a file as the JSON the plugin stores with a project:
//! `cargo run --release --example refjson -- FILE`
fn main() {
    let path = std::env::args().nth(1).expect("FILE");
    match onsetguard::reference::analyze_file(std::path::Path::new(&path)) {
        Ok(r) => println!("{}", serde_json::to_string(&r).unwrap()),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
