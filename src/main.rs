mod config;
mod detect;
mod rules;
mod transforms;

fn main() {
    let cfg = config::load_or_init().expect("config should load");
    println!("Pasteflow core loaded ({} rules)", cfg.rules.len());
}

