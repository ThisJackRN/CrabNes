use std::{
    error::Error,
    fs::{self, File},
    io::BufWriter,
    path::{Path, PathBuf},
};

use nes_core::{FRAME_HEIGHT, FRAME_WIDTH, Frame};

pub fn save(frame: &Frame, rom_path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let directory = rom_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("screenshots");
    fs::create_dir_all(&directory)?;
    let stem = rom_path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("nes");
    let path = directory.join(format!("{stem}-frame-{}.png", frame.number));

    let output = BufWriter::new(File::create(&path)?);
    let mut encoder = png::Encoder::new(output, FRAME_WIDTH as u32, FRAME_HEIGHT as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&frame.pixels)?;
    drop(writer);
    Ok(path)
}
