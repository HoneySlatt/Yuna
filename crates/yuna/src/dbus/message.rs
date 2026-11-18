//! A D-Bus message: its header, Yuna's own, and its body, both encoded through zbus's `wire`
//! module ([the specification](https://dbus.freedesktop.org/doc/dbus-specification.html#message-protocol)).
//!
//! A message is a fixed part of 12 bytes, an array of header fields, padding to 8 bytes, then
//! its body. The header's signature is `yyyyuua(yv)`: its byte order, its kind, its flags, the
//! protocol's version, the body's length, its serial and its fields. Its fields say where it goes
//! (path, interface, member, destination), whom it answers (reply serial) and what its body holds
//! (signature).
//!
//! Decoding refuses, with a [`MessageError`] and never a panic: a message over [`MAX_MESSAGE`],
//! a byte order other than `l` or `B`, a version other than 1, a field of the wrong type, a
//! kind without the fields it must have, a signature that does not parse, and file descriptors,
//! which Yuna never negotiates. Its body is decoded only when asked, for the signature the caller
//! expects ([`Message::body`]).
//!
//! Encoding refuses, with a [`MessageError`] and never a panic, what a bus disconnects its sender
//! for, so that Yuna never sends it: a kind without the fields it must have, a reply serial of 0,
//! a destination or a sender that is no bus name, an interface, a member or an error name that
//! breaks its rules (`names.rs`), a path that is no object path, and the path and the interface
//! the bus reserves. Names and paths come from programs too: a toolkit's bus name and its nodes'
//! paths are what Yuna's walks call.

use std::fmt;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde::ser::{SerializeStruct, Serializer};
use zbus::wire::serialized::{Context, Data};
use zbus::wire::{Endian, ObjectPath, OwnedVariant, Signature, Str, Variant};

use super::names::{self, LOCAL_INTERFACE, LOCAL_PATH, MAX_NAME};

/// The largest message Yuna sends or reads, header and body: 4 MiB. The accessibility bus refuses
/// larger ones too (its `max_message_size`), far under the 128 MiB the specification allows, so
/// that one reply never holds more than Yuna reads at a time.
pub(crate) const MAX_MESSAGE: usize = 4 << 20;

/// The fixed part of a header and the length of its fields' array: what says how long the whole
/// message is.
pub(crate) const FRAME_PREFIX: usize = 16;

/// The flag a call sets when it wants no reply.
pub(crate) const NO_REPLY_EXPECTED: u8 = 0x1;

/// The header's fields, by the codes the specification gives them.
const PATH: u8 = 1;
const INTERFACE: u8 = 2;
const MEMBER: u8 = 3;
const ERROR_NAME: u8 = 4;
const REPLY_SERIAL: u8 = 5;
const DESTINATION: u8 = 6;
const SENDER: u8 = 7;
const SIGNATURE: u8 = 8;
const UNIX_FDS: u8 = 9;

/// What a message is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    MethodCall,
    MethodReturn,
    Error,
    Signal,
}

impl Kind {
    fn code(self) -> u8 {
        match self {
            Kind::MethodCall => 1,
            Kind::MethodReturn => 2,
            Kind::Error => 3,
            Kind::Signal => 4,
        }
    }
}

/// Why a message could not be encoded or decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MessageError {
    /// Longer than [`MAX_MESSAGE`], announced or encoded.
    TooLong(usize),
    /// Malformed: what is wrong.
    Malformed(String),
    /// A kind the specification may add later, which a reader must ignore: its code.
    UnknownKind(u8),
}

impl fmt::Display for MessageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MessageError::TooLong(length) => {
                write!(
                    f,
                    "a message of {length} bytes, over the {MAX_MESSAGE} read"
                )
            }
            MessageError::Malformed(why) => write!(f, "a malformed message: {why}"),
            MessageError::UnknownKind(code) => write!(f, "a message of unknown kind {code}"),
        }
    }
}

fn malformed(why: impl fmt::Display) -> MessageError {
    MessageError::Malformed(why.to_string())
}

