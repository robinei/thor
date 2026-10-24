//! Protocol advertisement retains high-precision formats; imports are tested at the renderer boundary.

use super::*;
use smithay::reexports::wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_feedback_v1;
pub(super) use smithay::reexports::wayland_protocols::wp::linux_dmabuf::zv1::client::{zwp_linux_buffer_params_v1, zwp_linux_dmabuf_v1};
use std::io::{Read, Seek};

impl Dispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, ()> for Client {
    fn event(state: &mut Self, _: &zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, event: zwp_linux_dmabuf_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let zwp_linux_dmabuf_v1::Event::Modifier { format, modifier_hi, modifier_lo } = event {
            state.dmabuf_formats.push((format, (u64::from(modifier_hi) << 32) | u64::from(modifier_lo)));
        }
    }
}
impl Dispatch<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        event: zwp_linux_buffer_params_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_linux_buffer_params_v1::Event::Created { buffer } => {
                state.imported_buffers += 1;
                buffer.destroy();
            }
            _ => panic!("unexpected import failure"),
        }
    }
    wayland_client::event_created_child!(Client, zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, [0 => (wl_buffer::WlBuffer, ())]);
}

impl Dispatch<zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
        event: zwp_linux_dmabuf_feedback_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_linux_dmabuf_feedback_v1::Event::MainDevice { device } => state.feedback_device = device,
            zwp_linux_dmabuf_feedback_v1::Event::FormatTable { fd, size } => {
                let mut file = std::fs::File::from(fd);
                file.rewind().unwrap();
                file.take(u64::from(size)).read_to_end(&mut state.feedback_table).unwrap();
            }
            zwp_linux_dmabuf_feedback_v1::Event::Done => state.feedback_done = true,
            _ => {}
        }
    }
}

#[test]
fn an_accepted_import_completes_once_and_an_empty_frame_has_no_import_work() {
    use smithay::backend::allocator::{Format, Fourcc, Modifier};
    let mut display = Display::<Server>::new().unwrap();
    let config =
        Config { bottom_output: "ClamShell-Bottom".into(), top_size: (960, 540), bottom_size: (620, 540), bottom_panel: false, touch_layout: Arc::default() };
    let mut server = Server::new(State::new(Init::default()), config, display.handle(), false, || {}, |_| Process::default(), |_| {}).unwrap();
    server.advertise_dmabuf_formats([Format { code: Fourcc::Xrgb8888, modifier: Modifier::Linear }], None);
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    display.handle().insert_client(server_socket, Arc::new(ClientState::default())).unwrap();
    let thread = std::thread::spawn(move || {
        let connection = Connection::from_socket(client_socket).unwrap();
        let (globals, mut queue) = registry_queue_init::<Client>(&connection).unwrap();
        let qh = queue.handle();
        let manager: zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 = globals.bind(&qh, 3..=3, ()).unwrap();
        let params = manager.create_params(&qh, ());
        let file = tempfile::tempfile().unwrap();
        file.set_len(4096).unwrap();
        params.add(file.as_fd(), 0, 0, 64, 0, 0);
        params.create(16, 16, Fourcc::Xrgb8888 as u32, zwp_linux_buffer_params_v1::Flags::empty());
        let mut client = Client::default();
        queue.roundtrip(&mut client).unwrap();
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(client.imported_buffers, 1);
    });
    let until = Instant::now() + Duration::from_secs(15);
    let mut imported = 0;
    while !thread.is_finished() && Instant::now() < until {
        display.dispatch_clients(&mut server).unwrap();
        // The renderer boundary is mocked here; no ordinary file is treated as a GPU buffer.
        let changed = server.complete_dmabuf_imports(|_| {
            imported += 1;
            Ok::<(), std::convert::Infallible>(())
        });
        if changed {
            assert_eq!(imported, 1);
        }
        display.flush_clients().unwrap();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(thread.is_finished());
    thread.join().unwrap();
    assert_eq!(imported, 1);
    assert!(!server.complete_dmabuf_imports(|_| -> Result<(), &str> { panic!("empty queue called importer") }));
}
