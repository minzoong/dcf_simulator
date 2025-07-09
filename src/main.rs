
#[cfg(not(target_arch = "wasm32"))]
use dcf_simulator::AppState;

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result<()> {
    let app = AppState::default();
    eframe::run_native(
        "DCF simulator",
        eframe::NativeOptions::default(),
        Box::new(|_| Ok(Box::new(app))),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {}