/// A name or a path as an error shows it: quoted, or only its length past [`MAX_NAME`], since a
/// program may give one of megabytes.
fn shown(value: &str) -> String {
    if value.len() <= MAX_NAME {
        format!("{value:?}")
    } else {
        format!("of {} bytes", value.len())
    }
}

/// A message's body, encoded, with its signature: the types of its arguments.
///
/// zbus parses `(so)`, one structure, and `so`, two arguments, to the same [`Signature`]: their
/// bytes are the same too. A body Yuna sends keeps the exact text it was given, which the header
/// carries and which a toolkit checks (GTK 4 expects `Embed`'s reply as `((so))`); a body read
/// keeps what zbus parsed, which [`Message::body`] compares without outer parentheses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Body {
    text: String,
    parsed: Signature,
    bytes: Vec<u8>,
    endian: Endian,
}

impl Default for Body {
    fn default() -> Body {
        Body {
            text: String::new(),
            parsed: Signature::Unit,
            bytes: Vec::new(),
            endian: Endian::Little,
        }
    }
}

impl Body {
    /// No argument.
    pub(crate) fn empty() -> Body {
        Body::default()
    }

    /// `value` encoded as arguments of `signature`: a tuple for several, whose signature has no
    /// outer parentheses (`sss`), or one value (`(so)` is one structure). Little-endian, as
    /// every message Yuna sends.
    pub(crate) fn new<T: Serialize + ?Sized>(
        signature: &str,
        value: &T,
    ) -> Result<Body, MessageError> {
        let parsed = Signature::try_from(signature).map_err(malformed)?;
        let encoded =
            zbus::wire::to_bytes_for_signature(Context::new(Endian::Little, 0), &parsed, value)
                .map_err(malformed)?;
        if encoded.len() > MAX_MESSAGE {
            return Err(MessageError::TooLong(encoded.len()));
        }
        Ok(Body {
            text: signature.to_owned(),
            parsed,
            bytes: encoded.bytes().to_vec(),
            endian: Endian::Little,
        })
    }
}

/// A header field's value, a variant: one of zbus's, or the body's signature written exactly as
/// given. zbus's own `Signature` writes a structure with its parentheses, so that the arguments
/// `su` would go as one structure `(su)`, which a bus refuses for `RequestName`.
enum Field<'a> {
    Value(Variant<'a>),
    Signature(&'a str),
}

impl Serialize for Field<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Field::Value(value) => value.serialize(serializer),
            // A variant as zbus's serializer takes one, as `Variant` itself writes it: its
            // value's signature, here `g`, then the value, the text.
            Field::Signature(text) => {
                let mut variant = serializer.serialize_struct("Variant", 2)?;
                variant.serialize_field("signature", &Signature::Signature)?;
                variant.serialize_field("value", text)?;
                variant.end()
            }
        }
    }
}

/// A D-Bus message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Message {
    pub kind: Kind,
    pub flags: u8,
    /// Its serial, given by the connection that sends it: 0 until then.
    pub serial: u32,
    pub path: Option<String>,
    pub interface: Option<String>,
    pub member: Option<String>,
    pub error_name: Option<String>,
    pub reply_serial: Option<u32>,
    pub destination: Option<String>,
    pub sender: Option<String>,
    body: Body,
}

impl Message {
    fn new(kind: Kind, body: Body) -> Message {
        Message {
            kind,
            flags: 0,
            serial: 0,
            path: None,
            interface: None,
            member: None,
            error_name: None,
            reply_serial: None,
            destination: None,
            sender: None,
            body,
        }
    }

    /// A call of `member` of `interface` on the object `path` of `destination`.
    pub(crate) fn method_call(
        destination: &str,
        path: &str,
        interface: &str,
        member: &str,
        body: Body,
    ) -> Message {
        Message {
            destination: Some(destination.to_owned()),
            path: Some(path.to_owned()),
            interface: Some(interface.to_owned()),
            member: Some(member.to_owned()),
            ..Message::new(Kind::MethodCall, body)
        }
    }

    /// The reply to `call`, with `body`.
    pub(crate) fn method_return(call: &Message, body: Body) -> Message {
        Message {
            reply_serial: Some(call.serial),
            destination: call.sender.clone(),
            ..Message::new(Kind::MethodReturn, body)
        }
    }

