use std::{
    env,
    fs::{self, File},
    io::{self, BufWriter},
    path::Path,
    process::ExitCode,
};

use nes_core::{Button, FRAME_HEIGHT, FRAME_WIDTH, Nes};

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
        return Err(
            "usage: nes-cli <game.nes> [--frames N] [--insert-coin-at N] [--press-select-at N] [--press-start] [--press-start-at N] [--accuracycoin-report] [--peek ADDRESS] [--screenshot output.png]"
                .into(),
        );
    };
    let mut frames = 1_u64;
    let mut screenshot = None;
    let mut press_start_at = None;
    let mut insert_coin_at = None;
    let mut press_select_at = None;
    let mut accuracycoin_report = false;
    let mut peek_addresses = Vec::new();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--frames" => frames = args.next().ok_or("--frames needs a number")?.parse()?,
            "--press-start" => press_start_at = Some(60),
            "--press-start-at" => {
                press_start_at = Some(
                    args.next()
                        .ok_or("--press-start-at needs a frame")?
                        .parse()?,
                )
            }
            "--insert-coin-at" => {
                insert_coin_at = Some(
                    args.next()
                        .ok_or("--insert-coin-at needs a frame")?
                        .parse()?,
                )
            }
            "--press-select-at" => {
                press_select_at = Some(
                    args.next()
                        .ok_or("--press-select-at needs a frame")?
                        .parse()?,
                )
            }
            "--accuracycoin-report" => accuracycoin_report = true,
            "--peek" => {
                let address = args.next().ok_or("--peek needs an address")?;
                let address = address
                    .strip_prefix("0x")
                    .or_else(|| address.strip_prefix("0X"))
                    .or_else(|| address.strip_prefix('$'))
                    .unwrap_or(&address);
                peek_addresses.push(u16::from_str_radix(address, 16)?);
            }
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
    for frame in 0..frames {
        if insert_coin_at == Some(frame) {
            nes.controller_mut(0)
                .expect("player one controller exists")
                .set_coin(true);
        } else if insert_coin_at.is_some_and(|coin| frame == coin.saturating_add(3)) {
            nes.controller_mut(0)
                .expect("player one controller exists")
                .set_coin(false);
        }
        if press_select_at == Some(frame) {
            nes.controller_mut(0)
                .expect("player one controller exists")
                .set_button(Button::Select, true);
        } else if press_select_at.is_some_and(|select| frame == select.saturating_add(1)) {
            nes.controller_mut(0)
                .expect("player one controller exists")
                .set_button(Button::Select, false);
        }
        if press_start_at == Some(frame) {
            nes.controller_mut(0)
                .expect("player one controller exists")
                .set_button(Button::Start, true);
        } else if press_start_at.is_some_and(|start| frame == start.saturating_add(1)) {
            nes.controller_mut(0)
                .expect("player one controller exists")
                .set_button(Button::Start, false);
        }
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
    if accuracycoin_report {
        println!(
            "AccuracyCoin: {}/{} passed",
            nes.peek_cpu(0x0038),
            nes.peek_cpu(0x0037)
        );
    }
    for address in peek_addresses {
        println!("${address:04X} = ${:02X}", nes.peek_cpu(address));
    }
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
