mod app;
mod config;
mod detect;
mod diff;
mod rules;
mod transforms;

fn main() {
    if let Err(err) = app::run() {
        eprintln!("Pasteflow failed: {err}");
    }
}
