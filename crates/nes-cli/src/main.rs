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
            "usage: nes-cli <game.nes> [--frames N] [--insert-coin-at N] [--press-select-at N] [--press-start] [--press-start-at N] [--accuracycoin-page N] [--accuracycoin-repeat N] [--accuracycoin-report] [--peek ADDRESS] [--screenshot output.png]"
                .into(),
        );
    };
    let mut frames = None;
    let mut screenshot = None;
    let mut press_start_at = None;
    let mut insert_coin_at = None;
    let mut press_select_at = None;
    let mut accuracycoin_report = false;
    let mut accuracycoin_page = None;
    let mut accuracycoin_repeat = 1_u16;
    let mut peek_addresses = Vec::new();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--frames" => frames = Some(args.next().ok_or("--frames needs a number")?.parse()?),
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
            "--accuracycoin-page" => {
                let page: u8 = args
                    .next()
                    .ok_or("--accuracycoin-page needs a page number")?
                    .parse()?;
                if !(1..=20).contains(&page) {
                    return Err("--accuracycoin-page must be between 1 and 20".into());
                }
                accuracycoin_page = Some(page);
            }
            "--accuracycoin-repeat" => {
                accuracycoin_repeat = args
                    .next()
                    .ok_or("--accuracycoin-repeat needs a count")?
                    .parse()?;
                if accuracycoin_repeat == 0 {
                    return Err("--accuracycoin-repeat must be at least 1".into());
                }
            }
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

    if accuracycoin_page.is_some() && press_start_at.is_some() {
        return Err("--accuracycoin-page cannot be combined with --press-start".into());
    }
    if accuracycoin_page.is_none() && accuracycoin_repeat != 1 {
        return Err("--accuracycoin-repeat requires --accuracycoin-page".into());
    }

    let frames = frames.unwrap_or(if accuracycoin_page.is_some() {
        2_000
    } else {
        1
    });

    let rom = fs::read(&rom_path)?;
    let mut nes = Nes::from_ines(&rom)?;
    println!(
        "Loaded {rom_path} (mapper {}, battery: {})",
        nes.mapper_id(),
        nes.has_battery()
    );
    let mut accuracycoin_page_started = false;
    let mut accuracycoin_page_completed = false;
    let mut accuracycoin_page_runs = 0_u16;
    let mut accuracycoin_input_cooldown = 0_u8;
    for frame in 0..frames {
        if let Some(page) = accuracycoin_page {
            if accuracycoin_page_started
                && nes.cpu_state().program_counter == ACCURACYCOIN_MENU_IDLE_PC
            {
                accuracycoin_page_runs += 1;
                accuracycoin_page_started = false;
                if accuracycoin_repeat > 1 {
                    print!("AccuracyCoin page {page} run {accuracycoin_page_runs}:");
                    for address in &peek_addresses {
                        print!(" ${address:04X}=${:02X}", nes.peek_cpu(*address));
                    }
                    println!();
                }
                if accuracycoin_page_runs == accuracycoin_repeat {
                    accuracycoin_page_completed = true;
                    break;
                }
                accuracycoin_input_cooldown = 1;
            }
            let (right, activate) = if accuracycoin_input_cooldown > 0 {
                accuracycoin_input_cooldown -= 1;
                (false, false)
            } else {
                accuracycoin_page_buttons(
                    page,
                    frame,
                    nes.cpu_state().program_counter,
                    nes.peek_cpu(0x0014),
                    accuracycoin_page_started,
                )
            };
            let controller = nes.controller_mut(0).expect("player one controller exists");
            controller.set_button(Button::Right, right);
            controller.set_button(Button::A, activate);
            if right {
                accuracycoin_input_cooldown = 1;
            }
            accuracycoin_page_started |= activate;
        }
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
    if accuracycoin_page_started && nes.cpu_state().program_counter == ACCURACYCOIN_MENU_IDLE_PC {
        accuracycoin_page_runs += 1;
        accuracycoin_page_completed = accuracycoin_page_runs == accuracycoin_repeat;
    }
    if let Some(page) = accuracycoin_page
        && !accuracycoin_page_completed
    {
        return Err(format!(
            "AccuracyCoin page {page} did not finish within {frames} frames (PC=${:04X}, menu page={}, started={accuracycoin_page_started})",
            nes.cpu_state().program_counter,
            nes.peek_cpu(0x0014) + 1,
        )
        .into());
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
    if let Some(page) = accuracycoin_page {
        println!(
            "Completed AccuracyCoin page {page} {accuracycoin_page_runs} time(s) ({} tests per run)",
            nes.peek_cpu(0x0017),
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

const ACCURACYCOIN_MENU_READY_FRAME: u64 = 120;
const ACCURACYCOIN_MENU_IDLE_PC: u16 = 0x80df;

fn accuracycoin_page_buttons(
    page: u8,
    frame: u64,
    program_counter: u16,
    current_page: u8,
    started: bool,
) -> (bool, bool) {
    if frame < ACCURACYCOIN_MENU_READY_FRAME
        || program_counter != ACCURACYCOIN_MENU_IDLE_PC
        || started
    {
        return (false, false);
    }
    if current_page == page - 1 {
        (false, true)
    } else {
        (true, false)
    }
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

    #[test]
    fn accuracycoin_page_schedule_waits_for_the_idle_menu() {
        assert_eq!(
            accuracycoin_page_buttons(13, 119, 0x80df, 0, false),
            (false, false)
        );
        assert_eq!(
            accuracycoin_page_buttons(13, 120, 0x9000, 0, false),
            (false, false)
        );
        assert_eq!(
            accuracycoin_page_buttons(13, 120, 0x80df, 0, false),
            (true, false)
        );
        assert_eq!(
            accuracycoin_page_buttons(13, 120, 0x80df, 12, false),
            (false, true)
        );
        assert_eq!(
            accuracycoin_page_buttons(13, 120, 0x80df, 12, true),
            (false, false)
        );
    }
}
