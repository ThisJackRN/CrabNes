use serde::{Deserialize, Serialize};

use super::{
    CartridgeError, Mirroring,
    mapper::{Mapper, MapperSnapshot},
};

const FDS_SIDE_SIZE: usize = 65_500;
const INITIAL_GAP_BYTES: usize = 28_300 / 8;
const BLOCK_GAP_BYTES: usize = 976 / 8;
const BYTE_TRANSFER_DELAY: u32 = 149;
const HEAD_REWIND_DELAY: u32 = 50_000;

#[derive(Clone, Default, Serialize, Deserialize)]
struct FdsEnvelope {
    speed: u8,
    gain: u8,
    disabled: bool,
    increase: bool,
    frequency: u16,
    timer: u32,
    master_speed: u8,
}

impl FdsEnvelope {
    fn new() -> Self {
        Self {
            master_speed: 0xe8,
            ..Self::default()
        }
    }

    fn reset_timer(&mut self) {
        self.timer = 8 * u32::from(self.speed + 1) * u32::from(self.master_speed);
    }

    fn write_envelope(&mut self, value: u8) {
        self.speed = value & 0x3f;
        self.increase = value & 0x40 != 0;
        self.disabled = value & 0x80 != 0;
        self.reset_timer();
        if self.disabled {
            self.gain = self.speed;
        }
    }

