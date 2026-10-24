//! A real X11 client without fonts: fill a window and honour resize and WM_DELETE_WINDOW.
use x11rb::{
    connection::Connection,
    protocol::{Event, xproto::*},
    wrapper::ConnectionExt as _,
};
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
            .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY | EventMask::KEY_PRESS | EventMask::KEY_RELEASE),
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
                _ => return Err("unknown fixture command".into()),
            }
            connection.flush()?;
        }
        while let Some(event) = connection.poll_for_event()? {
            match event {
                Event::Expose(_) => {
                    connection.clear_area(false, window, 0, 0, 0, 0)?;
                    connection.flush()?;
                }
                Event::ConfigureNotify(event) => println!("configured {} {}", event.width, event.height),
                Event::ClientMessage(event) if event.type_ == protocols && event.data.as_data32().first() == Some(&delete) => break 'client,
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
