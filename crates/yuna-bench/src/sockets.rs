//! Which Unix socket is which: the path each is bound to, and the socket each is connected to.
//!
//! A client's end of a connection has no path of its own: only its peer, the end the server
//! accepted, shows the listener's path. `/proc/net/unix` lists paths but no peers; the kernel's
//! socket diagnostics (`NETLINK_SOCK_DIAG`, `linux/unix_diag.h`), which `ss -x` reads, give both
//! to any user. The bench reads them to refuse a compositor that is connected to a socket in the
//! owner's runtime folder, such as the owner's Wayland display or session bus.

use std::collections::BTreeMap;

use rustix::net::{
    AddressFamily, RecvFlags, SendFlags, SocketFlags, SocketType, netlink, recv, sendto,
    socket_with,
};

/// Each Unix socket's path and peer, by its inode.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SocketTable {
    /// The path each socket is bound to, or that its listener is bound to. An abstract name
    /// starts with `@`.
    pub paths: BTreeMap<u64, String>,
    /// The socket each connected socket is connected to.
    pub peers: BTreeMap<u64, u64>,
    /// Whether the peers could be read: without them, only paths are known.
    pub peers_known: bool,
}

impl SocketTable {
    /// Reads the table from the kernel's socket diagnostics, or, if they cannot be read, the
    /// paths alone from `/proc/net/unix` (`net_unix`, its text).
    pub fn read(net_unix: &str) -> SocketTable {
        match read_diagnostics() {
            Ok(table) => table,
            Err(_) => SocketTable {
                paths: parse_net_unix(net_unix),
                peers: BTreeMap::new(),
                peers_known: false,
            },
        }
    }

    /// The path of socket `inode`, or of the socket it is connected to.
    pub fn reaches(&self, inode: u64) -> Option<&str> {
        self.paths
            .get(&inode)
            .or_else(|| self.paths.get(self.peers.get(&inode)?))
            .map(String::as_str)
    }
}

/// The inode of a descriptor's link target `socket:[<inode>]`.
pub fn socket_inode(target: &str) -> Option<u64> {
    target
        .strip_prefix("socket:[")?
        .strip_suffix(']')?
        .parse()
        .ok()
}

/// The paths of `/proc/net/unix`: `Num RefCount Protocol Flags Type St Inode Path`.
pub fn parse_net_unix(text: &str) -> BTreeMap<u64, String> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let inode = fields.get(6)?.parse().ok()?;
            Some((inode, (*fields.get(7)?).to_owned()))
        })
        .collect()
}

// From linux/netlink.h, linux/sock_diag.h and linux/unix_diag.h.
const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const NLM_F_REQUEST: u16 = 0x1;
const NLM_F_DUMP: u16 = 0x300;
const SOCK_DIAG_BY_FAMILY: u16 = 20;
const AF_UNIX: u8 = 1;
const UDIAG_SHOW_NAME: u32 = 0x1;
const UDIAG_SHOW_PEER: u32 = 0x4;
const UNIX_DIAG_NAME: u16 = 0;
const UNIX_DIAG_PEER: u16 = 2;
/// `struct nlmsghdr`'s length.
const HEADER: usize = 16;
/// `struct unix_diag_msg`'s length.
const MESSAGE: usize = 16;

/// Asks the kernel for every Unix socket, with its name and its peer.
fn read_diagnostics() -> std::io::Result<SocketTable> {
    // Close-on-exec, as every descriptor of the bench: a child the bench starts meanwhile, on
    // another thread, must not inherit it.
    let fd = socket_with(
        AddressFamily::NETLINK,
        SocketType::DGRAM,
        SocketFlags::CLOEXEC,
        Some(netlink::SOCK_DIAG),
    )?;
    sendto(
        &fd,
        &request(),
        SendFlags::empty(),
        &netlink::SocketAddrNetlink::new(0, 0),
    )?;
    let mut table = SocketTable {
        peers_known: true,
        ..SocketTable::default()
    };
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let (length, _) = recv(&fd, &mut buffer[..], RecvFlags::empty())?;
        match parse_replies(&buffer[..length.min(buffer.len())], &mut table) {
            Replies::More => continue,
            Replies::Done => return Ok(table),
            Replies::Failed => return Err(std::io::Error::other("sock_diag refused the request")),
        }
    }
}

/// The dump request: a `struct nlmsghdr` and a `struct unix_diag_req`, in the machine's order.
fn request() -> Vec<u8> {
    let mut message = Vec::with_capacity(HEADER + 24);
    let length = (HEADER + 24) as u32;
    message.extend_from_slice(&length.to_ne_bytes());
    message.extend_from_slice(&SOCK_DIAG_BY_FAMILY.to_ne_bytes());
    message.extend_from_slice(&(NLM_F_REQUEST | NLM_F_DUMP).to_ne_bytes());
    message.extend_from_slice(&1u32.to_ne_bytes()); // sequence
    message.extend_from_slice(&0u32.to_ne_bytes()); // port: the kernel
    message.push(AF_UNIX); // sdiag_family
    message.push(0); // sdiag_protocol
    message.extend_from_slice(&0u16.to_ne_bytes()); // pad
    message.extend_from_slice(&u32::MAX.to_ne_bytes()); // every state
    message.extend_from_slice(&0u32.to_ne_bytes()); // every inode
    message.extend_from_slice(&(UDIAG_SHOW_NAME | UDIAG_SHOW_PEER).to_ne_bytes());
    message.extend_from_slice(&[0xff; 8]); // no cookie
    message
}

