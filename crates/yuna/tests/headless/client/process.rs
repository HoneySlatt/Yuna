//! The test client against Yuna run as a process, `yuna --headless`, rather than a compositor in
//! the test's own thread: the same requests, with a roundtrip and a wait that block on the socket
//! until Yuna answers, for the traces on the real clock (`traces/real.rs`).

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, poll};
use rustix::time::Timespec;
use wayland_client::Connection;
use wayland_client::backend::WaylandError;

use super::{CallbackKind, Client, ClientState};

/// How long a wait for Yuna's answer lasts before the test fails: far past any refresh.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

impl Client {
    /// Connects to the Yuna process whose socket is at `path`, as a client of the seat that socket
    /// serves, and binds its globals.
    pub fn connect_to_process(path: &Path) -> Client {
        let stream = UnixStream::connect(path).expect("Yuna's socket accepts a client");
        let socket = stream.try_clone().expect("a copy of the client's socket");
        let connection = Connection::from_socket(stream).expect("a connection");
        let queue = connection.new_event_queue();
        let registry = connection.display().get_registry(&queue.handle(), ());
        let mut client = Client {
            connection,
            queue,
            registry,
            state: ClientState::default(),
            socket,
        };
        // Once for the globals, once for the events of the objects bound.
        client.roundtrip_with_process();
        client.roundtrip_with_process();
        assert!(
            client.state.compositor.is_some()
                && client.state.shm.is_some()
                && client.state.wm_base.is_some(),
            "a global is missing"
        );
        client
    }

    /// As [`Client::roundtrip`], with Yuna run as a process: sends what the client wrote and
    /// handles every event until Yuna has answered all that came before.
    pub fn roundtrip_with_process(&mut self) {
        let target = self.state.syncs + 1;
        self.connection
            .display()
            .sync(&self.queue.handle(), CallbackKind::Sync);
        while self.state.syncs < target {
            self.wait_for_process();
        }
    }

    /// Sends what the client wrote, then waits for the next events Yuna sends and handles them,
    /// without asking anything: for an answer that comes at a refresh, which a roundtrip would not
    /// wait for. Fails when nothing comes within [`ANSWER_TIMEOUT`].
    pub fn wait_for_process(&mut self) {
        self.queue.flush().expect("the client sends");
        if let Some(guard) = self.queue.prepare_read() {
            let fd = guard.connection_fd();
            let mut fds = [PollFd::new(&fd, PollFlags::IN | PollFlags::ERR)];
            let deadline = Instant::now() + ANSWER_TIMEOUT;
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                assert!(
                    !left.is_zero(),
                    "Yuna did not answer within {ANSWER_TIMEOUT:?}"
                );
                let timeout = Timespec {
                    tv_sec: left.as_secs() as i64,
                    tv_nsec: i64::from(left.subsec_nanos()),
                };
                match poll(&mut fds, Some(&timeout)) {
                    Ok(0) | Err(rustix::io::Errno::INTR) => continue,
                    Ok(_) => break,
                    Err(error) => panic!("the client's socket cannot be polled: {error}"),
                }
            }
            match guard.read() {
                Ok(_) => {}
                Err(WaylandError::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("the client's connection failed: {error}"),
            }
        }
        self.queue
            .dispatch_pending(&mut self.state)
            .expect("the client's events are handled");
    }
}
