use eframe::egui::ColorImage;
use nes_core::{FRAME_HEIGHT, FRAME_WIDTH};
use rayon::prelude::*;

use crate::settings::{CrtMask, CrtProfile};

pub const CRT_SCALE: usize = 3;
pub const CRT_WIDTH: usize = FRAME_WIDTH * CRT_SCALE;
pub const CRT_HEIGHT: usize = FRAME_HEIGHT * CRT_SCALE;
const INVALID_SOURCE: u32 = u32::MAX;
const GAMMA_LUT_SIZE: usize = 4096;

#[derive(Clone, Copy)]
pub struct CrtParameters {
    pub profile: CrtProfile,
    pub mask: CrtMask,
    pub scanline_strength: f32,
    pub mask_strength: f32,
    pub bloom_strength: f32,
    pub curvature: f32,
    pub halation_strength: f32,
    pub diffusion_strength: f32,
    pub convergence: f32,
}

impl Default for CrtParameters {
    fn default() -> Self {
        Self {
            profile: CrtProfile::Royale,
            mask: CrtMask::ApertureGrille,
            scanline_strength: 0.38,
            mask_strength: 0.32,
            bloom_strength: 0.22,
            curvature: 0.055,
            halation_strength: 0.18,
            diffusion_strength: 0.10,
            convergence: 0.12,
        }
    }
}

#[derive(Clone, Copy)]
struct Mapping {
    source: u32,
    vignette: u8,
}

/// CPU-side CRT presentation filter. The expensive barrel-distortion lookup is
/// cached; the per-frame pass performs only local color blending and modulation.
pub struct CrtRenderer {
    mapping: Vec<Mapping>,
    mapped_curvature: f32,
    rgb: Vec<u8>,
    linear: Vec<f32>,
    blurred: Vec<f32>,
    linear_lut: [f32; 256],
    gamma_lut: [u8; GAMMA_LUT_SIZE],
    beam_lut: [[f32; CRT_SCALE]; 256],
    beam_scanline_strength: f32,
}

impl Default for CrtRenderer {
    fn default() -> Self {
        // Initialize worker threads during application setup instead of on the
        // first filtered gameplay frame, where it would look like a hitch.
        let _ = rayon::ThreadPoolBuilder::new().build_global();
        let linear_lut = std::array::from_fn(|value| {
            let encoded = value as f32 / 255.0;
            if encoded <= 0.040_45 {
                encoded / 12.92
            } else {
                ((encoded + 0.055) / 1.055).powf(2.4)
            }
        });
        let gamma_lut = std::array::from_fn(|value| {
            let linear = value as f32 / (GAMMA_LUT_SIZE - 1) as f32;
            let encoded = if linear <= 0.003_130_8 {
                linear * 12.92
            } else {
                1.055 * linear.powf(1.0 / 2.4) - 0.055
            };
            (encoded * 255.0).round().clamp(0.0, 255.0) as u8
        });
        Self {
            mapping: Vec::new(),
            mapped_curvature: f32::NAN,
            rgb: vec![0; CRT_WIDTH * CRT_HEIGHT * 3],
            linear: vec![0.0; FRAME_WIDTH * FRAME_HEIGHT * 3],
            blurred: vec![0.0; FRAME_WIDTH * FRAME_HEIGHT * 3],
            linear_lut,
            gamma_lut,
            beam_lut: [[1.0; CRT_SCALE]; 256],
            beam_scanline_strength: f32::NAN,
        }
    }
}

impl CrtRenderer {
    pub fn render(&mut self, source: &[u8], parameters: CrtParameters) -> ColorImage {
        let effective_curvature = if parameters.profile == CrtProfile::Flat {
            0.0
        } else {
            parameters.curvature
        };
        if self.mapping.len() != CRT_WIDTH * CRT_HEIGHT
            || (self.mapped_curvature - effective_curvature).abs() > f32::EPSILON
        {
            self.rebuild_mapping(effective_curvature);
        }
        self.rgb.fill(0);
        if source.len() != FRAME_WIDTH * FRAME_HEIGHT * 3 {
            return ColorImage::from_rgb([CRT_WIDTH, CRT_HEIGHT], &self.rgb);
        }

        match parameters.profile {
            CrtProfile::Lightweight => self.render_lightweight(source, parameters),
            CrtProfile::Flat => self.render_royale(source, parameters, false),
            CrtProfile::Royale => self.render_royale(source, parameters, true),
        }
        ColorImage::from_rgb([CRT_WIDTH, CRT_HEIGHT], &self.rgb)
    }

