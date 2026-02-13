mod app;
mod config;
mod ssh;
mod terminal;

fn main() -> eframe::Result {
    env_logger::init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 720.0])
            .with_min_inner_size([800.0, 500.0]),
        ..Default::default()
    };

    eframe::run_native(
        "SSHerald",
        options,
        Box::new(|cc| Ok(Box::new(app::AppState::new(cc)))),
    )
}
