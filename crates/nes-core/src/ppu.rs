use std::collections::HashMap;

use crate::{
    Region,
    cartridge::{Cartridge, Mirroring},
};
use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;

pub const FRAME_WIDTH: usize = 256;
pub const FRAME_HEIGHT: usize = 240;

// PPU I/O-bus retention is analogue and varies between consoles. AccuracyCoin
// recommends a deterministic value between 5 and 30 frames; use the long end
// so ordinary register traffic retains its value while still decaying well
// before the test ROM's two-second timeout.
const PPU_OPEN_BUS_DECAY_CYCLES: u32 = 30 * 341 * 262;

fn restored_open_bus_decay() -> [u32; 8] {
    [PPU_OPEN_BUS_DECAY_CYCLES; 8]
}

/// The front end may replace this 64-color RGB888 lookup without changing
/// emulated PPU memory or timing. It is deliberately presentation-only state.
pub type OutputPalette = [[u8; 3]; 64];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PpuState {
    pub control: u8,
    pub mask: u8,
    pub status: u8,
    pub vram_address: u16,
    pub temp_address: u16,
    pub fine_x: u8,
    pub scroll_x: u8,
    pub scroll_y: u8,
    pub scanline: i16,
    pub dot: u16,
}

#[derive(Clone, Copy, Default)]
struct CachedBackgroundTile {
    pattern_lo: u8,
    pattern_hi: u8,
    attribute: u8,
}

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
struct EvaluatedSprite {
    bytes: [u8; 4],
    sprite_zero: bool,
    pattern_lo: u8,
    pattern_hi: u8,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Frame {
    /// RGB888 pixels, row major.
    pub pixels: Vec<u8>,
    pub number: u64,
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            pixels: vec![0; FRAME_WIDTH * FRAME_HEIGHT * 3],
            number: 0,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Ppu {
    #[serde(with = "BigArray")]
    nametable: [u8; 0x1000],
    palette: [u8; 32],
    #[serde(with = "BigArray")]
    oam: [u8; 256],
    control: u8,
    mask: u8,
    status: u8,
    oam_address: u8,
    #[serde(default)]
    oam_evaluation_start_address: u8,
    vram_address: u16,
    temp_address: u16,
    fine_x: u8,
    write_latch: bool,
    read_buffer: u8,
    open_bus: u8,
    #[serde(skip, default = "restored_open_bus_decay")]
    open_bus_decay: [u32; 8],
    scroll_x: u8,
    scroll_y: u8,
    line_origin_x: usize,
    line_origin_y: usize,
    #[serde(default)]
    line_origin_address: u16,
    #[serde(default)]
    background_pipeline_warmup: u8,
    scanline: i16,
    dot: u16,
    frame_complete: bool,
    nmi_pending: bool,
    #[serde(default)]
    nmi_output_active: bool,
    #[serde(default)]
    suppress_vblank: bool,
    odd_frame: bool,
    frame: Frame,
    #[serde(skip, default = "default_output_palette")]
    output_palette: OutputPalette,
    // Keep the canonical palette indices with snapshots. Reconstructing them
    // from RGB pixels is lossy when a palette has duplicate colors, and makes
    // Vs. System's scrambled RGB palette visibly corrupt for a frame on
    // rewind restore.
    #[serde(default)]
    frame_color_indices: Vec<u8>,
    // Pattern bytes live in PPU shift registers on hardware. Keeping a
    // scanline cache also ensures mapper latches see one fetch per tile rather
    // than one fetch per output pixel.
    #[serde(skip, default)]
    background_tiles: Vec<CachedBackgroundTile>,
    #[serde(default)]
    evaluated_sprites: Vec<EvaluatedSprite>,
    #[serde(default)]
    next_sprites: Vec<EvaluatedSprite>,
    #[serde(default)]
    next_sprites_valid: bool,
    #[serde(default)]
    active_sprites: Vec<EvaluatedSprite>,
    #[serde(default)]
    oam_corruption_pending: Option<u8>,
    #[serde(default)]
    sprite_overflow_pending: bool,
}

impl Default for Ppu {
    fn default() -> Self {
        Self {
            nametable: [0; 0x1000],
            palette: [0; 32],
            oam: [0; 256],
            control: 0,
            mask: 0,
            status: 0,
            oam_address: 0,
            oam_evaluation_start_address: 0,
            vram_address: 0,
            temp_address: 0,
            fine_x: 0,
            write_latch: false,
            read_buffer: 0,
            open_bus: 0,
            open_bus_decay: [0; 8],
            scroll_x: 0,
            scroll_y: 0,
            line_origin_x: 0,
            line_origin_y: 0,
            line_origin_address: 0,
            background_pipeline_warmup: 0,
            scanline: -1,
            dot: 0,
            frame_complete: false,
            nmi_pending: false,
            nmi_output_active: false,
            suppress_vblank: false,
            odd_frame: false,
            frame: Frame::default(),
            output_palette: default_output_palette(),
            frame_color_indices: vec![0; FRAME_WIDTH * FRAME_HEIGHT],
            background_tiles: Vec::new(),
            evaluated_sprites: Vec::new(),
            next_sprites: Vec::new(),
            next_sprites_valid: false,
            active_sprites: Vec::new(),
            oam_corruption_pending: None,
            sprite_overflow_pending: false,
        }
    }
}

impl Ppu {
    pub fn set_output_palette(&mut self, palette: OutputPalette) {
        if self.frame_color_indices.len() == FRAME_WIDTH * FRAME_HEIGHT {
            for (pixel, &index) in self
                .frame
                .pixels
                .chunks_exact_mut(3)
                .zip(&self.frame_color_indices)
            {
                pixel.copy_from_slice(&palette[index as usize & 0x3f]);
            }
        } else {
            // Older/deserialized snapshots do not contain the transient index
            // buffer. Recolor their already-rendered RGB frame as closely as
            // possible; the next rendered frame repopulates exact indices.
            let mut palette_indices = HashMap::with_capacity(self.output_palette.len());
            for (index, color) in self.output_palette.iter().copied().enumerate() {
                // Match the old linear search's first-index behavior when a
                // palette contains duplicate RGB colors.
                palette_indices.entry(color).or_insert(index as u8);
            }
            self.frame_color_indices.clear();
            self.frame_color_indices.reserve(FRAME_WIDTH * FRAME_HEIGHT);
            for pixel in self.frame.pixels.chunks_exact_mut(3) {
                let color = [pixel[0], pixel[1], pixel[2]];
                let index = palette_indices.get(&color).copied().unwrap_or_default() as usize;
                pixel.copy_from_slice(&palette[index]);
                self.frame_color_indices.push(index as u8);
            }
        }
        self.output_palette = palette;
    }

    pub fn output_palette(&self) -> OutputPalette {
        self.output_palette
    }

    pub(crate) fn canonicalize_output_for_snapshot(&mut self) {
        self.set_output_palette(NTSC_2C02_PALETTE);
    }

    pub(crate) fn prepare_default_output_after_snapshot_restore(&mut self) {
        // This transient buffer is skipped by serde. Rendering overwrites each
        // visible entry, so a zero-filled buffer is sufficient and avoids an
        // unnecessary full-frame palette reverse lookup on every rewind step.
        self.frame_color_indices
            .resize(FRAME_WIDTH * FRAME_HEIGHT, 0);
    }

    pub fn reset(&mut self) {
        self.control = 0;
        self.mask = 0;
        self.status &= 0x1f;
        self.oam_address = 0;
        self.oam_evaluation_start_address = 0;
        self.vram_address = 0;
        self.temp_address = 0;
        self.fine_x = 0;
        self.write_latch = false;
        self.line_origin_address = 0;
        self.background_pipeline_warmup = 0;
        self.scanline = -1;
        self.dot = 0;
        self.frame_complete = false;
        self.nmi_pending = false;
        self.nmi_output_active = false;
        self.suppress_vblank = false;
        self.evaluated_sprites.clear();
        self.next_sprites.clear();
        self.next_sprites_valid = false;
        self.active_sprites.clear();
        self.oam_corruption_pending = None;
        self.sprite_overflow_pending = false;
    }

    pub fn cpu_read(&mut self, register: u16, cartridge: &mut Cartridge) -> u8 {
        match register & 7 {
            2 => {
                // PPUSTATUS latches vblank near the beginning of the CPU read,
                // while the sprite flags remain live until its end. At the
                // pre-render boundary this exposes cleared sprite flags one
                // dot before the vblank bit appears clear.
                let sprite_flags = if self.scanline == -1 && self.dot == 1 {
                    0
                } else {
                    self.status & 0x60
                };
                let value = sprite_flags | (self.status & 0x80) | (self.open_bus & 0x1f);
                if self.scanline == 241 && self.dot <= 1 {
                    self.suppress_vblank = true;
                }
                self.status &= !0x80;
                self.nmi_output_active = false;
                self.write_latch = false;
                self.nmi_pending = false;
                self.update_open_bus(value, 0xe0);
                value
            }
            4 if self.rendering_oam_read_returns_ff() => {
                self.update_open_bus(0xff, 0xff);
                0xff
            }
            4 if self.rendering_oam_evaluation_read() => {
                let offset = ((self.dot - 65) / 2) as u8;
                let address = self.oam_evaluation_start_address.wrapping_add(offset);
                let value = self.oam[address as usize] & if address & 3 == 2 { 0xe3 } else { 0xff };
                self.update_open_bus(value, 0xff);
                value
            }
            4 => {
                // Sprite attribute bytes expose only palette, priority, and
                // flip bits. The three unused middle bits read as low.
                let value = self.oam[self.oam_address as usize]
                    & if self.oam_address & 3 == 2 {
                        0xe3
                    } else {
                        0xff
                    };
                self.update_open_bus(value, 0xff);
                value
            }
            7 => {
                let address = self.vram_address & 0x3fff;
                let fetched = self.read_memory(address, cartridge);
                let (value, driven_bits) = if address < 0x3f00 {
                    let old = self.read_buffer;
                    self.read_buffer = fetched;
                    (old, 0xff)
                } else {
                    self.read_buffer = self.read_memory(address.wrapping_sub(0x1000), cartridge);
                    let palette_value = if self.mask & 0x01 != 0 {
                        fetched & 0x30
                    } else {
                        fetched
                    };
                    (palette_value | (self.open_bus & 0xc0), 0x3f)
                };
                self.increment_vram_after_cpu_access();
                self.update_open_bus(value, driven_bits);
                value
            }
            _ => self.open_bus,
        }
    }

    pub fn cpu_write(&mut self, register: u16, value: u8, cartridge: &mut Cartridge) {
        self.update_open_bus(value, 0xff);
        match register & 7 {
            0 => {
                let nmi_was_off = self.control & 0x80 == 0;
                self.control = value;
                self.temp_address = (self.temp_address & !0x0c00) | (((value as u16) & 3) << 10);
                if nmi_was_off && value & 0x80 != 0 && self.nmi_output_active {
                    self.nmi_pending = true;
                } else if value & 0x80 == 0 {
                    self.nmi_pending = false;
                }
            }
            1 => {
                let was_rendering = self.mask & 0x18 != 0;
                let will_render = value & 0x18 != 0;
                if was_rendering && !will_render {
                    if self.scanline == -1 || (0..240).contains(&self.scanline) {
                        // Disabling the rendering pipeline leaves its current
                        // secondary-OAM address as the seed for the next
                        // rendering start. The transfer itself is deferred.
                        let row = ((self.dot.saturating_add(1) / 2) as u8).clamp(1, 31);
                        self.oam_corruption_pending = Some(row);
                    } else {
                        // Re-enabling and disabling again during blanking does
                        // not let a pending transfer reach an active scanline.
                        self.oam_corruption_pending = None;
                    }
                }
                if self.mask & 0x18 == 0 && value & 0x18 != 0 {
                    // A disabled rendering pipeline does not clock or refill
                    // its 16-bit background shift registers. It takes two
                    // tile fetch groups before newly fetched pixels reach the
                    // output after rendering is enabled in the picture area.
                    self.background_pipeline_warmup = 16;
                }
                self.mask = value;
            }
            3 => self.oam_address = value,
            4 => {
                if self.rendering_oam_write_blocked() {
                    // During rendering OAM is owned by sprite evaluation. CPU
                    // writes do not reach OAM. The internal row counter moves
                    // to the next aligned four-byte group.
                    self.oam_address = self.oam_address.wrapping_add(4) & 0xfc;
                } else {
                    self.oam[self.oam_address as usize] = value;
                    self.oam_address = self.oam_address.wrapping_add(1);
                }
            }
            5 => {
                if !self.write_latch {
                    self.scroll_x = value;
                    self.fine_x = value & 7;
                    self.temp_address = (self.temp_address & !0x001f) | ((value as u16) >> 3);
                } else {
                    self.scroll_y = value;
                    self.temp_address = (self.temp_address & !0x73e0)
                        | (((value as u16) & 0xf8) << 2)
                        | (((value as u16) & 7) << 12);
                }
                self.write_latch = !self.write_latch;
            }
            6 => {
                if !self.write_latch {
                    self.temp_address =
                        (self.temp_address & 0x00ff) | (((value as u16) & 0x3f) << 8);
                } else {
                    self.temp_address = (self.temp_address & 0xff00) | value as u16;
                    self.vram_address = self.temp_address;
                }
                self.write_latch = !self.write_latch;
            }
            7 => {
                self.write_memory(self.vram_address & 0x3fff, value, cartridge);
                self.increment_vram_after_cpu_access();
            }
            _ => {}
        }
    }

    pub fn write_oam_dma(&mut self, page: &[u8; 256]) {
        for value in page {
            self.oam[self.oam_address as usize] = *value;
            self.oam_address = self.oam_address.wrapping_add(1);
        }
    }

    pub fn clock(&mut self, cartridge: &mut Cartridge) {
        self.clock_for_region(cartridge, Region::Ntsc);
    }

    pub(crate) fn clock_for_region(&mut self, cartridge: &mut Cartridge, region: Region) {
        self.clock_open_bus_decay();
        let rendering = self.mask & 0x18 != 0;
        if self.dot == 0 && (self.scanline == -1 || (0..240).contains(&self.scanline)) {
            if rendering && let Some(row) = self.oam_corruption_pending.take() {
                let source: [u8; 8] = self.oam[..8].try_into().unwrap();
                let start = usize::from(row) * 8;
                self.oam[start..start + 8].copy_from_slice(&source);
            }
            if self.next_sprites_valid {
                self.active_sprites = std::mem::take(&mut self.next_sprites);
                self.next_sprites_valid = false;
            }
        }
        if rendering && self.scanline >= 0 && self.scanline < 240 && self.dot == 1 {
            self.capture_line_origin();
            if matches!(cartridge.mapper_id(), 9 | 10) {
                self.cache_scanline_patterns(cartridge);
            } else {
                self.background_tiles.clear();
            }
        }
        if self.scanline >= 0 && self.scanline < 240 && self.dot >= 1 && self.dot <= 256 {
            self.render_pixel(self.dot as usize - 1, self.scanline as usize, cartridge);
        }

        // The PPU's NMI output changes one dot before the readable vblank
        // flag. Keep this window separate so precisely timed $2000 writes
        // see the hardware boundary without moving $2002's tested timing.
        if self.scanline == 241 && self.dot == 0 {
            self.nmi_output_active = true;
            if self.control & 0x80 != 0 {
                self.nmi_pending = true;
            }
        } else if self.scanline == -1 && self.dot == 0 {
            self.nmi_output_active = false;
        }

        if self.scanline == -1 && self.dot == 1 {
            self.status &= !0xe0;
        } else if self.scanline == 241 && self.dot == 1 {
            if self.suppress_vblank {
                self.suppress_vblank = false;
                self.status &= !0x80;
                self.nmi_output_active = false;
                self.nmi_pending = false;
            } else {
                self.status |= 0x80;
            }
            self.frame_complete = true;
            self.frame.number = self.frame.number.wrapping_add(1);
        }

        if rendering && (self.scanline == -1 || (0..240).contains(&self.scanline)) {
            if self.background_pipeline_warmup != 0
                && ((1..=256).contains(&self.dot) || (321..=336).contains(&self.dot))
            {
                self.background_pipeline_warmup -= 1;
            }
            if (257..=320).contains(&self.dot) {
                // Sprite evaluation owns OAM during rendering. While the
                // sprite pattern fetches run, the hardware repeatedly forces
                // OAMADDR to zero so a following DMA starts at sprite zero.
                self.oam_address = 0;
            }
            if self.dot == 65 {
                self.evaluate_sprites_for_next_scanline();
            } else if self.dot == 130 && self.sprite_overflow_pending {
                self.status |= 0x20;
                self.sprite_overflow_pending = false;
            }
            if (8..=256).contains(&self.dot) && self.dot.is_multiple_of(8) {
                self.increment_coarse_x();
            }
            if self.dot == 256 {
                self.increment_render_y();
            } else if self.dot == 257 {
                self.copy_horizontal_scroll();
            } else if self.dot == 264 {
                self.fetch_evaluated_sprite_patterns(cartridge);
            }
            if self.scanline == -1 && (280..=304).contains(&self.dot) {
                self.copy_vertical_scroll();
            }
            if self.dot == 260 {
                cartridge.clock_scanline(self.scanline);
            }
        }

        self.dot += 1;
        if region == Region::Ntsc
            && self.scanline == -1
            && self.dot == 340
            && self.odd_frame
            && rendering
        {
            self.dot = 0;
            self.scanline = 0;
        } else if self.dot >= 341 {
            self.dot = 0;
            self.scanline += 1;
            if self.scanline >= region.ppu_scanlines() - 1 {
                self.scanline = -1;
                self.odd_frame = !self.odd_frame;
            }
        }
    }

    pub fn take_nmi(&mut self) -> bool {
        std::mem::take(&mut self.nmi_pending)
    }
    pub(crate) fn nmi_pending(&self) -> bool {
        self.nmi_pending
    }
    #[cfg(test)]
    pub(crate) fn force_nmi_for_test(&mut self) {
        self.nmi_pending = true;
    }
    pub fn take_frame_complete(&mut self) -> bool {
        std::mem::take(&mut self.frame_complete)
    }
    pub fn frame(&self) -> &Frame {
        &self.frame
    }
    pub fn state(&self) -> PpuState {
        PpuState {
            control: self.control,
            mask: self.mask,
            status: self.status,
            vram_address: self.vram_address,
            temp_address: self.temp_address,
            fine_x: self.fine_x,
            scroll_x: self.scroll_x,
            scroll_y: self.scroll_y,
            scanline: self.scanline,
            dot: self.dot,
        }
    }
    pub fn scanline(&self) -> i16 {
        self.scanline
    }
    pub fn dot(&self) -> u16 {
        self.dot
    }

    pub(crate) fn nametable_memory(&self) -> &[u8] {
        &self.nametable
    }

    pub(crate) fn palette_memory(&self) -> &[u8] {
        &self.palette
    }

    pub(crate) fn oam_memory(&self) -> &[u8] {
        &self.oam
    }

    pub(crate) fn debug_write_nametable(&mut self, offset: usize, value: u8) -> bool {
        self.nametable.get_mut(offset).is_some_and(|byte| {
            *byte = value;
            true
        })
    }

    pub(crate) fn debug_write_palette(&mut self, offset: usize, value: u8) -> bool {
        self.palette.get_mut(offset).is_some_and(|byte| {
            *byte = value & 0x3f;
            true
        })
    }

    pub(crate) fn debug_write_oam(&mut self, offset: usize, value: u8) -> bool {
        self.oam.get_mut(offset).is_some_and(|byte| {
            *byte = value;
            true
        })
    }

    fn increment_vram(&mut self) {
        self.vram_address =
            self.vram_address
                .wrapping_add(if self.control & 4 != 0 { 32 } else { 1 });
    }

    fn update_open_bus(&mut self, value: u8, driven_bits: u8) {
        self.open_bus = (self.open_bus & !driven_bits) | (value & driven_bits);
        for bit in 0..8 {
            let mask = 1 << bit;
            if driven_bits & mask != 0 {
                self.open_bus_decay[bit] = if value & mask != 0 {
                    PPU_OPEN_BUS_DECAY_CYCLES
                } else {
                    0
                };
            }
        }
    }

    fn clock_open_bus_decay(&mut self) {
        for (bit, remaining) in self.open_bus_decay.iter_mut().enumerate() {
            if *remaining == 0 {
                continue;
            }
            *remaining -= 1;
            if *remaining == 0 {
                self.open_bus &= !(1 << bit);
            }
        }
    }

    fn increment_vram_after_cpu_access(&mut self) {
        if self.mask & 0x18 != 0 && (self.scanline == -1 || (0..240).contains(&self.scanline)) {
            self.increment_coarse_x();
            self.increment_render_y();
        } else {
            self.increment_vram();
        }
    }

    fn rendering_oam_read_returns_ff(&self) -> bool {
        self.mask & 0x18 != 0
            && (self.scanline == -1 || (0..240).contains(&self.scanline))
            && ((1..=64).contains(&self.dot) || (257..=320).contains(&self.dot))
    }

    fn rendering_oam_write_blocked(&self) -> bool {
        self.mask & 0x18 != 0 && (self.scanline == -1 || (0..240).contains(&self.scanline))
    }

    fn rendering_oam_evaluation_read(&self) -> bool {
        self.mask & 0x18 != 0
            && (self.scanline == -1 || (0..240).contains(&self.scanline))
            && (65..=256).contains(&self.dot)
    }

    fn evaluate_sprites_for_next_scanline(&mut self) {
        let sprite_height = if self.control & 0x20 != 0 { 16 } else { 8 };
        let start_address = self.oam_address;
        self.oam_evaluation_start_address = start_address;
        let mut address = start_address;
        let mut inspected = 0u16;
        self.evaluated_sprites.clear();
        self.sprite_overflow_pending = false;

        while inspected < 64 && self.evaluated_sprites.len() < 8 {
            let y = self.oam[address as usize];
            if sprite_y_in_range(self.scanline, y, sprite_height) {
                let mut bytes = [0; 4];
                for byte in &mut bytes {
                    *byte = self.oam[address as usize];
                    address = address.wrapping_add(1);
                }
                self.evaluated_sprites.push(EvaluatedSprite {
                    bytes,
                    sprite_zero: inspected == 0,
                    ..EvaluatedSprite::default()
                });
                // The X byte is also fed through the range comparator. When
                // it misses, the next Y read is realigned down to that byte.
                if !sprite_y_in_range(self.scanline, bytes[3], sprite_height) {
                    address &= 0xfc;
                }
            } else {
                address = address.wrapping_add(4) & 0xfc;
            }
            inspected += 1;
            if address == start_address {
                break;
            }
        }

        // Once secondary OAM is full, the diagonal overflow scan advances
        // five bytes after a miss. A hit is enough to assert the flag.
        if self.evaluated_sprites.len() == 8 {
            while inspected < 64 {
                let candidate = self.oam[address as usize];
                if sprite_y_in_range(self.scanline, candidate, sprite_height) {
                    self.sprite_overflow_pending = true;
                    break;
                }
                address = address.wrapping_add(5);
                inspected += 1;
                if address == start_address {
                    break;
                }
            }
        }
        self.oam_address = address;
    }

    fn fetch_evaluated_sprite_patterns(&mut self, cartridge: &mut Cartridge) {
        let sprite_height = if self.control & 0x20 != 0 { 16 } else { 8 };
        let scanline = self.scanline as u8;
        self.next_sprites = self.evaluated_sprites.clone();
        for index in 0..self.next_sprites.len() {
            let sprite = self.next_sprites[index];
            let tile = sprite.bytes[1];
            let attributes = sprite.bytes[2];
            let mut row = usize::from(scanline.wrapping_sub(sprite.bytes[0]));
            if attributes & 0x80 != 0 {
                row = sprite_height - 1 - row.min(sprite_height - 1);
            }
            let (pattern_base, tile_number, tile_row) = if sprite_height == 16 {
                let table = (tile as u16 & 1) * 0x1000;
                let tile_number = (tile as u16 & 0xfe) + (row / 8) as u16;
                (table, tile_number, row & 7)
            } else {
                let table = if self.control & 0x08 != 0 { 0x1000 } else { 0 };
                (table, tile as u16, row & 7)
            };
            let address = pattern_base + tile_number * 16 + tile_row as u16;
            let pattern_lo = self.read_memory(address, cartridge);
            let pattern_hi = self.read_memory(address + 8, cartridge);
            self.next_sprites[index].pattern_lo = pattern_lo;
            self.next_sprites[index].pattern_hi = pattern_hi;
        }
        let dummy_address = if sprite_height == 16 {
            0x1fe0
        } else {
            (if self.control & 0x08 != 0 { 0x1000 } else { 0 }) + 0x0ff0
        };
        for _ in self.next_sprites.len()..8 {
            self.read_memory(dummy_address, cartridge);
            self.read_memory(dummy_address + 8, cartridge);
        }
        self.next_sprites_valid = true;
    }

    fn read_memory(&mut self, address: u16, cartridge: &mut Cartridge) -> u8 {
        let address = address & 0x3fff;
        match address {
            0x0000..=0x1fff => cartridge.ppu_read(address).unwrap_or(0),
            0x2000..=0x3eff => {
                if let Some(value) = cartridge.nametable_read(address) {
                    value
                } else {
                    let index = cartridge
                        .nametable_ciram_index(address)
                        .unwrap_or_else(|| mirror_nametable(address, cartridge.mirroring()));
                    self.nametable[index]
                }
            }
            _ => self.palette[mirror_palette(address)],
        }
    }

    fn write_memory(&mut self, address: u16, value: u8, cartridge: &mut Cartridge) {
        let address = address & 0x3fff;
        match address {
            0x0000..=0x1fff => {
                cartridge.ppu_write(address, value);
            }
            0x2000..=0x3eff => {
                if !cartridge.nametable_write(address, value) {
                    let index = cartridge
                        .nametable_ciram_index(address)
                        .unwrap_or_else(|| mirror_nametable(address, cartridge.mirroring()));
                    self.nametable[index] = value;
                }
            }
            _ => self.palette[mirror_palette(address)] = value & 0x3f,
        }
    }

    fn render_pixel(&mut self, x: usize, y: usize, cartridge: &mut Cartridge) {
        let universal = self.palette[0] & 0x3f;
        let mut color = universal;
        let mut background_pixel = 0;

        if self.mask & 0x08 != 0
            && self.background_pipeline_warmup == 0
            && (x >= 8 || self.mask & 0x02 != 0)
        {
            let pixel_x = self.fine_x as usize + x;
            let coarse_x_total = (self.line_origin_address & 0x001f) as usize + pixel_x / 8;
            let coarse_x = coarse_x_total & 0x1f;
            let nametable_x =
                ((self.line_origin_address >> 10) as usize & 1) ^ ((coarse_x_total / 32) & 1);
            let tile_address =
                (self.line_origin_address & !0x041f) | (nametable_x as u16) << 10 | coarse_x as u16;
            let name_addr = 0x2000 | (tile_address & 0x0fff);
            let attribute_addr = 0x23c0
                | (tile_address & 0x0c00)
                | ((tile_address >> 4) & 0x38)
                | ((tile_address >> 2) & 0x07);
            let row = (tile_address >> 12) & 7;
            let bit = 7 - (pixel_x & 7);
            let attribute_shift = (((tile_address >> 4) & 4) | (tile_address & 2)) as u8;
            let (pattern_lo, pattern_hi, attribute, bit) =
                if let Some(cached) = self.background_tiles.get(pixel_x / 8) {
                    (cached.pattern_lo, cached.pattern_hi, cached.attribute, bit)
                } else {
                    let tile = self.read_memory(name_addr, cartridge);
                    let attribute = self.read_memory(attribute_addr, cartridge);
                    let pattern_base = if self.control & 0x10 != 0 { 0x1000 } else { 0 };
                    (
                        self.read_memory(pattern_base + tile as u16 * 16 + row, cartridge),
                        self.read_memory(pattern_base + tile as u16 * 16 + row + 8, cartridge),
                        attribute,
                        bit,
                    )
                };
            background_pixel = ((pattern_lo >> bit) & 1) | (((pattern_hi >> bit) & 1) << 1);
            if background_pixel != 0 {
                let palette = (attribute >> attribute_shift) & 3;
                color =
                    self.palette[(palette as usize * 4 + background_pixel as usize) & 0x1f] & 0x3f;
            }
        }

        if self.mask & 0x10 != 0 && (x >= 8 || self.mask & 0x04 != 0) {
            for sprite in &self.active_sprites {
                let sprite_x = sprite.bytes[3] as usize;
                if x < sprite_x || x >= sprite_x + 8 {
                    continue;
                }
                let attributes = sprite.bytes[2];
                let mut column = x - sprite_x;
                if attributes & 0x40 != 0 {
                    column = 7 - column;
                }
                let bit = 7 - column;
                let sprite_pixel =
                    ((sprite.pattern_lo >> bit) & 1) | (((sprite.pattern_hi >> bit) & 1) << 1);
                if sprite_pixel == 0 {
                    continue;
                }

                if sprite.sprite_zero && background_pixel != 0 && x != 0 && x != 255 {
                    self.status |= 0x40;
                }
                let behind_background = attributes & 0x20 != 0;
                if background_pixel == 0 || !behind_background {
                    let palette = attributes & 3;
                    color =
                        self.palette[0x10 + palette as usize * 4 + sprite_pixel as usize] & 0x3f;
                }
                break;
            }
        }

        if self.mask & 0x01 != 0 {
            color &= 0x30;
        }
        let rgb = self.output_palette[color as usize];
        let offset = (y * FRAME_WIDTH + x) * 3;
        self.frame_color_indices[y * FRAME_WIDTH + x] = color;
        self.frame.pixels[offset..offset + 3].copy_from_slice(&rgb);
    }

    fn capture_line_origin(&mut self) {
        self.line_origin_address = self.vram_address;
        let coarse_x = (self.vram_address & 0x001f) as usize;
        let coarse_y = ((self.vram_address >> 5) & 0x001f) as usize;
        let nametable_x = ((self.vram_address >> 10) & 1) as usize;
        let nametable_y = ((self.vram_address >> 11) & 1) as usize;
        let fine_y = ((self.vram_address >> 12) & 7) as usize;
        self.line_origin_x = nametable_x * 256 + coarse_x * 8 + self.fine_x as usize;
        self.line_origin_y = nametable_y * 240 + coarse_y * 8 + fine_y;
    }

    fn cache_scanline_patterns(&mut self, cartridge: &mut Cartridge) {
        self.background_tiles.clear();
        self.background_tiles.reserve(34);
        let world_y = self.line_origin_y % 480;
        let first_tile_x = self.line_origin_x & !7;
        let pattern_base = if self.control & 0x10 != 0 { 0x1000 } else { 0 };
        for slot in 0..34 {
            let world_x = (first_tile_x + slot * 8) % 512;
            let table_x = world_x / 256;
            let table_y = world_y / 240;
            let local_x = world_x % 256;
            let local_y = world_y % 240;
            let table = table_y * 2 + table_x;
            let tile_x = local_x / 8;
            let tile_y = local_y / 8;
            let name_addr = 0x2000 + table as u16 * 0x400 + (tile_y * 32 + tile_x) as u16;
            let attribute_addr =
                0x23c0 + table as u16 * 0x400 + ((tile_y / 4) * 8 + tile_x / 4) as u16;
            let tile = self.read_memory(name_addr, cartridge);
            let attribute = self.read_memory(attribute_addr, cartridge);
            let row = (local_y & 7) as u16;
            let pattern_lo = self.read_memory(pattern_base + tile as u16 * 16 + row, cartridge);
            let pattern_hi = self.read_memory(pattern_base + tile as u16 * 16 + row + 8, cartridge);
            self.background_tiles.push(CachedBackgroundTile {
                pattern_lo,
                pattern_hi,
                attribute,
            });
        }
    }

    fn increment_coarse_x(&mut self) {
        if self.vram_address & 0x001f == 31 {
            self.vram_address &= !0x001f;
            self.vram_address ^= 0x0400;
        } else {
            self.vram_address = self.vram_address.wrapping_add(1);
        }
    }

    fn increment_render_y(&mut self) {
        if self.vram_address & 0x7000 != 0x7000 {
            self.vram_address += 0x1000;
            return;
        }
        self.vram_address &= !0x7000;
        let mut coarse_y = (self.vram_address & 0x03e0) >> 5;
        if coarse_y == 29 {
            coarse_y = 0;
            self.vram_address ^= 0x0800;
        } else if coarse_y == 31 {
            coarse_y = 0;
        } else {
            coarse_y += 1;
        }
        self.vram_address = (self.vram_address & !0x03e0) | (coarse_y << 5);
    }

    fn copy_horizontal_scroll(&mut self) {
        self.vram_address = (self.vram_address & !0x041f) | (self.temp_address & 0x041f);
    }

    fn copy_vertical_scroll(&mut self) {
        self.vram_address = (self.vram_address & !0x7be0) | (self.temp_address & 0x7be0);
    }
}

fn sprite_y_in_range(scanline: i16, y: u8, sprite_height: usize) -> bool {
    scanline >= i16::from(y) && scanline < i16::from(y) + sprite_height as i16
}

fn mirror_nametable(address: u16, mirroring: Mirroring) -> usize {
    let relative = (address - 0x2000) as usize & 0x0fff;
    let table = relative / 0x400;
    let offset = relative & 0x3ff;
    let physical = match mirroring {
        Mirroring::Vertical => [0, 1, 0, 1][table],
        Mirroring::Horizontal => [0, 0, 1, 1][table],
        Mirroring::FourScreen => table,
        Mirroring::SingleScreenLower => 0,
        Mirroring::SingleScreenUpper => 1,
    };
    physical * 0x400 + offset
}

fn mirror_palette(address: u16) -> usize {
    let mut index = (address as usize - 0x3f00) & 0x1f;
    if matches!(index, 0x10 | 0x14 | 0x18 | 0x1c) {
        index -= 0x10;
    }
    index
}

// Common 2C02 palette approximation. Palette output is a presentation detail;
// games select only the six-bit indices emulated above.
pub const NTSC_2C02_PALETTE: OutputPalette = [
    [84, 84, 84],
    [0, 30, 116],
    [8, 16, 144],
    [48, 0, 136],
    [68, 0, 100],
    [92, 0, 48],
    [84, 4, 0],
    [60, 24, 0],
    [32, 42, 0],
    [8, 58, 0],
    [0, 64, 0],
    [0, 60, 0],
    [0, 50, 60],
    [0, 0, 0],
    [0, 0, 0],
    [0, 0, 0],
    [152, 150, 152],
    [8, 76, 196],
    [48, 50, 236],
    [92, 30, 228],
    [136, 20, 176],
    [160, 20, 100],
    [152, 34, 32],
    [120, 60, 0],
    [84, 90, 0],
    [40, 114, 0],
    [8, 124, 0],
    [0, 118, 40],
    [0, 102, 120],
    [0, 0, 0],
    [0, 0, 0],
    [0, 0, 0],
    [236, 238, 236],
    [76, 154, 236],
    [120, 124, 236],
    [176, 98, 236],
    [228, 84, 236],
    [236, 88, 180],
    [236, 106, 100],
    [212, 136, 32],
    [160, 170, 0],
    [116, 196, 0],
    [76, 208, 32],
    [56, 204, 108],
    [56, 180, 204],
    [60, 60, 60],
    [0, 0, 0],
    [0, 0, 0],
    [236, 238, 236],
    [168, 204, 236],
    [188, 188, 236],
    [212, 178, 236],
    [236, 174, 236],
    [236, 174, 212],
    [236, 180, 176],
    [228, 196, 144],
    [204, 210, 120],
    [180, 222, 120],
    [168, 226, 144],
    [152, 226, 180],
    [160, 214, 228],
    [160, 162, 160],
    [0, 0, 0],
    [0, 0, 0],
];

const fn rgb_3bit(red: u8, green: u8, blue: u8) -> [u8; 3] {
    const fn expand(value: u8) -> u8 {
        ((value as u16 * 255 + 3) / 7) as u8
    }
    [expand(red), expand(green), expand(blue)]
}

/// RP2C03/RP2C05 RGB DAC palette used by PlayChoice-10 hardware.
///
/// The three-bit DAC codes come from the NESdev PPU palettes documentation.
/// This selects RGB output colors only; it does not pretend to change this
/// emulator's PPU register behavior or timing into another hardware variant.
pub const RGB_2C03_PALETTE: OutputPalette = [
    rgb_3bit(3, 3, 3),
    rgb_3bit(0, 1, 4),
    rgb_3bit(0, 0, 6),
    rgb_3bit(3, 2, 6),
    rgb_3bit(4, 0, 3),
    rgb_3bit(5, 0, 3),
    rgb_3bit(5, 1, 0),
    rgb_3bit(4, 2, 0),
    rgb_3bit(3, 2, 0),
    rgb_3bit(1, 2, 0),
    rgb_3bit(0, 3, 1),
    rgb_3bit(0, 4, 0),
    rgb_3bit(0, 2, 2),
    rgb_3bit(0, 0, 0),
    rgb_3bit(0, 0, 0),
    rgb_3bit(0, 0, 0),
    rgb_3bit(5, 5, 5),
    rgb_3bit(0, 3, 6),
    rgb_3bit(0, 2, 7),
    rgb_3bit(4, 0, 7),
    rgb_3bit(5, 0, 7),
    rgb_3bit(7, 0, 4),
    rgb_3bit(7, 0, 0),
    rgb_3bit(6, 3, 0),
    rgb_3bit(4, 3, 0),
    rgb_3bit(1, 4, 0),
    rgb_3bit(0, 4, 0),
    rgb_3bit(0, 5, 3),
    rgb_3bit(0, 4, 4),
    rgb_3bit(0, 0, 0),
    rgb_3bit(0, 0, 0),
    rgb_3bit(0, 0, 0),
    rgb_3bit(7, 7, 7),
    rgb_3bit(3, 5, 7),
    rgb_3bit(4, 4, 7),
    rgb_3bit(6, 3, 7),
    rgb_3bit(7, 0, 7),
    rgb_3bit(7, 3, 7),
    rgb_3bit(7, 4, 0),
    rgb_3bit(7, 5, 0),
    rgb_3bit(6, 6, 0),
    rgb_3bit(3, 6, 0),
    rgb_3bit(0, 7, 0),
    rgb_3bit(2, 7, 6),
    rgb_3bit(0, 7, 7),
    rgb_3bit(0, 0, 0),
    rgb_3bit(0, 0, 0),
    rgb_3bit(0, 0, 0),
    rgb_3bit(7, 7, 7),
    rgb_3bit(5, 6, 7),
    rgb_3bit(6, 5, 7),
    rgb_3bit(7, 5, 7),
    rgb_3bit(7, 4, 7),
    rgb_3bit(7, 5, 5),
    rgb_3bit(7, 6, 4),
    rgb_3bit(7, 7, 2),
    rgb_3bit(7, 7, 3),
    rgb_3bit(5, 7, 2),
    rgb_3bit(4, 7, 3),
    rgb_3bit(2, 7, 6),
    rgb_3bit(4, 6, 7),
    rgb_3bit(0, 0, 0),
    rgb_3bit(0, 0, 0),
    rgb_3bit(0, 0, 0),
];

/// RP2C04-0004 RGB palette used by Vs. Super Mario Bros. hardware.
///
/// Unlike a 2C02 palette approximation, these are the RGB PPU's physical
/// three-bit DAC values in the hardware's scrambled palette-index order.
pub const RGB_2C04_0004_PALETTE: OutputPalette = [
    rgb_3bit(4, 3, 0),
    rgb_3bit(3, 2, 6),
    rgb_3bit(0, 4, 4),
    rgb_3bit(6, 6, 0),
    rgb_3bit(0, 0, 0),
    rgb_3bit(7, 5, 5),
    rgb_3bit(0, 1, 4),
    rgb_3bit(6, 3, 0),
    rgb_3bit(5, 5, 5),
    rgb_3bit(3, 1, 0),
    rgb_3bit(0, 7, 0),
    rgb_3bit(0, 0, 3),
    rgb_3bit(7, 6, 4),
    rgb_3bit(7, 7, 0),
    rgb_3bit(0, 4, 0),
    rgb_3bit(5, 7, 2),
    rgb_3bit(7, 3, 7),
    rgb_3bit(2, 0, 0),
    rgb_3bit(0, 2, 7),
    rgb_3bit(7, 4, 7),
    rgb_3bit(0, 0, 0),
    rgb_3bit(2, 2, 2),
    rgb_3bit(5, 1, 0),
    rgb_3bit(7, 4, 0),
    rgb_3bit(6, 5, 3),
    rgb_3bit(0, 5, 3),
    rgb_3bit(4, 4, 7),
    rgb_3bit(1, 4, 0),
    rgb_3bit(4, 0, 3),
    rgb_3bit(0, 0, 0),
    rgb_3bit(4, 7, 3),
    rgb_3bit(3, 5, 7),
    rgb_3bit(5, 0, 3),
    rgb_3bit(0, 3, 1),
    rgb_3bit(4, 2, 0),
    rgb_3bit(0, 0, 6),
    rgb_3bit(4, 0, 7),
    rgb_3bit(5, 0, 7),
    rgb_3bit(3, 3, 3),
    rgb_3bit(7, 0, 4),
    rgb_3bit(0, 2, 2),
    rgb_3bit(6, 6, 6),
    rgb_3bit(0, 3, 6),
    rgb_3bit(0, 2, 0),
    rgb_3bit(1, 1, 1),
    rgb_3bit(7, 7, 3),
    rgb_3bit(4, 4, 4),
    rgb_3bit(7, 0, 7),
    rgb_3bit(7, 5, 7),
    rgb_3bit(7, 7, 7),
    rgb_3bit(3, 2, 0),
    rgb_3bit(7, 0, 0),
    rgb_3bit(7, 6, 0),
    rgb_3bit(2, 7, 6),
    rgb_3bit(7, 7, 7),
    rgb_3bit(4, 6, 7),
    rgb_3bit(0, 0, 0),
    rgb_3bit(7, 5, 0),
    rgb_3bit(6, 3, 7),
    rgb_3bit(5, 6, 7),
    rgb_3bit(3, 6, 0),
    rgb_3bit(6, 5, 7),
    rgb_3bit(0, 7, 7),
    rgb_3bit(1, 2, 0),
];

fn default_output_palette() -> OutputPalette {
    NTSC_2C02_PALETTE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cartridge() -> Cartridge {
        let mut rom = vec![0; 16 + 0x4000 + 0x2000];
        rom[0..4].copy_from_slice(b"NES\x1a");
        rom[4] = 1;
        rom[5] = 1;
        Cartridge::from_ines(&rom).unwrap()
    }

    fn chr_ram_cartridge() -> Cartridge {
        let mut rom = vec![0; 16 + 0x4000];
        rom[0..4].copy_from_slice(b"NES\x1a");
        rom[4] = 1;
        Cartridge::from_ines(&rom).unwrap()
    }

    fn mapper9_cartridge(chr: &[u8]) -> Cartridge {
        let mut rom = vec![0; 16 + 0x20_000 + 0x20_000];
        rom[0..4].copy_from_slice(b"NES\x1a");
        rom[4] = 8;
        rom[5] = 16;
        rom[6] = 0x90;
        rom[16 + 0x20_000..].copy_from_slice(chr);
        Cartridge::from_ines(&rom).unwrap()
    }

    #[test]
    fn mirrors_nametables() {
        assert_eq!(mirror_nametable(0x2800, Mirroring::Vertical), 0);
        assert_eq!(mirror_nametable(0x2400, Mirroring::Horizontal), 0);
        assert_eq!(mirror_nametable(0x2c00, Mirroring::Horizontal), 0x400);
    }

    #[test]
    fn mirrors_universal_sprite_palette_entries() {
        assert_eq!(mirror_palette(0x3f10), 0);
        assert_eq!(mirror_palette(0x3f24), 4);
    }

    #[test]
    fn rgb_2c03_palette_uses_documented_dac_values() {
        assert_eq!(RGB_2C03_PALETTE[0x00], [109, 109, 109]);
        assert_eq!(RGB_2C03_PALETTE[0x01], [0, 36, 146]);
        assert_eq!(RGB_2C03_PALETTE[0x2d], [0, 0, 0]);
        assert_eq!(RGB_2C03_PALETTE[0x3d], [0, 0, 0]);
    }

    #[test]
    fn rgb_2c04_0004_palette_uses_the_scrambled_hardware_order() {
        assert_eq!(RGB_2C04_0004_PALETTE[0x00], rgb_3bit(4, 3, 0));
        assert_eq!(RGB_2C04_0004_PALETTE[0x04], [0, 0, 0]);
        assert_eq!(RGB_2C04_0004_PALETTE[0x31], [255, 255, 255]);
    }

    #[test]
    fn palette_reads_preserve_the_ppu_io_latch_high_bits() {
        let mut ppu = Ppu::default();
        let mut cartridge = test_cartridge();
        ppu.debug_write_palette(0, 0x2a);
        ppu.cpu_write(6, 0x3f, &mut cartridge);
        ppu.cpu_write(6, 0x00, &mut cartridge);
        ppu.cpu_write(0, 0xc0, &mut cartridge);
        assert_eq!(ppu.cpu_read(7, &mut cartridge), 0xea);
    }

    #[test]
    fn ppu_open_bus_bits_decay_independently() {
        let mut ppu = Ppu::default();
        let mut cartridge = test_cartridge();
        ppu.cpu_write(2, 0x81, &mut cartridge);
        ppu.open_bus_decay[0] = 1;
        ppu.open_bus_decay[7] = 2;

        ppu.clock(&mut cartridge);
        assert_eq!(ppu.cpu_read(0, &mut cartridge), 0x80);
        ppu.clock(&mut cartridge);
        assert_eq!(ppu.cpu_read(0, &mut cartridge), 0x00);
    }

    #[test]
    fn grayscale_masks_palette_reads_but_not_writes() {
        let mut ppu = Ppu::default();
        let mut cartridge = test_cartridge();
        ppu.debug_write_palette(0x0c, 0x5a);
        ppu.vram_address = 0x3f1c;
        ppu.mask = 0x01;
        assert_eq!(ppu.cpu_read(7, &mut cartridge), 0x10);

        ppu.vram_address = 0x3f1d;
        ppu.cpu_write(7, 0x16, &mut cartridge);
        ppu.vram_address = 0x3f1d;
        ppu.mask = 0;
        assert_eq!(ppu.cpu_read(7, &mut cartridge), 0x16);
    }

    #[test]
    fn ppudata_access_during_rendering_increments_both_scroll_axes() {
        let mut ppu = Ppu::default();
        let mut cartridge = test_cartridge();
        ppu.mask = 0x18;
        ppu.scanline = 0;
        ppu.dot = 100;
        ppu.vram_address = 0x2000;
        ppu.cpu_read(7, &mut cartridge);
        assert_eq!(ppu.vram_address, 0x3001);
    }

    #[test]
    fn oamdata_writes_are_blocked_during_sprite_evaluation() {
        let mut ppu = Ppu::default();
        let mut cartridge = test_cartridge();
        ppu.mask = 0x10;
        ppu.scanline = 0;
        ppu.dot = 10;
        ppu.oam_address = 1;
        ppu.oam[1] = 0x12;
        ppu.cpu_write(4, 0xaa, &mut cartridge);
        assert_eq!(ppu.oam[1], 0x12);
        assert_eq!(ppu.oam_address, 4);
        assert_eq!(ppu.cpu_read(4, &mut cartridge), 0xff);
    }

    #[test]
    fn sprite_fetch_window_forces_oam_address_to_zero() {
        let mut ppu = Ppu::default();
        let mut cartridge = test_cartridge();
        ppu.mask = 0x08;
        ppu.scanline = 0;
        ppu.dot = 257;
        ppu.oam_address = 0x5d;

        ppu.clock(&mut cartridge);

        assert_eq!(ppu.oam_address, 0);
    }

    #[test]
    fn oamdata_masks_unused_sprite_attribute_bits_on_read() {
        let mut ppu = Ppu::default();
        let mut cartridge = test_cartridge();
        ppu.oam_address = 2;
        ppu.oam[2] = 0xff;

        assert_eq!(ppu.cpu_read(4, &mut cartridge), 0xe3);
    }

    #[test]
    fn status_read_on_the_vblank_edge_suppresses_vblank_and_nmi() {
        let mut ppu = Ppu::default();
        let mut cartridge = test_cartridge();
        ppu.control = 0x80;
        ppu.scanline = 241;
        ppu.dot = 0;
        ppu.cpu_read(2, &mut cartridge);
        ppu.clock(&mut cartridge);
        ppu.clock(&mut cartridge);
        assert_eq!(ppu.status & 0x80, 0);
        assert!(!ppu.take_nmi());
        assert!(ppu.take_frame_complete());
    }

    #[test]
    fn nmi_output_window_leads_the_readable_vblank_flag_by_one_dot() {
        let mut ppu = Ppu::default();
        let mut cartridge = test_cartridge();
        ppu.control = 0x80;
        ppu.scanline = 241;
        ppu.dot = 0;

        ppu.clock(&mut cartridge);

        assert!(ppu.nmi_output_active);
        assert_eq!(ppu.status & 0x80, 0);
        assert!(ppu.take_nmi());

        ppu.status |= 0x80;
        ppu.scanline = -1;
        ppu.dot = 0;
        ppu.clock(&mut cartridge);
        assert!(!ppu.nmi_output_active);
        assert_ne!(ppu.status & 0x80, 0);

        ppu.cpu_write(0, 0x00, &mut cartridge);
        ppu.cpu_write(0, 0x80, &mut cartridge);
        assert!(!ppu.take_nmi());
    }

    #[test]
    fn sprite_overflow_is_evaluated_during_the_sprite_window() {
        let mut ppu = Ppu::default();
        let mut cartridge = test_cartridge();
        ppu.mask = 0x10;
        ppu.scanline = 1;
        ppu.dot = 65;
        ppu.clock(&mut cartridge);
        assert_eq!(ppu.status & 0x20, 0);
        assert!(ppu.sprite_overflow_pending);
        while ppu.dot < 130 {
            ppu.clock(&mut cartridge);
        }
        assert_eq!(ppu.status & 0x20, 0);
        ppu.clock(&mut cartridge);
        assert_ne!(ppu.status & 0x20, 0);
    }

    #[test]
    fn status_read_sees_cleared_sprite_flags_before_vblank_clears() {
        let mut ppu = Ppu::default();
        let mut cartridge = test_cartridge();
        ppu.status = 0xe0;
        ppu.scanline = -1;
        ppu.dot = 1;

        assert_eq!(ppu.cpu_read(2, &mut cartridge) & 0xe0, 0x80);
    }

    #[test]
    fn grayscale_mask_limits_output_to_the_current_color_emphasis() {
        let mut ppu = Ppu::default();
        let mut cartridge = test_cartridge();
        ppu.palette[0] = 0x2f;
        ppu.mask = 0x01;
        ppu.scanline = 0;
        ppu.dot = 1;
        ppu.clock(&mut cartridge);
        assert_eq!(&ppu.frame.pixels[0..3], &ppu.output_palette[0x20]);
    }

    #[test]
    fn ppudata_chr_reads_have_the_hardware_one_byte_delay() {
        let mut ppu = Ppu::default();
        let mut cartridge = chr_ram_cartridge();
        assert!(cartridge.debug_write_chr(0, 0xab));
        ppu.vram_address = 0;
        assert_eq!(ppu.cpu_read(7, &mut cartridge), 0);
        assert_eq!(ppu.cpu_read(7, &mut cartridge), 0xab);
    }

    #[test]
    fn ppuctrl_nametable_bits_feed_both_scroll_transfer_axes() {
        let mut ppu = Ppu::default();
        let mut cartridge = test_cartridge();
        ppu.cpu_write(0, 0x03, &mut cartridge);
        ppu.vram_address = 0;
        ppu.copy_horizontal_scroll();
        assert_eq!(ppu.vram_address & 0x0400, 0x0400);
        ppu.copy_vertical_scroll();
        assert_eq!(ppu.vram_address & 0x0c00, 0x0c00);
    }

    #[test]
    fn sprite_zero_hit_starts_after_pixel_zero_and_requires_opaque_overlap() {
        let mut ppu = Ppu::default();
        let mut cartridge = chr_ram_cartridge();
        assert!(cartridge.debug_write_chr(0, 0xff));
        ppu.mask = 0x1e;
        ppu.palette[1] = 1;
        ppu.palette[0x11] = 2;
        ppu.oam[0..4].copy_from_slice(&[0, 0, 0, 0]);
        ppu.scanline = 0;
        ppu.evaluate_sprites_for_next_scanline();
        ppu.fetch_evaluated_sprite_patterns(&mut cartridge);
        ppu.active_sprites = std::mem::take(&mut ppu.next_sprites);
        ppu.scanline = 1;
        ppu.dot = 1;

        ppu.clock(&mut cartridge);
        assert_eq!(ppu.status & 0x40, 0);
        ppu.clock(&mut cartridge);
        assert_ne!(ppu.status & 0x40, 0);
    }

    #[test]
    fn background_pipeline_refills_before_drawing_after_midline_enable() {
        let mut ppu = Ppu::default();
        let mut cartridge = chr_ram_cartridge();
        assert!(cartridge.debug_write_chr(0, 0xff));
        ppu.palette[1] = 1;
        ppu.scanline = 0;
        ppu.dot = 100;
        ppu.cpu_write(1, 0x0a, &mut cartridge);

        for _ in 0..16 {
            ppu.clock(&mut cartridge);
        }
        assert_eq!(ppu.background_pipeline_warmup, 0);
        assert_eq!(ppu.frame_color_indices[115], 0);

        ppu.clock(&mut cartridge);
        assert_eq!(ppu.frame_color_indices[115], 1);
    }

    #[test]
    fn invalid_coarse_y_fetches_attribute_bytes_as_tile_ids() {
        let mut ppu = Ppu::default();
        let mut cartridge = chr_ram_cartridge();
        assert!(cartridge.debug_write_chr(0x0252, 0xff));
        ppu.nametable[mirror_nametable(0x2fc8, Mirroring::Horizontal)] = 0x25;
        ppu.palette[1] = 1;
        ppu.mask = 0x0a;
        ppu.vram_address = 0x2fc0;
        ppu.capture_line_origin();

        ppu.render_pixel(64, 0, &mut cartridge);

        assert_eq!(ppu.frame_color_indices[64], 1);
    }

    #[test]
    fn mapper_latch_tile_uses_one_cached_pattern_fetch_for_all_eight_pixels() {
        let mut chr = vec![0; 0x20_000];
        // Latch 1 starts in FE. Tile FD must be drawn from the old (FE) bank,
        // even though its high-plane fetch switches subsequent tiles to FD.
        chr[2 * 0x1000 + 0x0fd0] = 0xff;
        let mut cartridge = mapper9_cartridge(&chr);
        cartridge.cpu_write(0xd000, 1);
        cartridge.cpu_write(0xe000, 2);

        let mut ppu = Ppu::default();
        ppu.control = 0x10;
        ppu.mask = 0x0a;
        ppu.palette[1] = 1;
        ppu.nametable[0] = 0xfd;
        ppu.scanline = 0;
        ppu.dot = 1;
        for _ in 0..8 {
            ppu.clock(&mut cartridge);
        }

        assert_eq!(&ppu.frame_color_indices[..8], &[1; 8]);
    }

    #[test]
    fn mapper_latches_see_the_two_background_fetches_past_the_right_edge() {
        let mut chr = vec![0; 0x20_000];
        chr[1 * 0x1000] = 0x11;
        chr[2 * 0x1000] = 0x22;
        let mut cartridge = mapper9_cartridge(&chr);
        cartridge.cpu_write(0xd000, 1);
        cartridge.cpu_write(0xe000, 2);

        let mut ppu = Ppu::default();
        ppu.control = 0x10;
        ppu.mask = 0x08;
        // With vertical mirroring, slot 33 is the second tile in nametable 1.
        // It is outside the picture but is still fetched by the real PPU.
        ppu.nametable[0x401] = 0xfd;
        ppu.capture_line_origin();
        ppu.cache_scanline_patterns(&mut cartridge);

        assert_eq!(cartridge.ppu_read(0x1000), Some(0x11));
    }

    #[test]
    fn mapper_latch_sprite_keeps_the_old_bank_for_its_entire_row() {
        let mut chr = vec![0; 0x20_000];
        chr[2 * 0x1000 + 0x0fd0] = 0xff;
        let mut cartridge = mapper9_cartridge(&chr);
        cartridge.cpu_write(0xb000, 1);
        cartridge.cpu_write(0xc000, 2);

        let mut ppu = Ppu::default();
        ppu.mask = 0x14;
        ppu.palette[0x11] = 1;
        ppu.oam[0..4].copy_from_slice(&[0, 0xfd, 0, 0]);
        ppu.scanline = 0;
        ppu.evaluate_sprites_for_next_scanline();
        ppu.fetch_evaluated_sprite_patterns(&mut cartridge);
        ppu.active_sprites = std::mem::take(&mut ppu.next_sprites);
        for x in 0..8 {
            ppu.render_pixel(x, 1, &mut cartridge);
        }

        assert_eq!(
            &ppu.frame_color_indices[FRAME_WIDTH..FRAME_WIDTH + 8],
            &[1; 8]
        );
    }

    #[test]
    fn mapper_latches_see_unused_eight_by_sixteen_sprite_fetches() {
        let mut chr = vec![0; 0x20_000];
        chr[1 * 0x1000] = 0x11;
        chr[2 * 0x1000] = 0x22;
        let mut cartridge = mapper9_cartridge(&chr);
        cartridge.cpu_write(0xd000, 1);
        cartridge.cpu_write(0xe000, 2);
        cartridge.ppu_read(0x1fd8);

        let mut ppu = Ppu::default();
        ppu.control = 0x20;
        ppu.oam.fill(0xff);
        ppu.scanline = 0;
        ppu.evaluate_sprites_for_next_scanline();
        ppu.fetch_evaluated_sprite_patterns(&mut cartridge);

        assert_eq!(cartridge.ppu_read(0x1000), Some(0x22));
    }
}
