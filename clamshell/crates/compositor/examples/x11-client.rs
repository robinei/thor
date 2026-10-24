//! A real X11 client without fonts: fill a window and honour resize and WM_DELETE_WINDOW.
use x11rb::{
    connection::Connection,
    protocol::{Event, xproto::*},
    wrapper::ConnectionExt as _,
};
fn popup(connection: &impl Connection, root: u32, parent: u32, tooltip: bool, parent_hint: bool) -> Result<u32, Box<dyn std::error::Error>> {
    let geometry = connection.get_geometry(parent)?.reply()?;
    let window = connection.generate_id()?;
    let nested = connection.get_window_attributes(parent)?.reply()?.override_redirect;
    let colour = if tooltip {
        0x606080
    } else if nested {
        0x204080
    } else {
        0x408020
    };
    let (x, y) = (geometry.x + 100, geometry.y + 80);
    connection.create_window(
        0,
        window,
        root,
        x,
        y,
        160,
        100,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new()
            .override_redirect(1)
            .background_pixel(colour)
            .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE),
    )?;
    let type_ = connection.intern_atom(false, b"_NET_WM_WINDOW_TYPE")?.reply()?.atom;
    let kind = connection.intern_atom(false, if tooltip { b"_NET_WM_WINDOW_TYPE_TOOLTIP" } else { b"_NET_WM_WINDOW_TYPE_POPUP_MENU" })?.reply()?.atom;
    connection.change_property32(PropMode::REPLACE, window, type_, AtomEnum::ATOM, &[kind])?;
    if parent_hint {
        connection.change_property32(PropMode::REPLACE, window, AtomEnum::WM_TRANSIENT_FOR, AtomEnum::WINDOW, &[parent])?;
    }
    connection.change_property8(PropMode::REPLACE, window, AtomEnum::WM_CLASS, AtomEnum::STRING, b"popup-fixture\0X11PopupFixture\0")?;
    if tooltip {
        use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
        connection.shape_rectangles(SO::SET, SK::INPUT, ClipOrdering::UNSORTED, window, 0, 0, &[])?;
    }
    connection.map_window(window)?;
    println!(
        "popup {} {window} {x} {y}",
        if tooltip {
            "tooltip"
        } else if nested {
            "submenu"
        } else {
            "menu"
        }
    );
    Ok(window)
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (connection, screen) = x11rb::connect(None)?;
    let screen = connection.setup().roots.get(screen).ok_or("missing X11 screen")?;
    let window = connection.generate_id()?;
    connection.create_window(
        0,
        window,
        screen.root,
        0,
        0,
        400,
        300,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new()
            .background_pixel(0x805020)
            .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY | EventMask::KEY_PRESS | EventMask::KEY_RELEASE | EventMask::BUTTON_PRESS),
    )?;
    let atom = |name: &[u8]| -> Result<u32, Box<dyn std::error::Error>> { Ok(connection.intern_atom(false, name)?.reply()?.atom) };
    let protocols = atom(b"WM_PROTOCOLS")?;
    let delete = atom(b"WM_DELETE_WINDOW")?;
    let utf8 = atom(b"UTF8_STRING")?;
    let name = atom(b"_NET_WM_NAME")?;
    connection.change_property8(PropMode::REPLACE, window, AtomEnum::WM_CLASS, AtomEnum::STRING, b"nested-x11\0NestedX11\0")?;
    connection.change_property8(PropMode::REPLACE, window, name, utf8, b"X11 fixture")?;
    connection.change_property32(PropMode::REPLACE, window, protocols, AtomEnum::ATOM, &[delete])?;
    let pid = atom(b"_NET_WM_PID")?;
    connection.change_property32(PropMode::REPLACE, window, pid, AtomEnum::CARDINAL, &[1])?;
    if std::env::args().any(|arg| arg == "--fixed") {
        let hints = atom(b"WM_NORMAL_HINTS")?;
        let type_ = atom(b"WM_SIZE_HINTS")?;
        connection.change_property32(PropMode::REPLACE, window, hints, type_, &[(1 << 4) | (1 << 5), 0, 0, 0, 0, 400, 300, 400, 300])?;
    }
    let helper = connection.generate_id()?;
    connection.create_window(0, helper, screen.root, 0, 0, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::default())?;
    connection.change_property8(PropMode::REPLACE, helper, AtomEnum::WM_CLASS, AtomEnum::STRING, b"hidden-helper\0HiddenX11Helper\0")?;
    connection.map_window(helper)?;
    if std::env::args().any(|arg| arg == "--transient") {
        let parent = connection.generate_id()?;
        connection.create_window(0, parent, screen.root, 0, 0, 400, 300, 0, WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::default())?;
        connection.change_property32(PropMode::REPLACE, window, AtomEnum::WM_TRANSIENT_FOR, AtomEnum::WINDOW, &[parent])?;
    }
    connection.map_window(window)?;
    connection.flush()?;
    let (commands, received) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::BufRead;
        for command in std::io::stdin().lock().lines().map_while(Result::ok) {
            if commands.send(command).is_err() {
                break;
            }
        }
    });
    let mut popups = Vec::new();
    'client: loop {
        for command in received.try_iter() {
            match command.as_str() {
                "unmap" => {
                    connection.unmap_window(window)?;
                }
                "map" => {
                    connection.map_window(window)?;
                }
                "title" => {
                    connection.change_property8(PropMode::REPLACE, window, name, utf8, b"X11 renamed")?;
                }
                "menu" => popups.push(popup(&connection, screen.root, window, false, true)?),
                "submenu" => {
                    let parent = popups.last().copied().ok_or("submenu needs a menu")?;
                    popups.push(popup(&connection, screen.root, parent, false, true)?);
                }
                "menu-no-parent" => popups.push(popup(&connection, screen.root, window, false, false)?),
                "unmap-menu" => {
                    if let Some(menu) = popups.first() {
                        connection.unmap_window(*menu)?;
                    }
                }
                "edge-menu" => {
                    let geometry = connection.get_geometry(window)?.reply()?;
                    let menu = popup(&connection, screen.root, window, false, true)?;
                    connection.configure_window(menu, &ConfigureWindowAux::new().x(i32::from(geometry.x) + i32::from(geometry.width) - 20))?;
                    popups.push(menu);
                }
                "tooltip" => popups.push(popup(&connection, screen.root, window, true, true)?),
                "hide-popups" => {
                    for popup in popups.drain(..) {
                        connection.destroy_window(popup)?;
                    }
                }
                _ => return Err("unknown fixture command".into()),
            }
            connection.flush()?;
        }
        while let Some(event) = connection.poll_for_event()? {
            match event {
                Event::Expose(event) => {
                    connection.clear_area(false, event.window, 0, 0, 0, 0)?;
                    connection.flush()?;
                }
                Event::ConfigureNotify(event) => println!("configured {} {}", event.width, event.height),
                Event::ClientMessage(event) if event.type_ == protocols && event.data.as_data32().first() == Some(&delete) => break 'client,
                Event::ButtonPress(event) => {
                    println!("{} button {} {}", if popups.contains(&event.event) { "popup" } else { "parent" }, event.event_x, event.event_y)
                }
                Event::UnmapNotify(event) if popups.contains(&event.window) => println!("popup unmapped {}", event.window),
                Event::KeyPress(event) => println!("key down {}", event.detail),
                Event::KeyRelease(event) => println!("key up {}", event.detail),
                _ => {}
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    connection.destroy_window(window)?;
    connection.flush()?;
    println!("closed");
    Ok(())
}
