//! The descriptor tells WLCS exactly the globals a client of the shim's seat is announced, at
//! their versions: Yuna is started as the shim's server starts it, a client connects to its
//! seat, and its registry is read on the Wayland wire, by hand, so that the shim's tests need no
//! client library.

use std::collections::BTreeMap;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use yuna_wlcs::compositor::WlcsCompositor;
use yuna_wlcs::descriptor::{DMABUF, EXTENSIONS};

/// The ids the client gives its objects: the display's is 1, by the protocol.
const DISPLAY: u32 = 1;
const REGISTRY: u32 = 2;
const CALLBACK: u32 = 3;

/// A message's header and its arguments, as 32-bit words in the machine's order.
fn message(object: u32, opcode: u16, arguments: &[u32]) -> Vec<u8> {
    let size = u32::try_from(8 + 4 * arguments.len()).expect("a short message");
    let mut bytes = Vec::new();
    bytes.extend(object.to_ne_bytes());
    bytes.extend(((size << 16) | u32::from(opcode)).to_ne_bytes());
    for argument in arguments {
        bytes.extend(argument.to_ne_bytes());
    }
    bytes
}

/// The 32-bit word at `at`.
fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_ne_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

/// `wl_registry.global`'s arguments: its name, its interface and its version.
fn global(body: &[u8]) -> (String, u32) {
    let length = usize::try_from(word(body, 4)).expect("a length");
    // The string holds its terminating zero, and is padded to a word.
    let interface = String::from_utf8(body[8..8 + length - 1].to_vec()).expect("a name");
    let version = word(body, 8 + length.next_multiple_of(4));
    (interface, version)
}

/// What `stream`'s registry announces, read once the compositor has answered the client's
/// `sync`: each global's interface, with its version.
fn announced_globals(wlcs: &mut WlcsCompositor, mut stream: UnixStream) -> BTreeMap<String, u32> {
    stream
        .set_nonblocking(true)
        .expect("a socket that does not block");
    let mut requests = message(DISPLAY, 1, &[REGISTRY]);
    requests.extend(message(DISPLAY, 0, &[CALLBACK]));
    stream.write_all(&requests).expect("the requests are sent");
    let mut received = Vec::new();
    let mut globals = BTreeMap::new();
    for _ in 0..500 {
        wlcs.compositor()
            .dispatch(Some(Duration::from_millis(10)))
            .expect("Yuna dispatches");
        let mut buffer = [0; 4096];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => panic!("Yuna closed the connection"),
                Ok(read) => received.extend_from_slice(&buffer[..read]),
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => panic!("the connection failed: {error}"),
            }
        }
        while received.len() >= 8 {
            let (object, header) = (word(&received, 0), word(&received, 4));
            let size = usize::try_from(header >> 16).expect("a size");
            if received.len() < size {
                break;
            }
            let body = &received[8..size];
            match (object, header & 0xffff) {
                (REGISTRY, 0) => {
                    let (interface, version) = global(body);
                    let previous = globals.insert(interface.clone(), version);
                    assert_eq!(previous, None, "{interface} announced twice");
                }
                (CALLBACK, 0) => return globals,
                (DISPLAY, 0) => panic!("Yuna sent the client an error"),
                _ => {}
            }
            received.drain(..size);
        }
    }
    panic!("Yuna did not answer the client's sync");
}

#[test]
fn the_descriptor_is_what_a_client_of_the_seat_is_announced() {
    let mut wlcs = WlcsCompositor::start().expect("Yuna starts as the shim starts it");
    let stream = wlcs.connect_client().expect("a client connects");
    let announced = announced_globals(&mut wlcs, stream);
    let dmabuf_offered = wlcs.compositor().dmabuf_format_count() > 0;
    let described: BTreeMap<String, u32> = EXTENSIONS
        .iter()
        .filter(|&&(name, _)| dmabuf_offered || name != DMABUF)
        .map(|&(name, version)| (name.to_str().expect("a name").to_owned(), version))
        .collect();
    assert_eq!(
        described.len(),
        EXTENSIONS.len() - usize::from(!dmabuf_offered),
        "a global is listed twice"
    );
    assert_eq!(
        announced, described,
        "src/descriptor.rs differs from the globals a client of the shim's seat is announced"
    );
}
