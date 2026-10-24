//! Read a seat selection through data control, including non-text and deliberately slow readers.
use smithay::reexports::wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::{self, ExtDataControlDeviceV1},
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_offer_v1::{self, ExtDataControlOfferV1},
};
use std::{
    fs::File,
    io::{Read, Write},
    os::fd::AsFd,
};
use wayland_client::{
    Connection, Dispatch, QueueHandle, delegate_noop, event_created_child,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_registry, wl_seat::WlSeat},
};
struct App {
    primary: bool,
    mime: String,
    offers: Vec<(ExtDataControlOfferV1, Vec<String>)>,
    selected: Option<ExtDataControlOfferV1>,
}
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for App {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
delegate_noop!(App: ignore WlSeat);
delegate_noop!(App: ignore ExtDataControlManagerV1);
impl Dispatch<ExtDataControlOfferV1, ()> for App {
    fn event(app: &mut Self, offer: &ExtDataControlOfferV1, event: ext_data_control_offer_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let ext_data_control_offer_v1::Event::Offer { mime_type } = event {
            if let Some((_, mimes)) = app.offers.iter_mut().find(|(candidate, _)| candidate == offer) {
                mimes.push(mime_type);
            } else {
                app.offers.push((offer.clone(), vec![mime_type]));
            }
        }
    }
}
impl Dispatch<ExtDataControlDeviceV1, ()> for App {
    event_created_child!(App, ExtDataControlDeviceV1, [0 => (ExtDataControlOfferV1, ())]);
    fn event(app: &mut Self, _: &ExtDataControlDeviceV1, event: ext_data_control_device_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            ext_data_control_device_v1::Event::Selection { id } if !app.primary => app.selected = id,
            ext_data_control_device_v1::Event::PrimarySelection { id } if app.primary => app.selected = id,
            _ => {}
        }
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let primary = arguments.first().ok_or("selection required")? == "PRIMARY";
    let mime = arguments.get(1).ok_or("mime required")?.clone();
    let connection = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<App>(&connection)?;
    let qh = queue.handle();
    let seat: WlSeat = globals.bind(&qh, 1..=8, ())?;
    let manager: ExtDataControlManagerV1 = globals.bind(&qh, 1..=1, ())?;
    let _device = manager.get_data_device(&seat, &qh, ());
    let mut app = App { primary, mime, offers: Vec::new(), selected: None };
    while app.selected.is_none() {
        queue.blocking_dispatch(&mut app)?;
    }
    let offer = app.selected.as_ref().ok_or("no selection")?;
    let mimes = app.offers.iter().find(|(candidate, _)| candidate == offer).map(|(_, mimes)| mimes).ok_or("no mime types")?;
    if arguments.iter().any(|argument| argument == "--offers") {
        for mime in mimes {
            println!("{mime}");
        }
        return Ok(());
    }
    if !mimes.contains(&app.mime) {
        return Err("mime not offered".into());
    }
    let (read, write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)?;
    offer.receive(app.mime, write.as_fd());
    drop(write);
    connection.flush()?;
    if arguments.iter().any(|argument| argument == "--abandon") {
        return Ok(());
    }
    if arguments.iter().any(|argument| argument == "--stall") {
        std::thread::sleep(std::time::Duration::from_secs(6));
    }
    let mut reader = File::from(read);
    let mut bytes = [0; 1024];
    loop {
        let count = reader.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        std::io::stdout().write_all(bytes.get(..count).ok_or("invalid read size")?)?;
        if arguments.iter().any(|argument| argument == "--slow") {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    Ok(())
}
