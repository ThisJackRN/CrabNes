//! Deterministic controller-input smoke runner for gameplay regression testing.

use std::{
    env, fs,
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};

use nes_core::{Button, FRAME_HEIGHT, FRAME_WIDTH, Nes};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args()
        .nth(1)
        .ok_or("usage: autoplay <game.nes> [frames] [capture.wav] [idle]")?;
    let frames = env::args()
        .nth(2)
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(3_600);
    let capture_path = env::args().nth(3);
    let idle = env::args().nth(4).as_deref() == Some("idle");
    let rom = fs::read(&path)?;
    let mut nes = Nes::from_ines(&rom)?;
    let mut audio = Vec::new();
    let mut audio_energy = 0.0_f64;
    let mut audio_peak = 0.0_f32;
    let mut audio_samples = 0_usize;
    let mut captured_audio = capture_path.as_ref().map(|_| Vec::new());

    for frame in 0..frames {
        let controller = nes.controller_mut(0).expect("controller one");
        controller.set_button(Button::Start, (100..103).contains(&frame));
        controller.set_button(Button::Right, !idle && frame >= 180);
        // Repeated full jumps exercise scrolling, enemies, pits, and controller IO.
        controller.set_button(Button::A, !idle && frame >= 240 && (frame - 240) % 90 < 18);
        nes.run_frame()?;
        audio.clear();
        nes.drain_audio_samples(&mut audio);
        audio_samples += audio.len();
        if let Some(captured_audio) = &mut captured_audio {
            captured_audio.extend_from_slice(&audio);
        }
        for sample in &audio {
            audio_energy += f64::from(*sample) * f64::from(*sample);
            audio_peak = audio_peak.max(sample.abs());
        }

        if frame % 300 == 299 {
            println!(
                "frame={} pc={:04X} x={:02X} cycles={} ppu={:?} apu={:?}",
                nes.frame().number,
                nes.cpu_state().program_counter,
                nes.cpu_state().x,
                nes.cpu_cycles(),
                nes.ppu_state(),
                nes.apu_state()
            );
            write_png(
                Path::new("target").join(format!("autoplay-{}.png", frame + 1)),
                &nes.frame().pixels,
            )?;
        }
    }

    let name = Path::new(&path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("ROM");
    let rms = (audio_energy / audio_samples.max(1) as f64).sqrt();
    println!(
        "Autoplay smoke completed for {name}; audio samples={audio_samples} peak={audio_peak:.3} rms={rms:.3}"
    );
    if let (Some(path), Some(samples)) = (capture_path, captured_audio) {
        write_wav(&path, 48_000, &samples)?;
        println!("Wrote audio capture to {path}");
    }
    Ok(())
}

fn write_wav(path: impl AsRef<Path>, sample_rate: u32, samples: &[f32]) -> std::io::Result<()> {
    let mut output = BufWriter::new(File::create(path)?);
    let data_size = u32::try_from(samples.len().saturating_mul(2)).unwrap_or(u32::MAX);
    output.write_all(b"RIFF")?;
    output.write_all(&(36_u32.saturating_add(data_size)).to_le_bytes())?;
    output.write_all(b"WAVEfmt ")?;
    output.write_all(&16_u32.to_le_bytes())?;
    output.write_all(&1_u16.to_le_bytes())?;
    output.write_all(&1_u16.to_le_bytes())?;
    output.write_all(&sample_rate.to_le_bytes())?;
    output.write_all(&(sample_rate * 2).to_le_bytes())?;
    output.write_all(&2_u16.to_le_bytes())?;
    output.write_all(&16_u16.to_le_bytes())?;
    output.write_all(b"data")?;
    output.write_all(&data_size.to_le_bytes())?;
    for &sample in samples {
        let pcm = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        output.write_all(&pcm.to_le_bytes())?;
    }
    output.flush()
}

fn write_png(path: impl AsRef<Path>, pixels: &[u8]) -> Result<(), png::EncodingError> {
    let output = BufWriter::new(File::create(path)?);
    let mut encoder = png::Encoder::new(output, FRAME_WIDTH as u32, FRAME_HEIGHT as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(pixels)
}