    /// The error `name` that answers `call`, with `text` for whoever reads it.
    pub(crate) fn error(call: &Message, name: &str, text: &str) -> Message {
        let body = Body::new("s", text).unwrap_or_default();
        Message {
            reply_serial: Some(call.serial),
            destination: call.sender.clone(),
            error_name: Some(name.to_owned()),
            ..Message::new(Kind::Error, body)
        }
    }

    /// The signal `member` of `interface`, from the object `path`.
    #[cfg(test)]
    pub(crate) fn signal(path: &str, interface: &str, member: &str, body: Body) -> Message {
        Message {
            path: Some(path.to_owned()),
            interface: Some(interface.to_owned()),
            member: Some(member.to_owned()),
            ..Message::new(Kind::Signal, body)
        }
    }

    /// Its body's signature: as written for a message Yuna made; as zbus parsed it for one
    /// read, which writes one structure, `(so)`, as two arguments, `so`. Empty without
    /// arguments.
    pub(crate) fn signature(&self) -> &str {
        &self.body.text
    }

    /// Whether its sender waits for a reply: a call without [`NO_REPLY_EXPECTED`].
    pub(crate) fn expects_reply(&self) -> bool {
        self.kind == Kind::MethodCall && self.flags & NO_REPLY_EXPECTED == 0
    }

    /// Its body, decoded as `T`, when its signature is `signature`: a tuple for several
    /// arguments (`sss`), one value for one (`(so)`), outer parentheses aside, since their bytes
    /// are the same.
    pub(crate) fn body<T: DeserializeOwned>(&self, signature: &str) -> Result<T, MessageError> {
        if self.body.parsed != signature {
            return Err(malformed(format!(
                "a body of signature {:?}, not {signature:?}",
                self.signature()
            )));
        }
        let data = Data::new(&self.body.bytes[..], Context::new(self.body.endian, 0));
        let (value, read) = data
            .deserialize_for_signature::<_, T>(&self.body.parsed)
            .map_err(malformed)?;
        if read != self.body.bytes.len() {
            return Err(malformed(format!(
                "{} bytes left after the body's arguments",
                self.body.bytes.len() - read
            )));
        }
        Ok(value)
    }