    fn render_lightweight(&mut self, source: &[u8], parameters: CrtParameters) {
        let scanline_strength = parameters.scanline_strength.clamp(0.0, 1.0);
        let mask_strength = parameters.mask_strength.clamp(0.0, 1.0);
        let bloom_strength = parameters.bloom_strength.clamp(0.0, 1.0);
        for output_y in 0..CRT_HEIGHT {
            // A three-line beam profile gives every source row one bright
            // center and one dimmer gap without changing the picture height.
            let scanline = match output_y % CRT_SCALE {
                0 => 1.0 - scanline_strength * 0.18,
                1 => 1.0,
                _ => 1.0 - scanline_strength,
            };
            for output_x in 0..CRT_WIDTH {
                let output_pixel = output_y * CRT_WIDTH + output_x;
                let mapping = self.mapping[output_pixel];
                if mapping.source == INVALID_SOURCE {
                    continue;
                }
                let source_pixel = mapping.source as usize;
                let source_x = source_pixel % FRAME_WIDTH;
                let source_y = source_pixel / FRAME_WIDTH;
                let left = source_y * FRAME_WIDTH + source_x.saturating_sub(1);
                let right = source_y * FRAME_WIDTH + (source_x + 1).min(FRAME_WIDTH - 1);
                let above = source_y.saturating_sub(1) * FRAME_WIDTH + source_x;
                let below = (source_y + 1).min(FRAME_HEIGHT - 1) * FRAME_WIDTH + source_x;
                let vignette = f32::from(mapping.vignette) / 255.0;
                let output_offset = output_pixel * 3;

                for channel in 0..3 {
                    let center = f32::from(source[source_pixel * 3 + channel]);
                    // Horizontal low-pass approximates limited analog video
                    // bandwidth and removes the perfectly square emulator edge.
                    let softened = center * 0.76
                        + f32::from(source[left * 3 + channel]) * 0.12
                        + f32::from(source[right * 3 + channel]) * 0.12;
                    let glow = (f32::from(source[above * 3 + channel])
                        + f32::from(source[below * 3 + channel])
                        + f32::from(source[left * 3 + channel])
                        + f32::from(source[right * 3 + channel]))
                        * 0.25;
                    let mut value =
                        softened * (1.0 - bloom_strength * 0.16) + glow * bloom_strength * 0.16;
                    value += (glow - 96.0).max(0.0) * bloom_strength * 0.10;

                    // RGB phosphor triads. Brightening the active phosphor a
                    // little preserves average luminance as the others dim.
                    let mask = if output_x % 3 == channel {
                        1.0 + mask_strength * 0.10
                    } else {
                        1.0 - mask_strength * 0.32
                    };
                    value *= scanline * mask * vignette;
                    self.rgb[output_offset + channel] = value.clamp(0.0, 255.0) as u8;
                }
            }
        }
    }

