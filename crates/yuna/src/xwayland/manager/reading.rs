//! The manager reading what Yuna needs of a window: its properties, each with its bound
//! ([`Read`]), all asked at once when the window appears and each again when it changes, then
//! decoded into the [`Properties`] posted to Yuna; and its client's process, through XRes.
//!
//! It is apart from the events that call for a read (`x11_events.rs`), so that X11's side of the
//! properties, the requests and their replies, sits in one place. The bounds, and the decoding of
//! each property's bytes, are `xwayland/properties.rs`'s.

use x11rb::errors::{ConnectionError, ReplyError};
use x11rb::protocol::res::{ClientIdMask, ClientIdSpec, ConnectionExt as _};
use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt as _, Window};

use super::{Atoms, Failure, Manager, Tracked};
use crate::xwayland::properties::{
    self, MAX_EMULATED, MAX_LIST, MAX_STARTUP_ID_BYTES, MAX_TEXT_BYTES, Malformed, Properties,
    Protocols, Raw, TextTypes, WindowType, Wishes,
};

/// The properties of a window the manager reads, each with its bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Read {
    NetWmName,
    WmName,
    Class,
    NormalHints,
    Hints,
    Protocols,
    TransientFor,
    WindowType,
    Motif,
    FrameExtents,
    Pid,
    State,
    Emulated,
    SyncCounter,
    StartupId,
}

impl Read {
    pub(super) const ALL: [Read; 15] = [
        Read::NetWmName,
        Read::WmName,
        Read::Class,
        Read::NormalHints,
        Read::Hints,
        Read::Protocols,
        Read::TransientFor,
        Read::WindowType,
        Read::Motif,
        Read::FrameExtents,
        Read::Pid,
        Read::State,
        Read::Emulated,
        Read::SyncCounter,
        Read::StartupId,
    ];

    pub(super) fn atom(self, atoms: &Atoms) -> Atom {
        match self {
            Read::NetWmName => atoms._NET_WM_NAME,
            Read::WmName => AtomEnum::WM_NAME.into(),
            Read::Class => AtomEnum::WM_CLASS.into(),
            Read::NormalHints => AtomEnum::WM_NORMAL_HINTS.into(),
            Read::Hints => AtomEnum::WM_HINTS.into(),
            Read::Protocols => atoms.WM_PROTOCOLS,
            Read::TransientFor => AtomEnum::WM_TRANSIENT_FOR.into(),
            Read::WindowType => atoms._NET_WM_WINDOW_TYPE,
            Read::Motif => atoms._MOTIF_WM_HINTS,
            Read::FrameExtents => atoms._GTK_FRAME_EXTENTS,
            Read::Pid => atoms._NET_WM_PID,
            Read::State => atoms._NET_WM_STATE,
            Read::Emulated => atoms._XWAYLAND_RANDR_EMU_MONITOR_RECTS,
            Read::SyncCounter => atoms._NET_WM_SYNC_REQUEST_COUNTER,
            Read::StartupId => atoms._NET_STARTUP_ID,
        }
    }

    /// The most read of it, in 32-bit units, as `GetProperty` counts: each a property's whole
    /// size, or its bound (`properties.rs`).
    fn bound(self) -> u32 {
        match self {
            Read::NetWmName | Read::WmName | Read::Class => MAX_TEXT_BYTES / 4,
            Read::Protocols | Read::WindowType | Read::State => MAX_LIST,
            Read::NormalHints => 18,
            Read::Hints => 9,
            Read::Motif => 5,
            Read::FrameExtents => 4,
            Read::TransientFor | Read::Pid => 1,
            Read::Emulated => MAX_EMULATED * 4,
            Read::SyncCounter => 2,
            Read::StartupId => MAX_STARTUP_ID_BYTES / 4,
        }
    }
}

