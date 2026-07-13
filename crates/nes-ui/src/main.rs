#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod persistence;
mod screenshot;

use std::{env, error::Error, path::PathBuf, process::ExitCode};

use app::App;
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageLevel};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            MessageDialog::new()
                .set_level(MessageLevel::Error)
                .set_title("My Own NES Emulator")
                .set_description(error.to_string())
                .set_buttons(MessageButtons::Ok)
                .show();
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let rom_path = env::args_os().nth(1).map(PathBuf::from).or_else(pick_rom);
    let Some(rom_path) = rom_path else {
        println!("No ROM selected.");
        return Ok(());
    };
    let native_options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 820.0])
            .with_min_inner_size([720.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "My Own NES Emulator",
        native_options,
        Box::new(move |cc| {
            App::new(rom_path, cc)
                .map(|app| Box::new(app) as Box<dyn eframe::App>)
                .map_err(|error| std::io::Error::other(error).into())
        }),
    )?;
    Ok(())
}

fn pick_rom() -> Option<PathBuf> {
    FileDialog::new()
        .set_title("Open an NES ROM")
        .add_filter("NES ROM", &["nes"])
        .pick_file()
}