    /// The message's bytes, little-endian, with `serial`. Refused when a bus would disconnect
    /// Yuna for it ([`Message::check_header`]).
    pub(crate) fn encode(&self, serial: u32) -> Result<Vec<u8>, MessageError> {
        if serial == 0 {
            return Err(malformed("a serial of 0"));
        }
        if self.body.endian != Endian::Little {
            return Err(malformed("a big-endian body is never sent"));
        }
        self.check_header()?;
        let mut fields: Vec<(u8, Field<'_>)> = Vec::new();
        if let Some(path) = &self.path {
            let path = ObjectPath::try_from(path.as_str()).map_err(|error| {
                malformed(format!(
                    "path {} is not a valid object path: {error}",
                    shown(path)
                ))
            })?;
            fields.push((PATH, Field::Value(Variant::ObjectPath(path))));
        }
        let strings = [
            (INTERFACE, &self.interface),
            (MEMBER, &self.member),
            (ERROR_NAME, &self.error_name),
            (DESTINATION, &self.destination),
            (SENDER, &self.sender),
        ];
        for (code, value) in strings {
            if let Some(value) = value {
                fields.push((code, Field::Value(Variant::Str(Str::from(value.as_str())))));
            }
        }
        if let Some(reply_serial) = self.reply_serial {
            fields.push((REPLY_SERIAL, Field::Value(Variant::U32(reply_serial))));
        }
        if !self.body.text.is_empty() {
            fields.push((SIGNATURE, Field::Signature(&self.body.text)));
        }
        let body_length = u32::try_from(self.body.bytes.len())
            .map_err(|_| MessageError::TooLong(self.body.bytes.len()))?;
        let header = (
            b'l',
            self.kind.code(),
            self.flags,
            1_u8,
            body_length,
            serial,
            fields,
        );
        let encoded = zbus::wire::to_bytes_for_signature(
            Context::new(Endian::Little, 0),
            "(yyyyuua(yv))",
            &header,
        )
        .map_err(malformed)?;
        let mut bytes = encoded.bytes().to_vec();
        bytes.resize(bytes.len().next_multiple_of(8), 0);
        bytes.extend_from_slice(&self.body.bytes);
        if bytes.len() > MAX_MESSAGE {
            return Err(MessageError::TooLong(bytes.len()));
        }
        Ok(bytes)
    }

    /// Refuses a header a bus would disconnect Yuna for (the specification's *Header Fields*
    /// and *Valid Names*): a kind without the fields it must have, a reply serial of 0, a name
    /// that breaks its rules, the path or the interface the bus reserves. Its path's own rules
    /// are checked as it is encoded.
    fn check_header(&self) -> Result<(), MessageError> {
        if !self.has_required_fields() {
            return Err(malformed(format!(
                "a {:?} without the fields it must have",
                self.kind
            )));
        }
        if self.reply_serial == Some(0) {
            return Err(malformed("a reply serial of 0"));
        }
        if self.path.as_deref() == Some(LOCAL_PATH) {
            return Err(malformed(format!(
                "path {LOCAL_PATH} is reserved: a bus disconnects whoever sends it"
            )));
        }
        if self.interface.as_deref() == Some(LOCAL_INTERFACE) {
            return Err(malformed(format!(
                "interface {LOCAL_INTERFACE} is reserved: a bus disconnects whoever sends it"
            )));
        }
        type Check = fn(&str) -> Result<(), &'static str>;
        let named: [(&str, &Option<String>, &str, Check); 5] = [
            (
                "destination",
                &self.destination,
                "bus name",
                names::check_bus_name,
            ),
            ("sender", &self.sender, "bus name", names::check_bus_name),
            (
                "interface",
                &self.interface,
                "interface name",
                names::check_interface_name,
            ),
            (
                "member",
                &self.member,
                "member name",
                names::check_member_name,
            ),
            // An error's name has an interface's rules.
            (
                "error name",
                &self.error_name,
                "error name",
                names::check_interface_name,
            ),
        ];
        for (field, value, kind, check) in named {
            if let Some(value) = value
                && let Err(why) = check(value)
            {
                return Err(malformed(format!(
                    "{field} {} is not a valid {kind}: {why}",
                    shown(value)
                )));
            }
        }
        Ok(())
    }

    /// Whether it has the fields its kind must have: a call its path and member, a signal its
    /// path, interface and member, an error its name and reply serial, a reply its reply serial.
    fn has_required_fields(&self) -> bool {
        match self.kind {
            Kind::MethodCall => self.path.is_some() && self.member.is_some(),
            Kind::Signal => {
                self.path.is_some() && self.interface.is_some() && self.member.is_some()
            }
            Kind::Error => self.error_name.is_some() && self.reply_serial.is_some(),
            Kind::MethodReturn => self.reply_serial.is_some(),
        }
    }

    /// The length of the whole message that `prefix`, its first [`FRAME_PREFIX`] bytes or more,
    /// begins: none while fewer have come. Refused past [`MAX_MESSAGE`] and for a byte order
    /// that is neither `l` nor `B`, which leave the stream impossible to follow.
    pub(crate) fn frame_length(prefix: &[u8]) -> Result<Option<usize>, MessageError> {
        let Some(fixed) = prefix.get(..FRAME_PREFIX) else {
            return Ok(None);
        };
        let read = |at: usize| -> u32 {
            let word = [fixed[at], fixed[at + 1], fixed[at + 2], fixed[at + 3]];
            if fixed[0] == b'B' {
                u32::from_be_bytes(word)
            } else {
                u32::from_le_bytes(word)
            }
        };
        if fixed[0] != b'l' && fixed[0] != b'B' {
            return Err(malformed(format!("a byte order of {:#04x}", fixed[0])));
        }
        let body = read(4) as usize;
        let fields = read(12) as usize;
        let length = (FRAME_PREFIX as u64 + fields as u64).next_multiple_of(8) + body as u64;
        match usize::try_from(length) {
            Ok(length) if length <= MAX_MESSAGE => Ok(Some(length)),
            _ => Err(MessageError::TooLong(
                usize::try_from(length).unwrap_or(usize::MAX),
            )),
        }
    }

    /// Decodes one whole message, `bytes`, as [`Message::frame_length`] found it.
    pub(crate) fn decode(bytes: &[u8]) -> Result<Message, MessageError> {
        let length = Message::frame_length(bytes)?
            .ok_or_else(|| malformed("shorter than a header's fixed part"))?;
        if bytes.len() != length {
            return Err(malformed(format!(
                "{} bytes for a message of {length}",
                bytes.len()
            )));
        }
        let endian = if bytes[0] == b'B' {
            Endian::Big
        } else {
            Endian::Little
        };
        type Header = (u8, u8, u8, u8, u32, u32, Vec<(u8, OwnedVariant)>);
        let data = Data::new(bytes, Context::new(endian, 0));
        let ((_, kind, flags, version, body_length, serial, fields), header_end): (Header, usize) =
            data.deserialize_for_signature("(yyyyuua(yv))")
                .map_err(malformed)?;
        if version != 1 {
            return Err(malformed(format!("protocol version {version}")));
        }
        if serial == 0 {
            return Err(malformed("a serial of 0"));
        }
        let kind = match kind {
            1 => Kind::MethodCall,
            2 => Kind::MethodReturn,
            3 => Kind::Error,
            4 => Kind::Signal,
            other => return Err(MessageError::UnknownKind(other)),
        };
        let body_start = header_end.next_multiple_of(8);
        if bytes.len() - body_start != body_length as usize {
            return Err(malformed("a body whose length is not the header's"));
        }
        if bytes[header_end..body_start].iter().any(|&byte| byte != 0) {
            return Err(malformed("padding that is not zero"));
        }
        let mut message = Message::new(kind, Body::empty());
        message.flags = flags;
        message.serial = serial;
        let mut signature = Signature::Unit;
        for (code, value) in fields {
            let value: &Variant<'_> = &value;
            let text = || match value {
                Variant::Str(text) => Ok(text.as_str().to_owned()),
                _ => Err(malformed(format!("header field {code} is not a string"))),
            };
            match code {
                PATH => match value {
                    Variant::ObjectPath(path) => message.path = Some(path.as_str().to_owned()),
                    _ => return Err(malformed("a path that is not an object path")),
                },
                INTERFACE => message.interface = Some(text()?),
                MEMBER => message.member = Some(text()?),
                ERROR_NAME => message.error_name = Some(text()?),
                DESTINATION => message.destination = Some(text()?),
                SENDER => message.sender = Some(text()?),
                REPLY_SERIAL => match value {
                    Variant::U32(serial) if *serial != 0 => message.reply_serial = Some(*serial),
                    _ => return Err(malformed("a reply serial that is not a nonzero u32")),
                },
                SIGNATURE => match value {
                    Variant::Signature(parsed) => signature = parsed.clone(),
                    _ => return Err(malformed("a signature field that is not a signature")),
                },
                UNIX_FDS => match value {
                    Variant::U32(0) => {}
                    _ => return Err(malformed("file descriptors, which Yuna never takes")),
                },
                // Fields the specification may add later are ignored, as it asks.
                _ => {}
            }
        }
        if !message.has_required_fields() {
            return Err(malformed(format!(
                "a {kind:?} without the fields it must have"
            )));
        }
        // Every type takes a byte at least: a body and a signature come together or not at all.
        if (signature == Signature::Unit) != (body_length == 0) {
            return Err(malformed("a body and its signature do not match"));
        }
        message.body = Body {
            text: signature.to_string_no_parens(),
            parsed: signature,
            bytes: bytes[body_start..].to_vec(),
            endian,
        };
        Ok(message)
    }

    /// An error's text, the first argument its sender gave, when it is one string.
    pub(crate) fn error_text(&self) -> String {
        let name = self.error_name.clone().unwrap_or_default();
        match self.body::<String>("s") {
            Ok(text) => format!("{name}: {text}"),
            Err(_) => name,
        }
    }
}