impl Manager<'_> {
    /// The process of `window`'s client, as XRes gives it from its connection; none without
    /// XRes, or for a window gone.
    pub(super) fn pid_of(&self, window: Window) -> Result<Option<i32>, Failure> {
        if !self.xres {
            return Ok(None);
        }
        let spec = ClientIdSpec {
            client: window,
            mask: ClientIdMask::LOCAL_CLIENT_PID,
        };
        let reply = match self.connection.res_query_client_ids(&[spec])?.reply() {
            Ok(reply) => reply,
            Err(ReplyError::X11Error(_)) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        Ok(reply
            .ids
            .iter()
            .find(|id| id.spec.mask.contains(ClientIdMask::LOCAL_CLIENT_PID))
            .and_then(|id| id.value.first())
            .and_then(|&pid| i32::try_from(pid).ok())
            .filter(|&pid| pid > 0))
    }

    /// Every property the manager reads of `window`, asked at once, then waited for: none for a
    /// window gone meanwhile.
    pub(super) fn read_all(&self, window: Window) -> Result<Vec<(Read, Option<Reply>)>, Failure> {
        let cookies = Read::ALL
            .into_iter()
            .map(|read| {
                let cookie = self.connection.get_property(
                    false,
                    window,
                    read.atom(&self.atoms),
                    AtomEnum::ANY,
                    0,
                    read.bound(),
                )?;
                Ok((read, cookie))
            })
            .collect::<Result<Vec<_>, ConnectionError>>()?;
        cookies
            .into_iter()
            .map(|(read, cookie)| Ok((read, answered(cookie.reply())?)))
            .collect()
    }

    /// One property of `window`; none for a window gone meanwhile.
    pub(super) fn read_one(&self, window: Window, read: Read) -> Result<Option<Reply>, Failure> {
        let cookie = self.connection.get_property(
            false,
            window,
            read.atom(&self.atoms),
            AtomEnum::ANY,
            0,
            read.bound(),
        )?;
        answered(cookie.reply())
    }

    /// Decodes `raw`, `read`'s value, into `tracked`'s properties; a property not set, or
    /// malformed, sets its field to its default, as if it were not set, whatever it held before.
    /// A malformed value, or a title read from `COMPOUND_TEXT`, is logged once per window.
    pub(super) fn apply(
        &self,
        window: Window,
        tracked: &mut Tracked,
        read: Read,
        raw: Option<&Reply>,
    ) {
        let unset = Reply {
            type_: 0,
            format: 0,
            value: Vec::new(),
            truncated: false,
        };
        let raw = raw.unwrap_or(&unset).raw();
        let outcome = decode(&self.atoms, read, raw, &mut tracked.properties);
        if outcome.is_err() {
            // Decoding what is not set gives the default, and never fails.
            let _ = decode(&self.atoms, read, unset.raw(), &mut tracked.properties);
        }
        if std::mem::replace(&mut tracked.logged, true) {
            return;
        }
        match outcome {
            Err(malformed) => tracing::info!(
                window,
                ?read,
                %malformed,
                "an X11 window's property is malformed and ignored; said once per window"
            ),
            Ok(Decoding::Compound) => tracing::info!(
                window,
                "an X11 window's title is COMPOUND_TEXT, read as Latin-1; said once per window"
            ),
            Ok(Decoding::Plain) => tracked.logged = false,
        }
    }
}

/// How a property was decoded, as far as the log is concerned.
enum Decoding {
    Plain,
    /// A title of `COMPOUND_TEXT`, read as Latin-1.
    Compound,
}

