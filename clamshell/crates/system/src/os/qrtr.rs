//! QRTR sockets (AF_QIPCRTR), which Linux uses to talk to services on the Qualcomm DSPs. Each service is at
//! a node, which is the processor, and a port.
//!
//! The sockets carry datagrams. [`send_to`] sends one to a node and port, and [`recv_from`] says where one
//! came from. The name service, on our own node at [`PORT_CTRL`], tells a socket where a service is: after
//! a [`lookup`] it sends NEW_SERVER and DEL_SERVER packets as the service comes and goes. rustix has no
//! QRTR, so these functions call libc.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

/// The address family (linux/socket.h).
const AF_QIPCRTR: libc::c_int = 42;
/// The name service's port, on every node.
pub const PORT_CTRL: u32 = 0xffff_fffe;
/// Commands in control packets (linux/qrtr.h).
pub const NEW_SERVER: u32 = 4;
pub const DEL_SERVER: u32 = 5;
const NEW_LOOKUP: u32 = 10;

/// struct sockaddr_qrtr.
#[repr(C)]
#[derive(Default)]
struct Addr {
    family: libc::sa_family_t,
    node: u32,
    port: u32,
}

const ADDR_LEN: libc::socklen_t = std::mem::size_of::<Addr>() as libc::socklen_t;

/// A non-blocking QRTR datagram socket, and its node.
pub fn socket() -> io::Result<(OwnedFd, u32)> {
    #[allow(unsafe_code)]
    // SAFETY: socket(2) with constant arguments; a valid descriptor is owned from here on.
    let fd = unsafe {
        let fd = libc::socket(AF_QIPCRTR, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK, 0);
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        OwnedFd::from_raw_fd(fd)
    };
    let mut me = Addr::default();
    let mut len = ADDR_LEN;
    #[allow(unsafe_code)]
    // SAFETY: getsockname(2) writes at most `len` bytes into `me`, a sockaddr_qrtr.
    let r = unsafe { libc::getsockname(fd.as_raw_fd(), std::ptr::from_mut(&mut me).cast(), &raw mut len) };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((fd, me.node))
}

/// Sends a datagram to `node`:`port`.
pub fn send_to(fd: &OwnedFd, node: u32, port: u32, data: &[u8]) -> io::Result<()> {
    let to = Addr { family: AF_QIPCRTR as libc::sa_family_t, node, port };
    #[allow(unsafe_code)]
    // SAFETY: sendto(2) reads `data.len()` bytes of `data` and ADDR_LEN bytes of `to`.
    let r = unsafe { libc::sendto(fd.as_raw_fd(), data.as_ptr().cast(), data.len(), 0, std::ptr::from_ref(&to).cast(), ADDR_LEN) };
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

/// Reads a datagram into `buf`. Returns its length and where it came from (node, port), or `None` when
/// nothing is waiting.
pub fn recv_from(fd: &OwnedFd, buf: &mut [u8]) -> io::Result<Option<(usize, u32, u32)>> {
    let mut from = Addr::default();
    let mut len = ADDR_LEN;
    #[allow(unsafe_code)]
    // SAFETY: recvfrom(2) writes at most `buf.len()` bytes into `buf` and `len` bytes into `from`.
    let n = unsafe { libc::recvfrom(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0, std::ptr::from_mut(&mut from).cast(), &raw mut len) };
    if n < 0 {
        let e = io::Error::last_os_error();
        return if e.kind() == io::ErrorKind::WouldBlock { Ok(None) } else { Err(e) };
    }
    Ok(Some((n.unsigned_abs(), from.node, from.port)))
}

/// Asks the name service on our `node` to tell this socket where `service` is, and to tell it again
/// whenever the service comes or goes.
pub fn lookup(fd: &OwnedFd, node: u32, service: u32) -> io::Result<()> {
    let mut pkt = [0u8; 20];
    pkt[..4].copy_from_slice(&NEW_LOOKUP.to_le_bytes());
    pkt[4..8].copy_from_slice(&service.to_le_bytes());
    send_to(fd, node, PORT_CTRL, &pkt)
}

/// A control packet from the name service: its command and the server's service, node and port.
pub fn control(pkt: &[u8]) -> Option<(u32, u32, u32, u32)> {
    let word = |i: usize| pkt.get(i..i + 4).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
    Some((word(0)?, word(4)?, word(12)?, word(16)?))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn a_control_packet_reads_as_command_service_node_port() {
        let mut pkt = [0u8; 20];
        pkt[..4].copy_from_slice(&NEW_SERVER.to_le_bytes());
        pkt[4..8].copy_from_slice(&400u32.to_le_bytes());
        pkt[8..12].copy_from_slice(&1u32.to_le_bytes());
        pkt[12..16].copy_from_slice(&5u32.to_le_bytes());
        pkt[16..20].copy_from_slice(&0x4011u32.to_le_bytes());
        assert_eq!(control(&pkt), Some((NEW_SERVER, 400, 5, 0x4011)));
        assert_eq!(control(&pkt[..12]), None, "too short");
    }
}
