#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
use crate::server::{ClientState, Process, Server};
use clamshell_state::{
    Init, State,
    session::presentation::{ColourDescription, ColourLuminance, ColourPrimaries, TransferFunction},
};
use clamshell_ui::Config;
use smithay::reexports::{
    wayland_protocols::wp::color_management::v1::client::{
        wp_color_management_surface_feedback_v1::{self as feedback, WpColorManagementSurfaceFeedbackV1},
        wp_color_management_surface_v1::WpColorManagementSurfaceV1,
        wp_color_manager_v1::{self as manager, WpColorManagerV1},
        wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1,
        wp_image_description_info_v1::{self as info, WpImageDescriptionInfoV1},
        wp_image_description_v1::{self as image, WpImageDescriptionV1},
    },
    wayland_server::Display,
};
use std::{
    os::unix::net::UnixStream,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use wayland_client::{
    Connection, Dispatch, QueueHandle, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_compositor, wl_output, wl_registry, wl_surface},
};
#[derive(Default)]
struct Client {
    ready: Vec<u32>,
    failed: usize,
    features: Vec<u32>,
    preferred: u32,
    information: bool,
    /// Each description's information: its luminances (max, reference).
    luminances: Vec<(u32, u32)>,
}
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Client {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WpColorManagerV1, ()> for Client {
    fn event(state: &mut Self, _: &WpColorManagerV1, event: manager::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let manager::Event::SupportedFeature { feature } = event {
            state.features.push(feature.into());
        }
    }
}
impl Dispatch<WpImageDescriptionV1, ()> for Client {
    fn event(state: &mut Self, _: &WpImageDescriptionV1, event: image::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            image::Event::Ready { identity } => state.ready.push(identity),
            image::Event::Failed { .. } => state.failed += 1,
            _ => {}
        }
    }
}
impl Dispatch<WpColorManagementSurfaceFeedbackV1, ()> for Client {
    fn event(state: &mut Self, _: &WpColorManagementSurfaceFeedbackV1, event: feedback::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let feedback::Event::PreferredChanged { identity } = event {
            state.preferred = identity;
        }
    }
}
impl Dispatch<WpImageDescriptionInfoV1, ()> for Client {
    fn event(state: &mut Self, _: &WpImageDescriptionInfoV1, event: info::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            info::Event::Done => state.information = true,
            info::Event::Luminances { max_lum, reference_lum, .. } => state.luminances.push((max_lum, reference_lum)),
            _ => {}
        }
    }
}
delegate_noop!(Client:ignore wl_compositor::WlCompositor);
delegate_noop!(Client:ignore wl_surface::WlSurface);
delegate_noop!(Client:ignore WpColorManagementSurfaceV1);
delegate_noop!(Client:ignore WpImageDescriptionCreatorParamsV1);
delegate_noop!(Client:ignore wl_output::WlOutput);
delegate_noop!(Client:ignore smithay::reexports::wayland_protocols::wp::color_management::v1::client::wp_color_management_output_v1::WpColorManagementOutputV1);
fn server() -> (Display<Server>, Server) {
    let display = Display::new().unwrap();
    let config = Config { top_size: (960, 540), bottom_size: (620, 540), touch_layout: Arc::default() };
    let server = Server::new(State::new(Init::default()), config, display.handle(), false, || {}, |_| Process::default(), |_| {}).unwrap();
    super::protocol::install(&display.handle());
    (display, server)
}
#[test]
fn parametric_descriptions_are_double_buffered_and_preferred_information_is_available() {
    let (mut display, mut server) = server();
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    display.handle().insert_client(server_socket, Arc::new(ClientState::default())).unwrap();
    let (tx, rx) = mpsc::channel();
    let (ack, wait) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let connection = Connection::from_socket(client_socket).unwrap();
        let (globals, mut queue) = registry_queue_init::<Client>(&connection).unwrap();
        let qh = queue.handle();
        let manager: WpColorManagerV1 = globals.bind(&qh, 1..=1, ()).unwrap();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
        let surface = compositor.create_surface(&qh, ());
        let control = manager.get_surface(&surface, &qh, ());
        let feedback = manager.get_surface_feedback(&surface, &qh, ());
        let mut client = Client::default();
        queue.roundtrip(&mut client).unwrap();
        assert!(client.features.contains(&(manager::Feature::Parametric as u32)));
        assert!(!client.features.contains(&(manager::Feature::IccV2V4 as u32)));
        let preferred = feedback.get_preferred_parametric(&qh, ());
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(client.ready.last().copied(), Some(client.preferred));
        preferred.get_information(&qh, ());
        queue.roundtrip(&mut client).unwrap();
        assert!(client.information);
        let creator = manager.create_parametric_creator(&qh, ());
        creator.set_primaries_named(manager::Primaries::Bt2020);
        creator.set_tf_named(manager::TransferFunction::St2084Pq);
        creator.set_luminances(0, 1000, 203);
        let description = creator.create(&qh, ());
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(client.ready.len(), 2);
        control.set_image_description(&description, manager::RenderIntent::Perceptual);
        queue.roundtrip(&mut client).unwrap();
        tx.send((0, false)).unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        tx.send((1, true)).unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        control.unset_image_description();
        queue.roundtrip(&mut client).unwrap();
        tx.send((2, true)).unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        surface.commit();
        queue.roundtrip(&mut client).unwrap();
        tx.send((3, false)).unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        let unsupported = manager.create_parametric_creator(&qh, ());
        unsupported.set_primaries(640000, 330000, 300000, 600000, 150000, 60000, 345700, 358500);
        unsupported.set_tf_named(manager::TransferFunction::Srgb);
        unsupported.create(&qh, ());
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(client.failed, 1);
        surface.destroy();
        queue.roundtrip(&mut client).unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut stages = Vec::new();
    while !thread.is_finished() && Instant::now() < deadline {
        display.dispatch_clients(&mut server).unwrap();
        server.turn();
        server.dh.flush_clients().unwrap();
        if let Ok((stage, present)) = rx.try_recv() {
            assert_eq!(!server.colour.surfaces.is_empty(), present);
            if present {
                assert_eq!(
                    server.colour.surfaces.values().next().map(|colour| **colour),
                    Some(ColourDescription {
                        primaries: ColourPrimaries::Bt2020,
                        transfer: TransferFunction::Pq,
                        luminance: Some(ColourLuminance { min: 0, max: 10000, reference: 203 }),
                        target: None
                    })
                );
            }
            stages.push(stage);
            ack.send(()).unwrap();
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(thread.is_finished());
    thread.join().unwrap();
    assert_eq!(stages, [0, 1, 2, 3]);
}
#[test]
fn an_invalid_colour_request_disconnects_its_client_without_ending_the_server() {
    let (mut display, mut server) = server();
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    display.handle().insert_client(server_socket, Arc::new(ClientState::default())).unwrap();
    let thread = std::thread::spawn(move || {
        let connection = Connection::from_socket(client_socket).unwrap();
        let (globals, mut queue) = registry_queue_init::<Client>(&connection).unwrap();
        let qh = queue.handle();
        let manager: WpColorManagerV1 = globals.bind(&qh, 1..=1, ()).unwrap();
        let creator = manager.create_parametric_creator(&qh, ());
        creator.set_tf_named(manager::TransferFunction::Srgb);
        creator.set_tf_power(22000);
        assert!(queue.roundtrip(&mut Client::default()).is_err());
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while !thread.is_finished() && Instant::now() < deadline {
        display.dispatch_clients(&mut server).unwrap();
        server.turn();
        server.dh.flush_clients().unwrap();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(thread.is_finished());
    thread.join().unwrap();
    assert!(!server.exit);
    assert!(server.colour.surfaces.is_empty());
}

#[test]
fn each_screen_is_described_as_its_panel_and_the_top_is_preferred() {
    let (mut display, mut server) = server();
    let profile = clamshell_device::Profile::vm_hdr();
    let panels = [ColourDescription::from(profile.top.colour), ColourDescription::from(profile.bottom.colour)];
    server.colour.set_outputs(panels);
    assert_ne!(server.colour.identities[0], server.colour.identities[1], "different descriptions, different identities");
    let (server_socket, client_socket) = UnixStream::pair().unwrap();
    display.handle().insert_client(server_socket, Arc::new(ClientState::default())).unwrap();
    let thread = std::thread::spawn(move || {
        let connection = Connection::from_socket(client_socket).unwrap();
        let (globals, mut queue) = registry_queue_init::<Client>(&connection).unwrap();
        let qh = queue.handle();
        let manager: WpColorManagerV1 = globals.bind(&qh, 1..=1, ()).unwrap();
        let outputs: Vec<wl_output::WlOutput> = globals
            .contents()
            .with_list(|list| list.iter().filter(|g| g.interface == "wl_output").map(|g| globals.registry().bind(g.name, 1, &qh, ())).collect());
        let mut client = Client::default();
        for output in &outputs {
            manager.get_output(output, &qh, ()).get_image_description(&qh, ()).get_information(&qh, ());
            queue.roundtrip(&mut client).unwrap();
        }
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
        let surface = compositor.create_surface(&qh, ());
        let feedback = manager.get_surface_feedback(&surface, &qh, ());
        feedback.get_preferred_parametric(&qh, ()).get_information(&qh, ());
        queue.roundtrip(&mut client).unwrap();
        (client.luminances, client.ready, client.preferred)
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    while !thread.is_finished() && Instant::now() < deadline {
        display.dispatch_clients(&mut server).unwrap();
        server.turn();
        server.dh.flush_clients().unwrap();
        std::thread::sleep(Duration::from_millis(1));
    }
    let (luminances, ready, preferred) = thread.join().unwrap();
    // The top: 650 nits with SDR white at 203; the second: unprofiled, SDR's 80 at its peak.
    assert_eq!(luminances, [(650, 203), (80, 80), (650, 203)], "the outputs', then the preferred");
    assert_eq!(ready, [server.colour.identities[0], server.colour.identities[1], server.colour.identities[0]]);
    assert_eq!(preferred, server.colour.identities[0]);
}
