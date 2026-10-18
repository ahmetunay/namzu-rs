//! Offline GUI oracle, installed only in the acceptance image.
use std::io::Write;
use x11rb::{
    COPY_DEPTH_FROM_PARENT, CURRENT_TIME,
    connection::Connection,
    protocol::{
        Event,
        xproto::{
            ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask, InputFocus,
            KeyButMask, WindowClass,
        },
    },
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (c, n) = x11rb::connect(Some(":99"))?;
    let root = c.setup().roots[n].root;
    let window = c.generate_id()?;
    c.create_window(
        COPY_DEPTH_FROM_PARENT,
        window,
        root,
        20,
        30,
        400,
        240,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new()
            .background_pixel(0x224466)
            .event_mask(
                EventMask::EXPOSURE
                    | EventMask::KEY_PRESS
                    | EventMask::BUTTON_PRESS
                    | EventMask::BUTTON_RELEASE
                    | EventMask::POINTER_MOTION,
            ),
    )?
    .check()?;
    c.map_window(window)?.check()?;
    c.set_input_focus(InputFocus::PARENT, window, CURRENT_TIME)?
        .check()?;
    c.get_input_focus()?.reply()?;
    c.flush()?;
    let mut out = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open("/workspace/gui-events.jsonl")?;
    std::fs::write("/workspace/gui-ready", b"ready")?;
    loop {
        let value = match c.wait_for_event()? {
            Event::KeyPress(e) => {
                let map = c.get_keyboard_mapping(e.detail, 1)?.reply()?;
                let col = usize::from(e.state.contains(KeyButMask::SHIFT));
                serde_json::json!({"event":"key","keysym":map.keysyms[col.min(map.keysyms.len()-1)],"state":u16::from(e.state)})
            }
            Event::ButtonPress(e) => {
                if e.detail == 1 {
                    c.change_window_attributes(
                        window,
                        &ChangeWindowAttributesAux::new().background_pixel(0xaa3322),
                    )?
                    .check()?;
                    c.clear_area(false, window, 0, 0, 0, 0)?.check()?;
                }
                serde_json::json!({"event":"button","detail":e.detail,"x":e.event_x,"y":e.event_y})
            }
            Event::ButtonRelease(e) => serde_json::json!({"event":"release","detail":e.detail}),
            Event::MotionNotify(e) => {
                serde_json::json!({"event":"motion","x":e.event_x,"y":e.event_y})
            }
            _ => continue,
        };
        writeln!(out, "{value}")?;
        out.flush()?;
        c.flush()?;
    }
}