    fn render_royale(&mut self, source: &[u8], parameters: CrtParameters, screen_geometry: bool) {
        self.prepare_linear_blur(source);
        self.prepare_beam_lut(parameters.scanline_strength);
        let mask_strength = parameters.mask_strength.clamp(0.0, 1.0);
        let bloom_strength = parameters.bloom_strength.clamp(0.0, 1.0);
        let halation = parameters.halation_strength.clamp(0.0, 1.0);
        let diffusion = parameters.diffusion_strength.clamp(0.0, 1.0);
        let convergence = parameters.convergence.clamp(0.0, 1.0);

        let mapping_table = &self.mapping;
        let linear = &self.linear;
        let blurred = &self.blurred;
        let beam_lut = &self.beam_lut;
        let gamma_lut = &self.gamma_lut;
        self.rgb
            .par_chunks_mut(CRT_WIDTH * 3)
            .enumerate()
            .for_each(|(output_y, output_row)| {
                for output_x in 0..CRT_WIDTH {
                    let output_pixel = output_y * CRT_WIDTH + output_x;
                    let mapping = mapping_table[output_pixel];
                    if mapping.source == INVALID_SOURCE {
                        continue;
                    }
                    let source_pixel = mapping.source as usize;
                    let source_x = source_pixel % FRAME_WIDTH;
                    let source_y = source_pixel / FRAME_WIDTH;
                    let left = source_y * FRAME_WIDTH + source_x.saturating_sub(1);
                    let right = source_y * FRAME_WIDTH + (source_x + 1).min(FRAME_WIDTH - 1);
                    let luminance = ((u16::from(source[source_pixel * 3]) * 54
                        + u16::from(source[source_pixel * 3 + 1]) * 183
                        + u16::from(source[source_pixel * 3 + 2]) * 19)
                        >> 8) as usize;
                    let beam = beam_lut[luminance][output_y % CRT_SCALE];
                    let vignette = if screen_geometry {
                        f32::from(mapping.vignette) / 255.0
                    } else {
                        1.0
                    };
                    let output_offset = output_x * 3;

                    for channel in 0..3 {
                        let convergence_pixel = match channel {
                            0 => left,
                            2 => right,
                            _ => source_pixel,
                        };
                        let center = linear[source_pixel * 3 + channel] * (1.0 - convergence)
                            + linear[convergence_pixel * 3 + channel] * convergence;
                        let softened = center * 0.68
                            + linear[left * 3 + channel] * 0.16
                            + linear[right * 3 + channel] * 0.16;
                        let glass = blurred[source_pixel * 3 + channel];
                        let mut value =
                            softened * (1.0 - diffusion * 0.22) + glass * diffusion * 0.22;
                        // Halation is broad light reflected under the faceplate;
                        // bloom is the narrower bright electron-beam flare.
                        value += glass * halation * 0.075;
                        value += (glass - 0.12).max(0.0) * bloom_strength * 0.11;
                        value *= mask_factor(
                            parameters.mask,
                            output_x,
                            output_y,
                            channel,
                            mask_strength,
                        ) * beam
                            * vignette;
                        let gamma_index =
                            (value.clamp(0.0, 1.0) * (GAMMA_LUT_SIZE - 1) as f32).round() as usize;
                        output_row[output_offset + channel] = gamma_lut[gamma_index];
                    }
                }
            });
    }

    fn prepare_linear_blur(&mut self, source: &[u8]) {
        let linear_lut = &self.linear_lut;
        self.linear
            .par_iter_mut()
            .zip(source.par_iter())
            .for_each(|(destination, &encoded)| *destination = linear_lut[encoded as usize]);
        let linear = &self.linear;
        self.blurred
            .par_chunks_mut(FRAME_WIDTH * 3)
            .enumerate()
            .for_each(|(y, output_row)| {
                let above = y.saturating_sub(1);
                let below = (y + 1).min(FRAME_HEIGHT - 1);
                for x in 0..FRAME_WIDTH {
                    let left = x.saturating_sub(1);
                    let right = (x + 1).min(FRAME_WIDTH - 1);
                    for channel in 0..3 {
                        let sample = |sample_x: usize, sample_y: usize| {
                            linear[(sample_y * FRAME_WIDTH + sample_x) * 3 + channel]
                        };
                        let value = sample(x, y) * 0.40
                            + (sample(left, y)
                                + sample(right, y)
                                + sample(x, above)
                                + sample(x, below))
                                * 0.11
                            + (sample(left, above)
                                + sample(right, above)
                                + sample(left, below)
                                + sample(right, below))
                                * 0.04;
                        output_row[x * 3 + channel] = value;
                    }
                }
            });
    }

    fn prepare_beam_lut(&mut self, scanline_strength: f32) {
        let scanline_strength = scanline_strength.clamp(0.0, 1.0);
        if (self.beam_scanline_strength - scanline_strength).abs() <= f32::EPSILON {
            return;
        }
        self.beam_scanline_strength = scanline_strength;
        for (luminance, row) in self.beam_lut.iter_mut().enumerate() {
            let normalized = luminance as f32 / 255.0;
            let width = 0.42 + normalized * 0.34;
            let edge = (-1.0 / (2.0 * width * width)).exp();
            row[0] = 1.0 - scanline_strength * (1.0 - edge * 0.92);
            row[1] = 1.0;
            row[2] = 1.0 - scanline_strength * (1.0 - edge);
        }
    }