/// Decodes `raw`, `read`'s value, into its field of `properties`: the default when it is not
/// set; when it is malformed, the field is left as it was and the reason given (`apply` sets it
/// to its default then).
fn decode(
    atoms: &Atoms,
    read: Read,
    raw: Raw<'_>,
    properties: &mut Properties,
) -> Result<Decoding, Malformed> {
    let types = TextTypes {
        utf8_string: atoms.UTF8_STRING,
        string: AtomEnum::STRING.into(),
        compound_text: atoms.COMPOUND_TEXT,
    };
    let cardinal: Atom = AtomEnum::CARDINAL.into();
    let atom: Atom = AtomEnum::ATOM.into();
    match read {
        Read::NetWmName | Read::WmName => {
            let text = properties::decode_text(raw, types)?;
            let compound = text.as_ref().is_some_and(|text| text.compound);
            let text = text.map(|text| text.text);
            if read == Read::NetWmName {
                properties.net_wm_name = text;
            } else {
                properties.wm_name = text;
            }
            if compound {
                return Ok(Decoding::Compound);
            }
        }
        Read::Class => properties.class = properties::decode_class(raw, types.string)?,
        Read::NormalHints => {
            properties.size_hints =
                properties::decode_size_hints(raw, AtomEnum::WM_SIZE_HINTS.into())?
                    .unwrap_or_default();
        }
        Read::Hints => {
            properties.accepts_input =
                properties::decode_input_hint(raw, AtomEnum::WM_HINTS.into())?;
        }
        Read::Protocols => {
            let listed = properties::decode_atoms(raw, atom)?.unwrap_or_default();
            properties.protocols = Protocols {
                delete_window: listed.contains(&atoms.WM_DELETE_WINDOW),
                take_focus: listed.contains(&atoms.WM_TAKE_FOCUS),
                ping: listed.contains(&atoms._NET_WM_PING),
                sync_request: listed.contains(&atoms._NET_WM_SYNC_REQUEST),
            };
        }
        Read::TransientFor => {
            properties.transient_for = properties::decode_window(raw, AtomEnum::WINDOW.into())?;
        }
        Read::WindowType => {
            let listed = properties::decode_atoms(raw, atom)?.unwrap_or_default();
            properties.window_type = window_type(atoms, &listed);
        }
        Read::Motif => {
            properties.decorations =
                properties::decode_motif_decorations(raw, atoms._MOTIF_WM_HINTS)?;
        }
        Read::FrameExtents => {
            properties.frame_extents =
                properties::decode_frame_extents(raw, cardinal)?.unwrap_or_default();
        }
        Read::Pid => properties.pid = properties::decode_pid(raw, cardinal)?,
        Read::State => {
            let listed = properties::decode_atoms(raw, atom)?.unwrap_or_default();
            properties.wishes = Wishes {
                fullscreen: listed.contains(&atoms._NET_WM_STATE_FULLSCREEN),
                maximized: listed.contains(&atoms._NET_WM_STATE_MAXIMIZED_HORZ),
            };
        }
        Read::Emulated => {
            properties.emulated = properties::decode_emulated(raw, cardinal)?.unwrap_or_default();
        }
        Read::SyncCounter => {
            properties.sync_counter = properties::decode_counter(raw, cardinal)?;
        }
        Read::StartupId => {
            properties.startup_id = properties::decode_text(raw, types)?.map(|text| text.text);
        }
    }
    Ok(Decoding::Plain)
}

/// A property's value as the manager keeps it while it decodes it.
pub(super) struct Reply {
    type_: Atom,
    format: u8,
    value: Vec<u8>,
    truncated: bool,
}

impl Reply {
    fn raw(&self) -> Raw<'_> {
        Raw {
            type_: self.type_,
            format: self.format,
            value: &self.value,
            truncated: self.truncated,
        }
    }
}

/// A `GetProperty` reply as the manager keeps it: none when X11 refused the request, for a
/// window gone; a lost connection fails.
fn answered(
    reply: Result<x11rb::protocol::xproto::GetPropertyReply, ReplyError>,
) -> Result<Option<Reply>, Failure> {
    match reply {
        Ok(reply) => Ok(Some(Reply {
            type_: reply.type_,
            format: reply.format,
            truncated: reply.bytes_after > 0,
            value: reply.value,
        })),
        Err(ReplyError::X11Error(_)) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// The first type of `listed` the manager knows, EWMH listing them by preference; normal
/// without one.
fn window_type(atoms: &Atoms, listed: &[Atom]) -> WindowType {
    listed
        .iter()
        .find_map(|&atom| {
            Some(match atom {
                a if a == atoms._NET_WM_WINDOW_TYPE_NORMAL => WindowType::Normal,
                a if a == atoms._NET_WM_WINDOW_TYPE_DIALOG => WindowType::Dialog,
                a if a == atoms._NET_WM_WINDOW_TYPE_UTILITY => WindowType::Utility,
                a if a == atoms._NET_WM_WINDOW_TYPE_TOOLBAR => WindowType::Toolbar,
                a if a == atoms._NET_WM_WINDOW_TYPE_SPLASH => WindowType::Splash,
                a if a == atoms._NET_WM_WINDOW_TYPE_DOCK => WindowType::Dock,
                a if a == atoms._NET_WM_WINDOW_TYPE_DESKTOP => WindowType::Desktop,
                a if [
                    atoms._NET_WM_WINDOW_TYPE_MENU,
                    atoms._NET_WM_WINDOW_TYPE_DROPDOWN_MENU,
                    atoms._NET_WM_WINDOW_TYPE_POPUP_MENU,
                    atoms._NET_WM_WINDOW_TYPE_TOOLTIP,
                    atoms._NET_WM_WINDOW_TYPE_NOTIFICATION,
                    atoms._NET_WM_WINDOW_TYPE_COMBO,
                    atoms._NET_WM_WINDOW_TYPE_DND,
                ]
                .contains(&a) =>
                {
                    WindowType::Transient
                }
                _ => return None,
            })
        })
        .unwrap_or_default()
}