/// What a datagram of replies said.
#[derive(Debug, PartialEq)]
enum Replies {
    More,
    Done,
    Failed,
}

fn read_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_ne_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_ne_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// Reads the messages of one datagram into `table`.
fn parse_replies(datagram: &[u8], table: &mut SocketTable) -> Replies {
    let mut at = 0;
    while let (Some(length), Some(kind)) = (read_u32(datagram, at), read_u16(datagram, at + 4)) {
        let length = length as usize;
        if length < HEADER || at + length > datagram.len() {
            return Replies::Failed;
        }
        match kind {
            NLMSG_DONE => return Replies::Done,
            NLMSG_ERROR => return Replies::Failed,
            _ => read_socket(&datagram[at + HEADER..at + length], table),
        }
        // Each message is aligned to 4 bytes.
        at += (length + 3) & !3;
    }
    Replies::More
}

/// Reads one `struct unix_diag_msg` and its attributes.
fn read_socket(message: &[u8], table: &mut SocketTable) {
    let Some(inode) = read_u32(message, 4) else {
        return;
    };
    let inode = u64::from(inode);
    let mut at = MESSAGE;
    while let (Some(length), Some(kind)) = (read_u16(message, at), read_u16(message, at + 2)) {
        let length = usize::from(length);
        let Some(payload) = message.get(at + 4..at + length.max(4)) else {
            return;
        };
        match kind {
            UNIX_DIAG_NAME if !payload.is_empty() => {
                // An abstract name starts with a NUL, which `ss` and `/proc/net/unix` show as `@`.
                let name = if payload[0] == 0 {
                    format!("@{}", String::from_utf8_lossy(&payload[1..]))
                } else {
                    String::from_utf8_lossy(payload)
                        .trim_end_matches('\0')
                        .to_owned()
                };
                table.paths.insert(inode, name);
            }
            UNIX_DIAG_PEER => {
                if let Some(peer) = read_u32(payload, 0) {
                    table.peers.insert(inode, u64::from(peer));
                }
            }
            _ => {}
        }
        if length < 4 {
            return;
        }
        at += (length + 3) & !3;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reply for socket `inode`, with a name and a peer if given.
    fn reply(inode: u32, name: Option<&[u8]>, peer: Option<u32>) -> Vec<u8> {
        let mut body = vec![AF_UNIX, 1, 1, 0];
        body.extend_from_slice(&inode.to_ne_bytes());
        body.extend_from_slice(&[0; 8]);
        if let Some(name) = name {
            body.extend_from_slice(&((4 + name.len()) as u16).to_ne_bytes());
            body.extend_from_slice(&UNIX_DIAG_NAME.to_ne_bytes());
            body.extend_from_slice(name);
            while body.len() % 4 != 0 {
                body.push(0);
            }
        }
        if let Some(peer) = peer {
            body.extend_from_slice(&8u16.to_ne_bytes());
            body.extend_from_slice(&UNIX_DIAG_PEER.to_ne_bytes());
            body.extend_from_slice(&peer.to_ne_bytes());
        }
        let mut message = Vec::new();
        message.extend_from_slice(&((HEADER + body.len()) as u32).to_ne_bytes());
        message.extend_from_slice(&SOCK_DIAG_BY_FAMILY.to_ne_bytes());
        message.extend_from_slice(&[0; 10]);
        message.extend(body);
        message
    }

    #[test]
    fn diagnostics_give_paths_and_peers() {
        let mut datagram = reply(10, Some(b"/run/user/1000/wayland-1"), Some(11));
        // The client's end: no name, connected to 10.
        datagram.extend(reply(11, None, Some(10)));
        datagram.extend(reply(12, Some(b"\0/tmp/.X11-unix/X1"), None));
        let mut table = SocketTable::default();
        assert_eq!(parse_replies(&datagram, &mut table), Replies::More);
        assert_eq!(table.reaches(10), Some("/run/user/1000/wayland-1"));
        assert_eq!(table.reaches(11), Some("/run/user/1000/wayland-1"));
        assert_eq!(table.reaches(12), Some("@/tmp/.X11-unix/X1"));
        assert_eq!(table.reaches(13), None);
        let mut done = Vec::new();
        done.extend_from_slice(&(HEADER as u32).to_ne_bytes());
        done.extend_from_slice(&NLMSG_DONE.to_ne_bytes());
        done.extend_from_slice(&[0; 10]);
        assert_eq!(parse_replies(&done, &mut table), Replies::Done);
        assert_eq!(parse_replies(&[1, 2, 3], &mut table), Replies::More);
    }

    #[test]
    fn proc_net_unix_gives_paths() {
        let text = "Num       RefCount Protocol Flags    Type St Inode Path\n\
                    0000000000000000: 00000002 00000000 00010000 0001 01 4242 /run/user/1000/bus\n\
                    0000000000000000: 00000003 00000000 00000000 0001 03 4343\n";
        let paths = parse_net_unix(text);
        assert_eq!(
            paths.get(&4242).map(String::as_str),
            Some("/run/user/1000/bus")
        );
        assert!(!paths.contains_key(&4343));
        assert_eq!(socket_inode("socket:[4343]"), Some(4343));
        assert_eq!(socket_inode("/dev/null"), None);
    }

    #[test]
    fn the_kernel_answers_the_request() {
        // Any Linux machine has Unix sockets: this test's own runtime has some.
        let table = SocketTable::read("");
        assert!(table.peers_known, "sock_diag could not be read");
        assert!(!table.paths.is_empty());
    }
}
