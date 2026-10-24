//! Own an X11 selection and serve normal or incremental properties for real-client tests.
use std::time::{Duration, Instant};
use x11rb::{
    connection::Connection,
    protocol::{Event, xproto::*},
    wrapper::ConnectionExt as _,
};

struct Transfer {
    window: u32,
    property: u32,
    type_: u32,
    format: u8,
    data: Vec<u8>,
    offset: usize,
}
fn notify(connection: &impl Connection, request: &SelectionRequestEvent, property: u32) -> Result<(), Box<dyn std::error::Error>> {
    let event = SelectionNotifyEvent {
        response_type: SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: request.time,
        requestor: request.requestor,
        selection: request.selection,
        target: request.target,
        property,
    };
    connection.send_event(false, request.requestor, EventMask::NO_EVENT, event)?;
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let selection = args.first().ok_or("selection required")?;
    let data = std::fs::read(args.get(1).ok_or("content file required")?)?;
    let flag = |name: &str| args.iter().any(|arg| arg == name);
    let (connection, screen) = x11rb::connect(None)?;
    let root = connection.setup().roots.get(screen).ok_or("missing screen")?.root;
    let atom = |name: &str| -> Result<u32, Box<dyn std::error::Error>> { Ok(connection.intern_atom(false, name.as_bytes())?.reply()?.atom) };
    let selection = atom(selection)?;
    let targets = atom("TARGETS")?;
    let incr = atom("INCR")?;
    let mime = if let Some(index) = args.iter().position(|arg| arg == "--mime") {
        args.get(index + 1).ok_or("mime value required")?.as_str()
    } else if flag("--latin1") {
        "STRING"
    } else {
        "UTF8_STRING"
    };
    let mime = atom(mime)?;
    let window = connection.generate_id()?;
    connection.create_window(0, window, root, 0, 0, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new())?;
    connection.set_selection_owner(window, selection, 0u32)?;
    connection.flush()?;
    println!("owned");
    let mut transfers: Vec<Transfer> = Vec::new();
    let limit = Instant::now() + Duration::from_secs(90);
    while Instant::now() < limit {
        let Some(event) = connection.poll_for_event()? else {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        };
        match event {
            Event::SelectionRequest(request) => {
                let property = if request.property == 0 { request.target } else { request.property };
                let (type_, format, bytes) = if request.target == targets {
                    println!("targets");
                    if flag("--stall-targets") {
                        continue;
                    }
                    (u32::from(AtomEnum::ATOM), 32, vec![mime, targets].into_iter().flat_map(u32::to_ne_bytes).collect::<Vec<_>>())
                } else if request.target == mime {
                    println!("data");
                    if flag("--die-on-data") {
                        return Ok(());
                    }
                    if flag("--refuse") {
                        notify(&connection, &request, 0)?;
                        connection.flush()?;
                        continue;
                    }
                    (mime, if flag("--bad-format") { 16 } else { 8 }, data.clone())
                } else {
                    notify(&connection, &request, 0)?;
                    connection.flush()?;
                    continue;
                };
                let incremental = if request.target == targets { flag("--incr-targets") } else { flag("--incr") || flag("--stall-data") };
                if incremental {
                    connection.change_window_attributes(
                        request.requestor,
                        &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY),
                    )?;
                    connection.change_property32(PropMode::REPLACE, request.requestor, property, incr, &[bytes.len() as u32])?;
                    transfers.push(Transfer { window: request.requestor, property, type_, format, data: bytes, offset: 0 });
                } else {
                    connection.change_property(
                        PropMode::REPLACE,
                        request.requestor,
                        property,
                        type_,
                        format,
                        bytes.len() as u32 * 8 / u32::from(format),
                        &bytes,
                    )?;
                }
                notify(&connection, &request, property)?;
                connection.flush()?;
            }
            Event::PropertyNotify(event) if event.state == Property::DELETE => {
                if let Some(index) = transfers.iter().position(|transfer| transfer.window == event.window && transfer.property == event.atom) {
                    let transfer = transfers.get_mut(index).ok_or("missing transfer")?;
                    if flag("--stall-data") && transfer.type_ != u32::from(AtomEnum::ATOM) {
                        continue;
                    }
                    let end = (transfer.offset + 32768).min(transfer.data.len());
                    let bytes = transfer.data.get(transfer.offset..end).ok_or("chunk range")?;
                    connection.change_property(
                        PropMode::REPLACE,
                        transfer.window,
                        transfer.property,
                        transfer.type_,
                        transfer.format,
                        bytes.len() as u32 * 8 / u32::from(transfer.format),
                        bytes,
                    )?;
                    println!("chunk {}", bytes.len());
                    let done = bytes.is_empty();
                    transfer.offset = end;
                    if done {
                        transfers.remove(index);
                    }
                    connection.flush()?;
                }
            }
            Event::DestroyNotify(event) => transfers.retain(|transfer| transfer.window != event.window),
            _ => {}
        }
    }
    Ok(())
}
