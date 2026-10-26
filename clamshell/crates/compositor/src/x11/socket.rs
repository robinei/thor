//! Reserve a display socket while Xwayland is dormant. Its first connection starts the server.
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::PathBuf,
};
pub(super) struct Socket {
    pub listener: UnixListener,
    pub number: u32,
    path: PathBuf,
    lock: PathBuf,
    _file: File,
}
impl Socket {
    pub fn reserve() -> io::Result<Self> {
        let directory = PathBuf::from("/tmp/.X11-unix");
        match fs::create_dir(&directory) {
            Ok(()) => fs::set_permissions(&directory, fs::Permissions::from_mode(0o1777))?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        for number in 0..128 {
            let lock = PathBuf::from(format!("/tmp/.X{number}-lock"));
            let mut file = match OpenOptions::new().write(true).create_new(true).open(&lock) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    let pid = File::open(&lock).ok().and_then(|file| {
                        let mut text = String::new();
                        file.take(64).read_to_string(&mut text).ok()?;
                        text.trim().parse().ok().filter(|pid| *pid > 0).and_then(rustix::process::Pid::from_raw)
                    });
                    if !pid.is_some_and(|pid| rustix::process::test_kill_process(pid) == Err(rustix::io::Errno::SRCH)) {
                        continue;
                    }
                    // A dead lock owner cannot have a live listener. Leave occupied/unknown slots alone.
                    let path = directory.join(format!("X{number}"));
                    let _ = fs::remove_file(path);
                    if let Err(e) = fs::remove_file(&lock) {
                        log::warn!("compositor: X11 display :{number}'s stale lock {}: {e}; trying the next display", lock.display());
                        continue;
                    }
                    match OpenOptions::new().write(true).create_new(true).open(&lock) {
                        Ok(file) => file,
                        Err(_) => continue,
                    }
                }
                Err(error) => return Err(error),
            };
            if let Err(error) = writeln!(file, "{:>10}", std::process::id()) {
                let _ = fs::remove_file(&lock);
                return Err(error);
            }
            let path = directory.join(format!("X{number}"));
            match UnixListener::bind(&path) {
                Ok(listener) => {
                    let socket = Self { listener, number, path, lock, _file: file };
                    socket.listener.set_nonblocking(true)?;
                    return Ok(socket);
                }
                Err(error) => {
                    let _ = fs::remove_file(&lock);
                    if error.kind() != io::ErrorKind::AddrInUse {
                        return Err(error);
                    }
                }
            }
        }
        Err(io::Error::new(io::ErrorKind::AddrInUse, "no free X11 display socket"))
    }
}
impl Drop for Socket {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(&self.lock);
    }
}
