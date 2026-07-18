//! Temporary diagnostic: replay an FM2 movie against a ROM and report
//! per-frame CPU cycle counts and lag-frame accounting around a target frame
//! range, to compare against FCEUX's own lag counter for the same movie.
//! Not part of the shipped tool; delete after use.

use std::{env, fs, fs::File, io::BufWriter, path::Path};

use nes_core::{Button, FRAME_HEIGHT, FRAME_WIDTH, Nes};

/// Minimal standard-alphabet base64 decoder (with `=` padding) so this
/// example does not need an extra dependency.
fn decode_base64(text: &str) -> Result<Vec<u8>, String> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u8;
    for byte in text.bytes() {
        if byte == b'=' || byte.is_ascii_whitespace() {
            continue;
        }
        let value = ALPHABET
            .iter()
            .position(|&candidate| candidate == byte)
            .ok_or_else(|| format!("invalid base64 byte {byte:#04x}"))? as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((buffer >> bits) as u8);
        }
    }
    Ok(output)
}

fn write_png(path: impl AsRef<Path>, pixels: &[u8]) -> Result<(), png::EncodingError> {
    let output = BufWriter::new(File::create(path)?);
    let mut encoder = png::Encoder::new(output, FRAME_WIDTH as u32, FRAME_HEIGHT as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(pixels)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let rom_path = args
        .next()
        .ok_or("usage: fm2_repro <rom.nes> <movie.fm2> [focus_start] [focus_end] [run_limit]")?;
    let fm2_path = args
        .next()
        .ok_or("usage: fm2_repro <rom.nes> <movie.fm2> [focus_start] [focus_end] [run_limit]")?;
    let focus_start: usize = args.next().map(|v| v.parse()).transpose()?.unwrap_or(0);
    let focus_end: usize = args
        .next()
        .map(|v| v.parse())
        .transpose()?
        .unwrap_or(usize::MAX);

    let rom = fs::read(&rom_path)?;
    let text = fs::read_to_string(&fm2_path)?;
    let mut nes = Nes::from_ines(&rom)?;
    // FM2 movies were recorded against FCEUX's simplified joypad clocking,
    // exactly as the TAS Control View conversion plays them back.
    nes.set_fceux_joypad_compat(true);
    // Savestate-anchored movies embed an FCEUX FCS state; import it exactly
    // as the TAS Control View's embedded-state conversion path does.
    if let Some(state_line) = text
        .lines()
        .find_map(|line| line.strip_prefix("savestate base64:"))
    {
        let state = decode_base64(state_line.trim())?;
        nes.import_fceux_state(&state)?;
        println!(
            "Imported embedded FCEUX state; machine resumes at frame {}",
            nes.frame().number
        );
    }
    println!(
        "Loaded {rom_path} (mapper {}, FCEUX joypad compat: {})",
        nes.mapper_id(),
        nes.fceux_joypad_compat()
    );

    let mut frames = parse_fm2_frames(&text);
    println!("Parsed {} input frames from {fm2_path}", frames.len());
    let run_limit: usize = args
        .next()
        .map(|v| v.parse())
        .transpose()?
        .unwrap_or(frames.len());
    frames.truncate(run_limit);

    let mut lag_frames = 0u64;
    let mut lag_transitions = Vec::new();

    for (index, (p1, p2)) in frames.iter().enumerate() {
        apply_mask(&mut nes, 0, *p1);
        apply_mask(&mut nes, 1, *p2);

        let reads_before = nes
            .controller_reads(0)
            .wrapping_add(nes.controller_reads(1));
        let cycles_before = nes.cpu_cycles();
        nes.run_frame()?;
        let cycles_after = nes.cpu_cycles();
        let reads_after = nes
            .controller_reads(0)
            .wrapping_add(nes.controller_reads(1));

        let is_lag = reads_after == reads_before;
        let lag_before = lag_frames;
        if is_lag {
            lag_frames += 1;
            lag_transitions.push((index, lag_frames));
        }

        if index >= focus_start && index <= focus_end {
            println!(
                "movie_frame={index:06} game_frame={} cycles_this_frame={} lag_total={} {}",
                nes.frame().number,
                cycles_after - cycles_before,
                lag_frames,
                if is_lag {
                    format!("<-- LAG (was {lag_before})")
                } else {
                    String::new()
                }
            );
        }
        if index == focus_end {
            write_png("target/fm2_focus_end.png", &nes.frame().pixels)?;
        }
    }
    write_png("target/fm2_final.png", &nes.frame().pixels)?;
    // Let self-running payloads (ACE runs, auto-scrolling endings) resolve
    // after the last input frame, then capture the outcome.
    apply_mask(&mut nes, 0, 0);
    apply_mask(&mut nes, 1, 0);
    for _ in 0..600 {
        nes.run_frame()?;
    }
    write_png("target/fm2_after.png", &nes.frame().pixels)?;
    println!("Wrote target/fm2_focus_end.png, target/fm2_final.png, target/fm2_after.png");

    println!("Total lag frames over whole movie: {lag_frames}");
    println!("Final CPU cycles: {}", nes.cpu_cycles());
    println!(
        "First 20 lag transitions: {:?}",
        &lag_transitions[..lag_transitions.len().min(20)]
    );
    Ok(())
}

fn apply_mask(nes: &mut Nes, port: usize, mask: u8) {
    if let Some(controller) = nes.controller_mut(port) {
        for (index, button) in [
            Button::A,
            Button::B,
            Button::Select,
            Button::Start,
            Button::Up,
            Button::Down,
            Button::Left,
            Button::Right,
        ]
        .into_iter()
        .enumerate()
        {
            controller.set_button(button, mask & (1 << index) != 0);
        }
    }
}

/// Minimal FM2 input parser mirroring CrabNes's own tas_control.rs mapping
/// (character order Right,Left,Down,Up,Start,Select,B,A -> bits 7..0).
fn parse_fm2_frames(text: &str) -> Vec<(u8, u8)> {
    let mut frames = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if !line.starts_with('|') {
            continue;
        }
        let fields: Vec<_> = line.split('|').collect();
        if fields.len() < 5 {
            continue;
        }
        let p1 = parse_pad(fields[2]);
        let p2 = parse_pad(fields[3]);
        frames.push((p1, p2));
    }
    frames
}

fn parse_pad(field: &str) -> u8 {
    if field.len() != 8 {
        return 0;
    }
    let bits = [7u8, 6, 5, 4, 3, 2, 1, 0];
    let mut mask = 0u8;
    for (index, character) in field.chars().enumerate() {
        if !matches!(character, '.' | ' ') {
            mask |= 1 << bits[index];
        }
    }
    mask
}
