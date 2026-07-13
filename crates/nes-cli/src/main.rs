use std::{
    env,
    fs::{self, File},
    io::{self, BufWriter},
    path::Path,
    process::ExitCode,
};

use nes_core::{FRAME_HEIGHT, FRAME_WIDTH, Nes};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let Some(rom_path) = args.next() else {
        return Err("usage: nes-cli <game.nes> [--frames N] [--screenshot output.png]".into());
    };
    let mut frames = 1_u64;
    let mut screenshot = None;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--frames" => frames = args.next().ok_or("--frames needs a number")?.parse()?,
            "--screenshot" => screenshot = Some(args.next().ok_or("--screenshot needs a path")?),
            _ => return Err(format!("unknown argument: {argument}").into()),
        }
    }

    let rom = fs::read(&rom_path)?;
    let mut nes = Nes::from_ines(&rom)?;
    println!(
        "Loaded {rom_path} (mapper {}, battery: {})",
        nes.mapper_id(),
        nes.has_battery()
    );
    for _ in 0..frames {
        nes.run_frame()?;
    }
    let state = nes.cpu_state();
    println!(
        "Frame {} | CPU PC={:04X} A={:02X} X={:02X} Y={:02X} P={:02X} SP={:02X} | {} cycles",
        nes.frame().number,
        state.program_counter,
        state.a,
        state.x,
        state.y,
        state.status,
        state.stack_pointer,
        nes.cpu_cycles()
    );
    if let Some(path) = screenshot {
        write_screenshot(Path::new(&path), &nes.frame().pixels)?;
        println!("Wrote {path}");
    }
    Ok(())
}

fn write_screenshot(path: &Path, pixels: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => write_png(path, pixels)?,
        Some("ppm") => write_ppm(path, pixels)?,
        Some(extension) => {
            return Err(format!(
                "unsupported screenshot extension '.{extension}'; use .png or .ppm"
            )
            .into());
        }
        None => return Err("screenshot path needs a .png or .ppm extension".into()),
    }
    Ok(())
}

fn write_png(path: &Path, pixels: &[u8]) -> Result<(), png::EncodingError> {
    let output = BufWriter::new(File::create(path)?);
    let mut encoder = png::Encoder::new(output, FRAME_WIDTH as u32, FRAME_HEIGHT as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(pixels)
}

fn write_ppm(path: &Path, pixels: &[u8]) -> io::Result<()> {
    let mut data = format!("P6\n{FRAME_WIDTH} {FRAME_HEIGHT}\n255\n").into_bytes();
    data.extend_from_slice(pixels);
    fs::write(path, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_standard_png_screenshot() {
        let path = env::temp_dir().join(format!("nes-cli-{}-screenshot.png", std::process::id()));
        let pixels = vec![0x7f; FRAME_WIDTH * FRAME_HEIGHT * 3];
        write_png(&path, &pixels).unwrap();
        let encoded = fs::read(&path).unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(&encoded[..8], b"\x89PNG\r\n\x1a\n");
    }
}
