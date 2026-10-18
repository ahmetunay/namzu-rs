//! Native X11/XTEST worker. Always launched in an owned guest, with a deadline.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use namzu_computer::desktop::{Button, Reply, Request, SCREEN_BYTES, ScreenConfig, key_sym};
use std::io::{Read, Write};
use x11rb::{
    connection::Connection,
    protocol::{
        xproto::{
            BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, ConnectionExt as _, ImageFormat, ImageOrder,
            KEY_PRESS_EVENT, KEY_RELEASE_EVENT, MOTION_NOTIFY_EVENT,
        },
        xtest::ConnectionExt as _,
    },
    rust_connection::RustConnection,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn main() {
    let result = (|| -> Result<Reply> {
        let mut bytes = Vec::new();
        std::io::stdin().take(16385).read_to_end(&mut bytes)?;
        if bytes.len() > 16384 {
            return Err("input limit".into());
        }
        let request: Request = serde_json::from_slice(&bytes)?;
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let (c, n) = loop {
            match RustConnection::connect(Some(":99")) {
                Ok(v) => break v,
                Err(e) => {
                    if std::time::Instant::now() >= until {
                        return Err(e.into());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        };
        let s = &c.setup().roots[n];
        let config = ScreenConfig {
            width: s.width_in_pixels,
            height: s.height_in_pixels,
        };
        request.validate(&config)?;
        let root = s.root;
        if matches!(request, Request::Info) {
            let p = c.query_pointer(root)?.reply()?;
            return Ok(Reply::Info {
                width: config.width,
                height: config.height,
                pointer_x: p.root_x,
                pointer_y: p.root_y,
            });
        }
        if matches!(request, Request::Screenshot) {
            let format = c
                .setup()
                .pixmap_formats
                .iter()
                .find(|f| f.depth == s.root_depth)
                .ok_or("unsupported pixel format")?;
            let visual = s
                .allowed_depths
                .iter()
                .flat_map(|d| &d.visuals)
                .find(|v| v.visual_id == s.root_visual)
                .ok_or("visual missing")?;
            if s.root_depth != 24
                || ![24, 32].contains(&format.bits_per_pixel)
                || visual.red_mask != 0xff0000
                || visual.green_mask != 0xff00
                || visual.blue_mask != 0xff
            {
                return Err("unsupported visual".into());
            }
            let image = c
                .get_image(
                    ImageFormat::Z_PIXMAP,
                    root,
                    0,
                    0,
                    config.width,
                    config.height,
                    u32::MAX,
                )?
                .reply()?;
            let unit = usize::from(format.bits_per_pixel) / 8;
            let stride = (usize::from(config.width) * usize::from(format.bits_per_pixel))
                .div_ceil(usize::from(format.scanline_pad))
                * usize::from(format.scanline_pad)
                / 8;
            if image.data.len() != stride * usize::from(config.height) {
                return Err("invalid pixel data".into());
            }
            let mut rgb =
                Vec::with_capacity(usize::from(config.width) * usize::from(config.height) * 3);
            for row in image.data.chunks_exact(stride) {
                for pixel in row[..usize::from(config.width) * unit].chunks_exact(unit) {
                    let v = if c.setup().image_byte_order == ImageOrder::LSB_FIRST {
                        u32::from_le_bytes([
                            pixel[0],
                            pixel[1],
                            pixel[2],
                            if unit == 4 { pixel[3] } else { 0 },
                        ])
                    } else {
                        pixel.iter().fold(0u32, |v, b| (v << 8) | u32::from(*b))
                    };
                    rgb.extend([(v >> 16) as u8, (v >> 8) as u8, v as u8]);
                }
            }
            let mut png = Vec::new();
            {
                let mut encoder =
                    png::Encoder::new(&mut png, u32::from(config.width), u32::from(config.height));
                encoder.set_color(png::ColorType::Rgb);
                encoder.set_depth(png::BitDepth::Eight);
                encoder.write_header()?.write_image_data(&rgb)?;
            }
            if png.len() > SCREEN_BYTES {
                return Err("screenshot limit".into());
            }
            return Ok(Reply::Screenshot {
                width: config.width,
                height: config.height,
                png_base64: STANDARD.encode(png),
            });
        }
        c.xtest_get_version(2, 2)?.reply()?;
        let mapping = c
            .get_keyboard_mapping(
                c.setup().min_keycode,
                c.setup().max_keycode - c.setup().min_keycode + 1,
            )?
            .reply()?;
        let key = |sym: u32| -> Result<(u8, bool)> {
            for (i, row) in mapping
                .keysyms
                .chunks(usize::from(mapping.keysyms_per_keycode))
                .enumerate()
            {
                for (column, value) in row.iter().take(2).enumerate() {
                    if *value == sym {
                        return Ok((c.setup().min_keycode + i as u8, column == 1));
                    }
                }
            }
            Err("key not in guest layout".into())
        };
        let shift = key(key_sym("SHIFT").unwrap())?.0;
        let event = |kind: u8, detail: u8, x: i16, y: i16| -> Result<()> {
            c.xtest_fake_input(kind, detail, 0, root, x, y, 0)?
                .check()?;
            Ok(())
        };
        let stroke = |code: u8, shifted: bool| -> Result<()> {
            if shifted {
                event(KEY_PRESS_EVENT, shift, 0, 0)?;
            }
            event(KEY_PRESS_EVENT, code, 0, 0)?;
            event(KEY_RELEASE_EVENT, code, 0, 0)?;
            if shifted {
                event(KEY_RELEASE_EVENT, shift, 0, 0)?;
            }
            Ok(())
        };
        match request {
            Request::Move { x, y } => event(MOTION_NOTIFY_EVENT, 0, x, y)?,
            Request::Click { x, y, button } => {
                event(MOTION_NOTIFY_EVENT, 0, x, y)?;
                let button = match button {
                    Button::Left => 1,
                    Button::Middle => 2,
                    Button::Right => 3,
                };
                event(BUTTON_PRESS_EVENT, button, 0, 0)?;
                event(BUTTON_RELEASE_EVENT, button, 0, 0)?;
            }
            Request::Drag {
                from_x,
                from_y,
                to_x,
                to_y,
            } => {
                event(MOTION_NOTIFY_EVENT, 0, from_x, from_y)?;
                event(BUTTON_PRESS_EVENT, 1, 0, 0)?;
                for i in 1..=16 {
                    event(
                        MOTION_NOTIFY_EVENT,
                        0,
                        (i32::from(from_x) + (i32::from(to_x) - i32::from(from_x)) * i / 16) as i16,
                        (i32::from(from_y) + (i32::from(to_y) - i32::from(from_y)) * i / 16) as i16,
                    )?;
                }
                event(BUTTON_RELEASE_EVENT, 1, 0, 0)?;
            }
            Request::Scroll {
                horizontal,
                vertical,
            } => {
                for (value, negative, positive) in [(vertical, 4, 5), (horizontal, 6, 7)] {
                    for _ in 0..value.unsigned_abs() {
                        let b = if value < 0 { negative } else { positive };
                        event(BUTTON_PRESS_EVENT, b, 0, 0)?;
                        event(BUTTON_RELEASE_EVENT, b, 0, 0)?;
                    }
                }
            }
            Request::Type { text } => {
                let planned: Vec<_> = text
                    .bytes()
                    .map(|b| {
                        key(match b {
                            b'\n' => 0xff0d,
                            b'\t' => 0xff09,
                            _ => u32::from(b),
                        })
                    })
                    .collect::<Result<_>>()?;
                for (code, shifted) in planned {
                    stroke(code, shifted)?;
                }
            }
            Request::Key { keys } => {
                let mut plan = keys
                    .iter()
                    .map(|k| key(key_sym(k).ok_or("invalid key")?))
                    .collect::<Result<Vec<_>>>()?;
                let last = plan.pop().ok_or("missing key")?;
                let mut modifiers: Vec<u8> = plan.into_iter().map(|(code, _)| code).collect();
                if last.1 && !modifiers.contains(&shift) {
                    modifiers.push(shift);
                }
                for code in &modifiers {
                    event(KEY_PRESS_EVENT, *code, 0, 0)?;
                }
                stroke(last.0, false)?;
                for code in modifiers.into_iter().rev() {
                    event(KEY_RELEASE_EVENT, code, 0, 0)?;
                }
            }
            _ => unreachable!(),
        }
        c.get_input_focus()?.reply()?;
        c.flush()?;
        Ok(Reply::Applied)
    })()
    .map_err(|_| "guest desktop unavailable or input failed".to_string());
    let out = serde_json::to_vec(&result).unwrap();
    let _ = std::io::stdout().write_all(&out);
}