    fn tick(&mut self) -> bool {
        if self.disabled || self.master_speed == 0 {
            return false;
        }
        if self.timer > 1 {
            self.timer -= 1;
            return false;
        }
        self.reset_timer();
        if self.increase && self.gain < 32 {
            self.gain += 1;
        } else if !self.increase && self.gain > 0 {
            self.gain -= 1;
        }
        true
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct FdsAudio {
    wave_table: Vec<u8>,
    wave_write_enabled: bool,
    volume: FdsEnvelope,
    modulation: FdsEnvelope,
    modulation_disabled: bool,
    modulation_counter: i8,
    modulation_table: Vec<u8>,
    modulation_position: u8,
    modulation_accumulator: u16,
    modulation_output: i32,
    envelopes_disabled: bool,
    waveform_halted: bool,
    master_volume: u8,
    wave_accumulator: u16,
    wave_position: u8,
}

impl Default for FdsAudio {
    fn default() -> Self {
        Self {
            wave_table: vec![0; 64],
            wave_write_enabled: false,
            volume: FdsEnvelope::new(),
            modulation: FdsEnvelope::new(),
            modulation_disabled: false,
            modulation_counter: 0,
            modulation_table: vec![0; 64],
            modulation_position: 0,
            modulation_accumulator: 0,
            modulation_output: 0,
            envelopes_disabled: false,
            waveform_halted: false,
            master_volume: 0,
            wave_accumulator: 0,
            wave_position: 0,
        }
    }
}

impl FdsAudio {
    fn read(&self, address: u16) -> u8 {
        match address {
            0x4040..=0x407f => {
                let position = if self.wave_write_enabled {
                    usize::from(address & 0x3f)
                } else {
                    usize::from(self.wave_position)
                };
                self.wave_table[position]
            }
            0x4090 => self.volume.gain,
            0x4092 => self.modulation.gain,
            _ => 0,
        }
    }

    fn write(&mut self, address: u16, value: u8) {
        match address {
            0x4040..=0x407f if self.wave_write_enabled => {
                self.wave_table[usize::from(address & 0x3f)] = value & 0x3f;
            }
            0x4080 => self.volume.write_envelope(value),
            0x4082 => self.volume.frequency = (self.volume.frequency & 0x0f00) | u16::from(value),
            0x4083 => {
                self.volume.frequency =
                    (self.volume.frequency & 0x00ff) | (u16::from(value & 0x0f) << 8);
                self.envelopes_disabled = value & 0x40 != 0;
                self.waveform_halted = value & 0x80 != 0;
                if self.waveform_halted {
                    self.wave_position = 0;
                }
                if self.envelopes_disabled {
                    self.volume.reset_timer();
                    self.modulation.reset_timer();
                }
            }
            0x4084 => self.modulation.write_envelope(value),
            0x4085 => self.set_modulation_counter(value & 0x7f),
            0x4086 => {
                self.modulation.frequency = (self.modulation.frequency & 0x0f00) | u16::from(value);
            }
            0x4087 => {
                self.modulation.frequency =
                    (self.modulation.frequency & 0x00ff) | (u16::from(value & 0x0f) << 8);
                self.modulation_disabled = value & 0x80 != 0;
                if self.modulation_disabled {
                    self.modulation_accumulator = 0;
                }
            }
            0x4088 if self.modulation_disabled => {
                for offset in 0..2 {
                    self.modulation_table
                        [usize::from((self.modulation_position + offset) & 0x3f)] = value & 7;
                }
                self.modulation_position = (self.modulation_position + 2) & 0x3f;
            }
            0x4089 => {
                self.master_volume = value & 3;
                self.wave_write_enabled = value & 0x80 != 0;
            }
            0x408a => {
                self.volume.master_speed = value;
                self.modulation.master_speed = value;
            }
            _ => {}
        }
        self.update_modulation_output();
    }

    fn set_modulation_counter(&mut self, value: u8) {
        self.modulation_counter = if value >= 64 {
            (i16::from(value) - 128) as i8
        } else {
            value as i8
        };
    }

    fn update_modulation_output(&mut self) {
        let mut value = i32::from(self.modulation_counter) * i32::from(self.modulation.gain);
        let remainder = value & 0x0f;
        value >>= 4;
        if remainder > 0 && value & 0x80 == 0 {
            value += if self.modulation_counter < 0 { -1 } else { 2 };
        }
        if value >= 192 {
            value -= 256;
        } else if value < -64 {
            value += 256;
        }
        value *= i32::from(self.volume.frequency);
        let remainder = value & 0x3f;
        value >>= 6;
        if remainder >= 32 {
            value += 1;
        }
        self.modulation_output = value;
    }

    fn clock(&mut self) {
        if !self.waveform_halted && !self.envelopes_disabled {
            self.volume.tick();
            if self.modulation.tick() {
                self.update_modulation_output();
            }
        }

        if !self.modulation_disabled && self.modulation.frequency != 0 {
            let old = self.modulation_accumulator;
            self.modulation_accumulator = old.wrapping_add(self.modulation.frequency);
            if self.modulation_accumulator < old {
                const OFFSETS: [i16; 8] = [0, 1, 2, 4, 0x100, -4, -2, -1];
                let offset =
                    OFFSETS[self.modulation_table[usize::from(self.modulation_position)] as usize];
                if offset == 0x100 {
                    self.modulation_counter = 0;
                } else {
                    self.set_modulation_counter(
                        (i16::from(self.modulation_counter) + offset) as u8 & 0x7f,
                    );
                }
                self.modulation_position = (self.modulation_position + 1) & 0x3f;
                self.update_modulation_output();
            }
        }

        let pitch = i32::from(self.volume.frequency) + self.modulation_output;
        if !self.waveform_halted && pitch > 0 {
            let old = self.wave_accumulator;
            self.wave_accumulator = old.wrapping_add(pitch as u16);
            if self.wave_accumulator < old {
                self.wave_position = (self.wave_position + 1) & 0x3f;
            }
        }
    }

    fn output(&self) -> f32 {
        if self.wave_write_enabled {
            return 0.0;
        }
        const MASTER_LEVEL: [u32; 4] = [36, 24, 17, 14];
        let gain = u32::from(self.volume.gain.min(32));
        let level = u32::from(self.wave_table[usize::from(self.wave_position)])
            * gain
            * MASTER_LEVEL[usize::from(self.master_volume)]
            / 1152;
        level as f32 / 63.0 * 0.12
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct FdsSnapshot {
    prg_ram: Vec<u8>,
    chr_ram: Vec<u8>,
    disk_sides: Vec<Vec<u8>>,
    current_side: usize,
    disk_inserted: bool,
    mirroring: Mirroring,
    timer_irq_reload: u16,
    timer_irq_counter: u16,
    timer_irq_enabled: bool,
    timer_irq_repeat: bool,
    timer_irq_pending: bool,
    disk_io_enabled: bool,
    sound_io_enabled: bool,
    motor_on: bool,
    transfer_reset: bool,
    read_mode: bool,
    crc_control: bool,
    disk_ready: bool,
    disk_irq_enabled: bool,
    disk_position: usize,
    transfer_delay: u32,
    disk_irq_pending: bool,
    transfer_complete: bool,
    read_data: u8,
    write_data: u8,
    gap_ended: bool,
    scanning_disk: bool,
    end_of_head: bool,
    audio: FdsAudio,
}

/// Famicom Disk System RAM adapter and drive.
///
/// fwNES `.fds` images omit the physical gaps, sync marks, and CRC bytes that
/// the RP2C33 sees. `expand_disk_side` restores those bytes once at load time,
/// which keeps the runtime drive logic byte-oriented and also supports custom
/// disk readers instead of special-casing the BIOS block routine.
pub struct Fds {
    bios_rom: Vec<u8>,
    prg_ram: Vec<u8>,
    chr_ram: Vec<u8>,
    disk_sides: Vec<Vec<u8>>,
    side_offsets: Vec<usize>,
    battery_data: Vec<u8>,
    current_side: usize,
    disk_inserted: bool,
    mirroring: Mirroring,

    timer_irq_reload: u16,
    timer_irq_counter: u16,
    timer_irq_enabled: bool,
    timer_irq_repeat: bool,
    timer_irq_pending: bool,

    disk_io_enabled: bool,
    sound_io_enabled: bool,

    motor_on: bool,
    transfer_reset: bool,
    read_mode: bool,
    crc_control: bool,
    disk_ready: bool,
    disk_irq_enabled: bool,

    disk_position: usize,
    transfer_delay: u32,
    disk_irq_pending: bool,
    transfer_complete: bool,
    read_data: u8,
    write_data: u8,
    gap_ended: bool,
    scanning_disk: bool,
    end_of_head: bool,
    audio: FdsAudio,
}

impl Fds {
    pub fn new(bios: &[u8], disk_data: &[u8]) -> Result<Self, CartridgeError> {
        if bios.len() != 0x2000 {
            return Err(CartridgeError::InvalidFdsBiosSize(bios.len()));
        }

        let (side_count, payload) = if disk_data.starts_with(b"FDS\x1a") {
            if disk_data.len() < 16 {
                return Err(CartridgeError::InvalidFdsImage(
                    "the FDS header is truncated",
                ));
            }
            let side_count = usize::from(disk_data[4]);
            if side_count == 0 {
                return Err(CartridgeError::InvalidFdsImage(
                    "the FDS header declares zero disk sides",
                ));
            }
            (side_count, &disk_data[16..])
        } else {
            if disk_data.is_empty() || !disk_data.len().is_multiple_of(FDS_SIDE_SIZE) {
                return Err(CartridgeError::InvalidFdsImage(
                    "headerless images must contain complete 65500-byte sides",
                ));
            }
            (disk_data.len() / FDS_SIDE_SIZE, disk_data)
        };

        let expected = side_count
            .checked_mul(FDS_SIDE_SIZE)
            .ok_or(CartridgeError::RomSizeOverflow)?;
        if payload.len() < expected {
            return Err(CartridgeError::Truncated {
                expected: expected + usize::from(disk_data.starts_with(b"FDS\x1a")) * 16,
                actual: disk_data.len(),
            });
        }

        let disk_sides: Vec<_> = payload[..expected]
            .chunks_exact(FDS_SIDE_SIZE)
            .map(expand_disk_side)
            .collect();
        let (side_offsets, battery_data) = flatten_disk_sides(&disk_sides);

        Ok(Self {
            bios_rom: bios.to_vec(),
            prg_ram: vec![0; 0x8000],
            chr_ram: vec![0; 0x2000],
            disk_sides,
            side_offsets,
            battery_data,
            current_side: 0,
            // Loading a game is equivalent to powering on with side A in the
            // drive. Side changes still use an explicit eject/insert cycle.
            disk_inserted: true,
            mirroring: Mirroring::Vertical,
            timer_irq_reload: 0,
            timer_irq_counter: 0,
            timer_irq_enabled: false,
            timer_irq_repeat: false,
            timer_irq_pending: false,
            disk_io_enabled: false,
            sound_io_enabled: false,
            motor_on: false,
            transfer_reset: true,
            read_mode: true,
            crc_control: false,
            disk_ready: false,
            disk_irq_enabled: false,
            disk_position: 0,
            transfer_delay: 0,
            disk_irq_pending: false,
            transfer_complete: false,
            read_data: 0,
            write_data: 0,
            gap_ended: false,
            scanning_disk: false,
            end_of_head: true,
            audio: FdsAudio::default(),
        })
    }

    fn reset_disk_io(&mut self) {
        // Hardware reset value of $4025 is $06: motor off, transfer reset,
        // read mode. The mirroring latch and disk contents are retained.
        self.motor_on = false;
        self.transfer_reset = true;
        self.read_mode = true;
        self.crc_control = false;
        self.disk_ready = false;
        self.disk_irq_enabled = false;
        self.disk_irq_pending = false;
        self.transfer_complete = false;
        self.scanning_disk = false;
        self.end_of_head = true;
    }

    fn clock_timer_irq(&mut self) {
        if !self.timer_irq_enabled {
            return;
        }
        if self.timer_irq_counter == 0 {
            self.timer_irq_pending = true;
            self.timer_irq_counter = self.timer_irq_reload;
            if !self.timer_irq_repeat {
                self.timer_irq_enabled = false;
            }
        } else {
            self.timer_irq_counter -= 1;
        }
    }

    fn clock_disk(&mut self) {
        if !self.disk_inserted || !self.motor_on {
            self.end_of_head = true;
            self.scanning_disk = false;
            return;
        }
        if self.transfer_reset && !self.scanning_disk {
            return;
        }
        if self.end_of_head {
            self.transfer_delay = HEAD_REWIND_DELAY;
            self.end_of_head = false;
            self.disk_position = 0;
            self.gap_ended = false;
            return;
        }
        if self.transfer_delay != 0 {
            self.transfer_delay -= 1;
            return;
        }

        self.scanning_disk = true;
        let battery_position = self.side_offsets[self.current_side] + self.disk_position;
        let Some(side) = self.disk_sides.get_mut(self.current_side) else {
            return;
        };
        if self.disk_position >= side.len() {
            self.motor_on = false;
            self.end_of_head = true;
            if self.disk_irq_enabled {
                self.disk_irq_pending = true;
            }
            return;
        }

        if self.read_mode {
            let disk_byte = side[self.disk_position];
            if !self.disk_ready {
                self.gap_ended = false;
            } else if !self.gap_ended && disk_byte != 0 {
                // The first set bit terminates a physical gap. Its containing
                // sync byte participates in CRC but is not delivered to $4031.
                self.gap_ended = true;
            } else if self.gap_ended {
                self.read_data = disk_byte;
                self.transfer_complete = true;
                if self.disk_irq_enabled {
                    self.disk_irq_pending = true;
                }
            }
        } else {
            let disk_byte = if !self.disk_ready {
                0
            } else if self.crc_control {
                // `.fds` stores no CRC. Keep the placeholder bytes stable;
                // reads report a passing CRC, while ordinary data remains
                // writable for games that save to disk.
                side[self.disk_position]
            } else {
                self.transfer_complete = true;
                if self.disk_irq_enabled {
                    self.disk_irq_pending = true;
                }
                self.write_data
            };
            side[self.disk_position] = disk_byte;
            self.battery_data[battery_position] = disk_byte;
            self.gap_ended = false;
        }

        self.disk_position += 1;
        self.transfer_delay = BYTE_TRANSFER_DELAY;
    }
}

/// Convert one logical fwNES side into the byte stream present on physical
/// media. Standard block lengths are self-describing except block 4, whose
/// length comes from the immediately preceding block 3 header.
fn expand_disk_side(side: &[u8]) -> Vec<u8> {
    let mut physical = Vec::with_capacity(side.len());
    physical.resize(INITIAL_GAP_BYTES, 0);
    let mut offset = 0;
    let mut file_size = 0usize;

    while offset < side.len() {
        let block_len = match side[offset] {
            1 => 56,
            2 => 2,
            3 if offset + 16 <= side.len() => {
                file_size = usize::from(side[offset + 13]) | (usize::from(side[offset + 14]) << 8);
                16
            }
            4 => 1 + file_size,
            _ => {
                // Preserve non-standard formats verbatim after a sync mark.
                // This is preferable to rejecting copy protection/custom
                // loaders merely because their blocks are not BIOS-shaped.
                physical.push(0x80);
                physical.extend_from_slice(&side[offset..]);
                break;
            }
        };
        if offset + block_len > side.len() {
            physical.push(0x80);
            physical.extend_from_slice(&side[offset..]);
            break;
        }

        physical.push(0x80);
        physical.extend_from_slice(&side[offset..offset + block_len]);
        // A known-good dummy CRC is sufficient for `.fds`: the image format
        // omitted the originals, and this implementation never raises D4.
        physical.extend_from_slice(&[0x4d, 0x62]);
        physical.resize(physical.len() + BLOCK_GAP_BYTES, 0);
        offset += block_len;
    }

    physical.resize(physical.len().max(FDS_SIDE_SIZE), 0);
    physical
}

fn flatten_disk_sides(sides: &[Vec<u8>]) -> (Vec<usize>, Vec<u8>) {
    let mut offsets = Vec::with_capacity(sides.len());
    let mut data = Vec::new();
    for side in sides {
        offsets.push(data.len());
        data.extend_from_slice(side);
    }
    (offsets, data)
}

impl Mapper for Fds {
    fn swap_disk(&mut self) {
        if self.disk_inserted {
            self.disk_inserted = false;
            self.motor_on = false;
            self.scanning_disk = false;
            self.end_of_head = true;
        } else {
            self.current_side = (self.current_side + 1) % self.disk_sides.len();
            self.disk_inserted = true;
            self.disk_position = 0;
            self.transfer_delay = 0;
            self.gap_ended = false;
            self.scanning_disk = false;
            self.end_of_head = true;
        }
        self.disk_irq_pending = false;
        self.transfer_complete = false;
    }

    fn cpu_read(&mut self, address: u16) -> Option<u8> {
        match address {
            0x4030 => {
                let status = u8::from(self.timer_irq_pending)
                    | (u8::from(self.transfer_complete) << 1)
                    | (u8::from(self.mirroring == Mirroring::Horizontal) << 3)
                    | (u8::from(self.end_of_head) << 6);
                self.timer_irq_pending = false;
                self.disk_irq_pending = false;
                self.transfer_complete = false;
                Some(status)
            }
            0x4031 => {
                self.disk_irq_pending = false;
                self.transfer_complete = false;
                Some(self.read_data)
            }
            0x4032 => {
                self.disk_irq_pending = false;
                let not_inserted = !self.disk_inserted;
                Some(
                    u8::from(not_inserted)
                        | (u8::from(not_inserted || !self.scanning_disk) << 1)
                        | (u8::from(not_inserted) << 2),
                )
            }
            0x4033 => Some(0x80),
            0x4040..=0x4092 if self.sound_io_enabled => Some(self.audio.read(address)),
            _ => self.cpu_peek(address),
        }
    }

    fn cpu_peek(&self, address: u16) -> Option<u8> {
        match address {
            0x4030 => Some(
                u8::from(self.timer_irq_pending)
                    | (u8::from(self.transfer_complete) << 1)
                    | (u8::from(self.mirroring == Mirroring::Horizontal) << 3)
                    | (u8::from(self.end_of_head) << 6),
            ),
            0x4031 => Some(self.read_data),
            0x4032 => {
                let not_inserted = !self.disk_inserted;
                Some(
                    u8::from(not_inserted)
                        | (u8::from(not_inserted || !self.scanning_disk) << 1)
                        | (u8::from(not_inserted) << 2),
                )
            }
            0x4033 => Some(0x80),
            0x4040..=0x4092 if self.sound_io_enabled => Some(self.audio.read(address)),
            0x6000..=0xdfff => Some(self.prg_ram[usize::from(address - 0x6000)]),
            0xe000..=0xffff => Some(self.bios_rom[usize::from(address - 0xe000)]),
            _ => None,
        }
    }

    fn cpu_write(&mut self, address: u16, value: u8) -> bool {
        match address {
            0x4020 => {
                self.timer_irq_reload = (self.timer_irq_reload & 0xff00) | u16::from(value);
                true
            }
            0x4021 => {
                self.timer_irq_reload = (self.timer_irq_reload & 0x00ff) | (u16::from(value) << 8);
                true
            }
            0x4022 => {
                if self.disk_io_enabled {
                    self.timer_irq_repeat = value & 0x01 != 0;
                    self.timer_irq_enabled = value & 0x02 != 0;
                    if self.timer_irq_enabled {
                        self.timer_irq_counter = self.timer_irq_reload;
                    } else {
                        self.timer_irq_pending = false;
                    }
                }
                true
            }
            0x4023 => {
                self.disk_io_enabled = value & 0x01 != 0;
                self.sound_io_enabled = value & 0x02 != 0;
                if !self.disk_io_enabled {
                    self.timer_irq_enabled = false;
                    self.timer_irq_pending = false;
                    self.reset_disk_io();
                }
                if !self.sound_io_enabled {
                    self.audio = FdsAudio::default();
                }
                true
            }
            0x4024 if self.disk_io_enabled => {
                self.write_data = value;
                self.disk_irq_pending = false;
                self.transfer_complete = false;
                true
            }
            0x4025 if self.disk_io_enabled => {
                self.motor_on = value & 0x01 != 0;
                self.transfer_reset = value & 0x02 != 0;
                self.read_mode = value & 0x04 != 0;
                self.mirroring = if value & 0x08 != 0 {
                    Mirroring::Horizontal
                } else {
                    Mirroring::Vertical
                };
                self.crc_control = value & 0x10 != 0;
                self.disk_ready = value & 0x40 != 0;
                self.disk_irq_enabled = value & 0x80 != 0;
                self.disk_irq_pending = false;
                true
            }
            0x4026 if self.disk_io_enabled => true,
            0x4024..=0x4026 => true,
            0x4040..=0x4092 => {
                if self.sound_io_enabled {
                    self.audio.write(address, value);
                }
                true
            }
            0x6000..=0xdfff => {
                self.prg_ram[usize::from(address - 0x6000)] = value;
                true
            }
            _ => false,
        }
    }

    fn ppu_read(&mut self, address: u16) -> Option<u8> {
        (address < 0x2000).then(|| self.chr_ram[usize::from(address)])
    }

    fn ppu_write(&mut self, address: u16, value: u8) -> bool {
        if address < 0x2000 {
            self.chr_ram[usize::from(address)] = value;
            true
        } else {
            false
        }
    }

    fn mirroring(&self) -> Option<Mirroring> {
        Some(self.mirroring)
    }

    fn irq_pending(&self) -> bool {
        self.timer_irq_pending || self.disk_irq_pending
    }

    fn clock_cpu(&mut self) {
        self.clock_timer_irq();
        self.clock_disk();
        self.audio.clock();
    }

    fn expansion_audio(&self) -> f32 {
        self.audio.output()
    }

    fn reset(&mut self) {
        self.timer_irq_enabled = false;
        self.timer_irq_pending = false;
        self.disk_io_enabled = false;
        self.sound_io_enabled = false;
        self.reset_disk_io();
        self.audio = FdsAudio::default();
    }

    fn prg_rom(&self) -> &[u8] {
        &self.bios_rom
    }

    fn chr(&self) -> &[u8] {
        &self.chr_ram
    }

    fn chr_is_writable(&self) -> bool {
        true
    }

    fn battery_ram(&self) -> Option<&[u8]> {
        Some(&self.battery_data)
    }

    fn load_battery_ram(&mut self, data: &[u8]) {
        if data.len() != self.battery_data.len() {
            return;
        }
        self.battery_data.copy_from_slice(data);
        for (side, &offset) in self.disk_sides.iter_mut().zip(&self.side_offsets) {
            let side_len = side.len();
            side.copy_from_slice(&data[offset..offset + side_len]);
        }
    }

    fn snapshot(&self) -> MapperSnapshot {
        MapperSnapshot::Fds(FdsSnapshot {
            prg_ram: self.prg_ram.clone(),
            chr_ram: self.chr_ram.clone(),
            disk_sides: self.disk_sides.clone(),
            current_side: self.current_side,
            disk_inserted: self.disk_inserted,
            mirroring: self.mirroring,
            timer_irq_reload: self.timer_irq_reload,
            timer_irq_counter: self.timer_irq_counter,
            timer_irq_enabled: self.timer_irq_enabled,
            timer_irq_repeat: self.timer_irq_repeat,
            timer_irq_pending: self.timer_irq_pending,
            disk_io_enabled: self.disk_io_enabled,
            sound_io_enabled: self.sound_io_enabled,
            motor_on: self.motor_on,
            transfer_reset: self.transfer_reset,
            read_mode: self.read_mode,
            crc_control: self.crc_control,
            disk_ready: self.disk_ready,
            disk_irq_enabled: self.disk_irq_enabled,
            disk_position: self.disk_position,
            transfer_delay: self.transfer_delay,
            disk_irq_pending: self.disk_irq_pending,
            transfer_complete: self.transfer_complete,
            read_data: self.read_data,
            write_data: self.write_data,
            gap_ended: self.gap_ended,
            scanning_disk: self.scanning_disk,
            end_of_head: self.end_of_head,
            audio: self.audio.clone(),
        })
    }

    fn restore_snapshot(&mut self, snapshot: &MapperSnapshot) -> bool {
        let MapperSnapshot::Fds(snapshot) = snapshot else {
            return false;
        };
        self.prg_ram.clone_from(&snapshot.prg_ram);
        self.chr_ram.clone_from(&snapshot.chr_ram);
        self.disk_sides.clone_from(&snapshot.disk_sides);
        (self.side_offsets, self.battery_data) = flatten_disk_sides(&self.disk_sides);
        self.current_side = snapshot.current_side;
        self.disk_inserted = snapshot.disk_inserted;
        self.mirroring = snapshot.mirroring;
        self.timer_irq_reload = snapshot.timer_irq_reload;
        self.timer_irq_counter = snapshot.timer_irq_counter;
        self.timer_irq_enabled = snapshot.timer_irq_enabled;
        self.timer_irq_repeat = snapshot.timer_irq_repeat;
        self.timer_irq_pending = snapshot.timer_irq_pending;
        self.disk_io_enabled = snapshot.disk_io_enabled;
        self.sound_io_enabled = snapshot.sound_io_enabled;
        self.motor_on = snapshot.motor_on;
        self.transfer_reset = snapshot.transfer_reset;
        self.read_mode = snapshot.read_mode;
        self.crc_control = snapshot.crc_control;
        self.disk_ready = snapshot.disk_ready;
        self.disk_irq_enabled = snapshot.disk_irq_enabled;
        self.disk_position = snapshot.disk_position;
        self.transfer_delay = snapshot.transfer_delay;
        self.disk_irq_pending = snapshot.disk_irq_pending;
        self.transfer_complete = snapshot.transfer_complete;
        self.read_data = snapshot.read_data;
        self.write_data = snapshot.write_data;
        self.gap_ended = snapshot.gap_ended;
        self.scanning_disk = snapshot.scanning_disk;
        self.end_of_head = snapshot.end_of_head;
        self.audio.clone_from(&snapshot.audio);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk_side() -> Vec<u8> {
        let mut side = vec![0; FDS_SIDE_SIZE];
        side[..56].copy_from_slice(&[&[1], &b"*NINTENDO-HVC*"[..], &[0; 41]].concat());
        side[56..58].copy_from_slice(&[2, 0]);
        side
    }

    fn mapper() -> Fds {
        Fds::new(&vec![0; 0x2000], &disk_side()).unwrap()
    }

    fn clock_until_transfer(mapper: &mut Fds) {
        for _ in 0..1_000_000 {
            mapper.clock_cpu();
            if mapper.transfer_complete {
                return;
            }
        }
        panic!("disk transfer did not complete");
    }

    #[test]
    fn expands_standard_blocks_with_physical_gaps_and_crc_slots() {
        let expanded = expand_disk_side(&disk_side());
        assert_eq!(expanded[INITIAL_GAP_BYTES], 0x80);
        assert_eq!(expanded[INITIAL_GAP_BYTES + 1], 1);
        let second_sync = INITIAL_GAP_BYTES + 1 + 56 + 2 + BLOCK_GAP_BYTES;
        assert_eq!(expanded[second_sync], 0x80);
        assert_eq!(expanded[second_sync + 1], 2);
    }

    #[test]
    fn control_register_scans_to_the_first_disk_info_byte() {
        let mut mapper = mapper();
        mapper.cpu_write(0x4023, 1);
        // Motor on, transfer running, read mode, disk-ready, disk IRQ.
        mapper.cpu_write(0x4025, 0xc5);
        clock_until_transfer(&mut mapper);
        assert_eq!(mapper.cpu_read(0x4031), Some(1));
    }

    #[test]
    fn transfer_status_is_independent_of_irq_enable() {
        let mut mapper = mapper();
        mapper.cpu_write(0x4023, 1);
        mapper.cpu_write(0x4025, 0x45);
        clock_until_transfer(&mut mapper);
        assert!(!mapper.irq_pending());
        assert_eq!(mapper.cpu_read(0x4030).unwrap() & 0x02, 0x02);
    }

    #[test]
    fn disk_starts_inserted_and_swap_requires_eject_then_insert() {
        let mut image = disk_side();
        image.extend_from_slice(&disk_side());
        let mut mapper = Fds::new(&vec![0; 0x2000], &image).unwrap();
        assert_eq!(mapper.cpu_read(0x4032).unwrap() & 1, 0);
        mapper.swap_disk();
        assert_eq!(mapper.cpu_read(0x4032).unwrap() & 1, 1);
        mapper.swap_disk();
        assert_eq!(mapper.current_side, 1);
        assert_eq!(mapper.cpu_read(0x4032).unwrap() & 1, 0);
    }

    #[test]
    fn rejects_bad_bios_and_truncated_images() {
        assert!(matches!(
            Fds::new(&[], &disk_side()),
            Err(CartridgeError::InvalidFdsBiosSize(0))
        ));
        assert!(matches!(
            Fds::new(&vec![0; 0x2000], b"FDS\x1a\x01"),
            Err(CartridgeError::InvalidFdsImage(_))
        ));
    }

    #[test]
    fn accepts_headered_images_and_uses_the_declared_side_count() {
        let mut image = vec![0; 16];
        image[..5].copy_from_slice(b"FDS\x1a\x01");
        image.extend_from_slice(&disk_side());
        let mapper = Fds::new(&vec![0; 0x2000], &image).unwrap();
        assert_eq!(mapper.disk_sides.len(), 1);
        assert_eq!(mapper.disk_sides[0][INITIAL_GAP_BYTES + 1], 1);
    }

    #[test]
    fn exposes_writable_disk_data_for_battery_persistence() {
        let mut mapper = mapper();
        let mut saved = mapper.battery_ram().unwrap().to_vec();
        saved[INITIAL_GAP_BYTES + 1] = 0x55;
        mapper.load_battery_ram(&saved);
        assert_eq!(mapper.disk_sides[0][INITIAL_GAP_BYTES + 1], 0x55);
        assert_eq!(mapper.battery_ram().unwrap(), saved);
    }

    #[test]
    fn wavetable_audio_registers_generate_expansion_output() {
        let mut mapper = mapper();
        mapper.cpu_write(0x4023, 0x02);
        mapper.cpu_write(0x4089, 0x80);
        for address in 0x4040..=0x407f {
            mapper.cpu_write(address, 0x3f);
        }
        mapper.cpu_write(0x4080, 0x9f);
        mapper.cpu_write(0x4089, 0x00);
        assert!(mapper.expansion_audio() > 0.0);
    }
}
