use std::fs;
use std::io::Write;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_FAMILY").unwrap_or_default() != "windows" {
        return;
    }

    let out_dir = std::env::var("OUT_DIR").unwrap();
    let ico_path = format!("{}/icon.ico", out_dir);

    let img = image::open("../../icon.png").expect("failed to read icon.png");
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let w = width as u16;
    let h = height as u16;

    let row_bytes = ((w as u32 * 24 + 31) / 32) * 4;
    let pixel_data_size = row_bytes * height;
    let and_mask_row = ((w as u32 + 31) / 32) * 4;
    let and_mask_size = and_mask_row * height;
    let dib_size = 40 + pixel_data_size + and_mask_size;

    let mut ico = Vec::new();
    ico.extend_from_slice(&0u16.to_le_bytes());
    ico.extend_from_slice(&1u16.to_le_bytes());
    ico.extend_from_slice(&1u16.to_le_bytes());
    ico.push(if w >= 256 { 0 } else { w as u8 });
    ico.push(if h >= 256 { 0 } else { h as u8 });
    ico.push(0);
    ico.push(0);
    ico.extend_from_slice(&1u16.to_le_bytes());
    ico.extend_from_slice(&24u16.to_le_bytes());
    ico.extend_from_slice(&(dib_size as u32).to_le_bytes());
    ico.extend_from_slice(&22u32.to_le_bytes());

    ico.extend_from_slice(&40u32.to_le_bytes());
    ico.extend_from_slice(&(w as i32).to_le_bytes());
    ico.extend_from_slice(&(h as i32 * 2).to_le_bytes());
    ico.extend_from_slice(&1u16.to_le_bytes());
    ico.extend_from_slice(&24u16.to_le_bytes());
    ico.extend_from_slice(&0u32.to_le_bytes());
    ico.extend_from_slice(&(pixel_data_size as u32 + and_mask_size as u32).to_le_bytes());
    ico.extend_from_slice(&0i32.to_le_bytes());
    ico.extend_from_slice(&0i32.to_le_bytes());
    ico.extend_from_slice(&0u32.to_le_bytes());
    ico.extend_from_slice(&0u32.to_le_bytes());

    let padding = vec![0u8; (row_bytes - w as u32 * 3) as usize];
    for y in (0..height).rev() {
        for x in 0..width {
            let p = rgba.get_pixel(x, y);
            ico.push(p[2]);
            ico.push(p[1]);
            ico.push(p[0]);
        }
        ico.extend_from_slice(&padding);
    }

    let and_pad = vec![0u8; and_mask_row as usize];
    for _y in (0..height).rev() {
        ico.extend_from_slice(&and_pad);
    }

    let mut f = fs::File::create(&ico_path).expect("failed to create icon.ico");
    f.write_all(&ico).expect("failed to write icon.ico");

    let mut res = winres::WindowsResource::new();
    res.set_icon(&ico_path);
    res.compile().expect("failed to compile Windows resources");
}