    fn rebuild_mapping(&mut self, curvature: f32) {
        let curvature = curvature.clamp(0.0, 0.25);
        self.mapped_curvature = curvature;
        self.mapping.clear();
        self.mapping.reserve(CRT_WIDTH * CRT_HEIGHT);
        for output_y in 0..CRT_HEIGHT {
            let normal_y = ((output_y as f32 + 0.5) / CRT_HEIGHT as f32) * 2.0 - 1.0;
            for output_x in 0..CRT_WIDTH {
                let normal_x = ((output_x as f32 + 0.5) / CRT_WIDTH as f32) * 2.0 - 1.0;
                let radius_squared = normal_x * normal_x + normal_y * normal_y;
                let barrel = 1.0 + curvature * radius_squared;
                let mapped_x = normal_x * barrel;
                let mapped_y = normal_y * barrel;
                if mapped_x.abs() >= 1.0 || mapped_y.abs() >= 1.0 {
                    self.mapping.push(Mapping {
                        source: INVALID_SOURCE,
                        vignette: 0,
                    });
                    continue;
                }
                let source_x = (((mapped_x + 1.0) * 0.5) * FRAME_WIDTH as f32)
                    .floor()
                    .clamp(0.0, (FRAME_WIDTH - 1) as f32) as usize;
                let source_y = (((mapped_y + 1.0) * 0.5) * FRAME_HEIGHT as f32)
                    .floor()
                    .clamp(0.0, (FRAME_HEIGHT - 1) as f32) as usize;
                let edge = normal_x.abs().max(normal_y.abs());
                let vignette =
                    (1.0 - ((edge - 0.55) / 0.45).max(0.0).powi(2) * 0.34).clamp(0.0, 1.0);
                self.mapping.push(Mapping {
                    source: (source_y * FRAME_WIDTH + source_x) as u32,
                    vignette: (vignette * 255.0) as u8,
                });
            }
        }
    }
}

fn mask_factor(mask: CrtMask, x: usize, y: usize, channel: usize, strength: f32) -> f32 {
    let (phase, vertical) = match mask {
        CrtMask::ApertureGrille => (x % 3, 1.0),
        CrtMask::SlotMask => {
            let stagger = usize::from((y / 6).is_multiple_of(2));
            let vertical = if y % 6 == 5 {
                1.0 - strength * 0.28
            } else {
                1.0
            };
            ((x + stagger) % 3, vertical)
        }
        CrtMask::ShadowMask => {
            let stagger = (y / 3) % 2;
            let vertical = if y % 3 == 2 {
                1.0 - strength * 0.18
            } else {
                1.0
            };
            ((x + stagger) % 3, vertical)
        }
    };
    let phosphor = if phase == channel {
        1.0 + strength * 0.16
    } else {
        1.0 - strength * 0.38
    };
    phosphor * vertical
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_a_three_x_crt_raster_and_curved_black_corners() {
        let source = vec![255; FRAME_WIDTH * FRAME_HEIGHT * 3];
        let image = CrtRenderer::default().render(
            &source,
            CrtParameters {
                curvature: 0.08,
                ..Default::default()
            },
        );
        assert_eq!(image.size, [768, 720]);
        assert_eq!(image.pixels[0], eframe::egui::Color32::BLACK);
        assert_ne!(
            image.pixels[(CRT_HEIGHT / 2) * CRT_WIDTH + CRT_WIDTH / 2],
            eframe::egui::Color32::BLACK
        );
    }

    #[test]
    fn royale_mask_types_produce_distinct_output() {
        let source = vec![192; FRAME_WIDTH * FRAME_HEIGHT * 3];
        let mut renderer = CrtRenderer::default();
        let grille = renderer.render(&source, CrtParameters::default());
        let slot = renderer.render(
            &source,
            CrtParameters {
                mask: CrtMask::SlotMask,
                ..Default::default()
            },
        );
        assert_ne!(grille.pixels, slot.pixels);
    }

    #[test]
    fn flat_profile_has_no_curved_or_vignetted_edges() {
        let source = vec![255; FRAME_WIDTH * FRAME_HEIGHT * 3];
        let image = CrtRenderer::default().render(
            &source,
            CrtParameters {
                profile: CrtProfile::Flat,
                curvature: 0.16,
                ..Default::default()
            },
        );
        assert_ne!(image.pixels[0], eframe::egui::Color32::BLACK);
        assert_ne!(image.pixels[CRT_WIDTH - 1], eframe::egui::Color32::BLACK);
    }
}
