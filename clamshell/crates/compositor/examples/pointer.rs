//! Move the headless host's virtual pointer from coordinates on stdin.

use std::io::BufRead;
use wayland_client::{
    Connection, Dispatch, QueueHandle, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_pointer::ButtonState, wl_registry},
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{zwlr_virtual_pointer_manager_v1, zwlr_virtual_pointer_v1};

struct Client;
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Client {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
delegate_noop!(Client: ignore zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1);
delegate_noop!(Client: ignore zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let connection = Connection::connect_to_env()?;
    let (globals, queue) = registry_queue_init::<Client>(&connection)?;
    let qh = queue.handle();
    let manager: zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1 = globals.bind(&qh, 1..=2, ())?;
    let pointer = manager.create_virtual_pointer(None, &qh, ());
    let start = std::time::Instant::now();
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        let mut words = line.split_whitespace();
        let x: u32 = words.next().ok_or("missing x")?.parse()?;
        let y: u32 = words.next().ok_or("missing y")?.parse()?;
        pointer.motion_absolute(u32::try_from(start.elapsed().as_millis()).unwrap_or(u32::MAX), x, y, 1580, 540);
        pointer.frame();
        if words.next() == Some("click") {
            let time = u32::try_from(start.elapsed().as_millis()).unwrap_or(u32::MAX);
            pointer.button(time, 0x110, ButtonState::Pressed);
            pointer.frame();
            pointer.button(time.saturating_add(1), 0x110, ButtonState::Released);
            pointer.frame();
        }
        connection.flush()?;
    }
    Ok(())
}
