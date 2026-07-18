#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod achievement_archive;
mod achievements;
mod app;
mod audio;
mod crt;
mod library;
mod palettes;
mod persistence;
mod save_states;
mod screenshot;
mod settings;
mod tas;
mod tas_control;

use std::{env, error::Error, path::PathBuf, process::ExitCode};

use app::App;
use rfd::{MessageButtons, MessageDialog, MessageLevel};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            MessageDialog::new()
                .set_level(MessageLevel::Error)
                .set_title("CrabNes")
                .set_description(error.to_string())
                .set_buttons(MessageButtons::Ok)
                .show();
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let rom_path = env::args_os().nth(1).map(PathBuf::from);

    let icon_data = {
        let img = image::open("icon.png").ok().map(|img| img.to_rgba8());
        img.map(|img| {
            let (w, h) = (img.width(), img.height());
            eframe::egui::IconData {
                rgba: img.into_raw(),
                width: w,
                height: h,
            }
        })
    };

    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([1100.0, 820.0])
        .with_min_inner_size([480.0, 360.0]);
    if let Some(icon) = icon_data {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }

    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "CrabNes",
        native_options,
        Box::new(move |cc| {
            App::new(rom_path, cc)
                .map(|app| Box::new(app) as Box<dyn eframe::App>)
                .map_err(|error| std::io::Error::other(error).into())
        }),
    )?;
    Ok(())
}
