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
