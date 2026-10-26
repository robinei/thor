#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
use super::*;

#[test]
fn private_descriptors_are_inherited_only_by_the_selected_child() {
    let (mut parent, child) = UnixStream::pair().unwrap();
    parent.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let number = child.as_raw_fd();
    let mut command = Command::new("python3");
    command.args(["-c", "import os,sys; os.write(int(sys.argv[1]), b'private')", &number.to_string()]);
    let fd: OwnedFd = child.into();
    let copy = fd.try_clone().unwrap();
    let flags = rustix::io::fcntl_getfd(&copy).unwrap();
    assert!(flags.contains(rustix::io::FdFlags::CLOEXEC));
    let mut child = command.preserved_fds(vec![fd]).spawn().unwrap();
    let mut bytes = [0; 7];
    parent.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"private");
    assert!(child.wait().unwrap().success());
    assert_eq!(rustix::io::fcntl_getfd(&copy).unwrap(), flags, "the parent retains CLOEXEC");
}

fn server() -> Server {
    let display = smithay::reexports::wayland_server::Display::<Server>::new().unwrap();
    Server::new(
        clamshell_state::State::new(clamshell_state::Init::default()),
        clamshell_ui::Config { top_size: (960, 540), bottom_size: (620, 540), touch_layout: Arc::default() },
        display.handle(),
        false,
        || {},
        |_| crate::Process::default(),
        |_| {},
    )
    .unwrap()
}

#[test]
fn dormant_xwayland_has_no_child_or_deadline_and_a_connection_wakes_its_listener() {
    let mut event_loop = smithay::reexports::calloop::EventLoop::<Server>::try_new().unwrap();
    let supervisor = Supervisor::new(event_loop.handle()).unwrap();
    let path = format!("/tmp/.X11-unix/X{}", supervisor.socket.number);
    assert_eq!(supervisor.deadline(), None);
    assert!(supervisor.running.is_none());
    let _client = UnixStream::connect(&path).unwrap();
    event_loop.dispatch(Some(Duration::ZERO), &mut server()).unwrap();
    assert!(supervisor.demand.get(), "the connection wakes the loop without starting Xwayland during dispatch");
    assert!(supervisor.running.is_none());
    assert_eq!(supervisor.deadline(), None);
    // Drop also removes the registration and the display slot this instance owns.
    supervisor.demand.set(false);
    drop(supervisor);
    assert!(!std::path::Path::new(&path).exists());
}
