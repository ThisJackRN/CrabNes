use std::{thread, time::Duration};

use nes_audio_native::NativeAudio;

fn main() -> Result<(), String> {
    let audio = NativeAudio::new(48_000, 1_920, 12_000)?;
    let silence = vec![0.0; 4_800];
    let written = audio.push(&silence);
    thread::sleep(Duration::from_millis(120));
    println!(
        "device={} device_rate={} written={} queued={} underflows={} overflows={}",
        audio.device_name(),
        audio.device_rate(),
        written,
        audio.queued_frames(),
        audio.underflows(),
        audio.overflows()
    );
    Ok(())
}
