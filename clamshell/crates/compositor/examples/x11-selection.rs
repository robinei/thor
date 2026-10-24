//! Read an X11 selection, including INCR, without a toolkit or a window on screen.
use std::{
    io::Write,
    time::{Duration, Instant},
};
use x11rb::{
    connection::Connection,
    protocol::{Event, xproto::*},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let selection = arguments.first().ok_or("selection name required")?;
    let target = arguments.get(1).ok_or("target name required")?;
    let (connection, screen) = x11rb::connect(None)?;
    let root = connection.setup().roots.get(screen).ok_or("missing X11 screen")?.root;
    let window = connection.generate_id()?;
    connection.create_window(0, window, root, 0, 0, 1, 1, 0, WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE))?;
    let atom = |name: &str| -> Result<u32, Box<dyn std::error::Error>> { Ok(connection.intern_atom(false, name.as_bytes())?.reply()?.atom) };
    let (selection, target, property, incr) = (atom(selection)?, atom(target)?, atom("THOR_FIXTURE_SELECTION")?, atom("INCR")?);
    if arguments.iter().any(|argument| argument == "--take") {
        connection.set_selection_owner(window, selection, 0u32)?;
        connection.flush()?;
        std::thread::sleep(Duration::from_millis(200));
        if connection.get_selection_owner(selection)?.reply()?.owner != window {
            return Err("bridge reclaimed another client's selection".into());
        }
        println!("owned");
        return Ok(());
    }
    let limit = Instant::now() + Duration::from_secs(8);
    while connection.get_selection_owner(selection)?.reply()?.owner == 0 {
        if Instant::now() >= limit {
            return Err("no selection owner".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let requested = if arguments.iter().any(|arg| arg == "--no-property") { 0 } else { property };
    connection.convert_selection(window, selection, target, requested, 0u32)?;
    connection.flush()?;
    let mut incremental = false;
    let mut active_property = property;
    loop {
        if Instant::now() >= limit {
            return Err("selection timed out".into());
        }
        let Some(event) = connection.poll_for_event()? else {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        };
        let read = match event {
            Event::SelectionNotify(event) => {
                if event.property == 0 {
                    return Err("selection refused".into());
                }
                active_property = event.property;
                true
            }
            Event::PropertyNotify(event) => incremental && event.atom == active_property && event.state == Property::NEW_VALUE,
            _ => false,
        };
        if !read {
            continue;
        }
        let reply = connection.get_property(false, window, active_property, AtomEnum::ANY, 0, 65536)?.reply()?;
        if reply.bytes_after != 0 {
            return Err("fixture chunk exceeds limit".into());
        }
        if reply.type_ == incr {
            incremental = true;
            eprintln!("INCR");
            if arguments.iter().any(|arg| arg == "--abandon") {
                return Ok(());
            }
            if arguments.iter().any(|arg| arg == "--stall") {
                std::thread::sleep(Duration::from_secs(6));
            }
        } else {
            std::io::stdout().write_all(&reply.value)?;
            if !incremental || reply.value.is_empty() {
                break;
            }
        }
        if arguments.iter().any(|arg| arg == "--slow") {
            std::thread::sleep(Duration::from_millis(10));
        }
        connection.delete_property(window, active_property)?;
        connection.flush()?;
    }
    Ok(())
}
