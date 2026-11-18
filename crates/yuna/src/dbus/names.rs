//! The specification's rules for the names a message's header carries
//! ([Valid Names](https://dbus.freedesktop.org/doc/dbus-specification.html#message-protocol-names)).
//! A bus disconnects whoever sends a message that breaks them, so that
//! [`Message::encode`](super::Message::encode) checks each name before Yuna sends it: names come
//! from programs too, the bus names and object paths a toolkit gives the registry, which Yuna's
//! walks then call.
//!
//! Each check is the specification's rule. Where a bus is looser, Yuna refuses what the bus would
//! take, never the reverse; the names at the edge of each rule are sent to a real `dbus-daemon`
//! in the tests.

/// The longest bus name, interface, member or error name.
pub(super) const MAX_NAME: usize = 255;

/// The object path the bus reserves: it disconnects whoever sends a message with it.
pub(super) const LOCAL_PATH: &str = "/org/freedesktop/DBus/Local";

/// The interface the bus reserves, which it disconnects a sender for too.
pub(super) const LOCAL_INTERFACE: &str = "org.freedesktop.DBus.Local";

/// Whether `name` is a bus name, a destination's or a sender's: a unique name, `:` then two
/// elements or more, or a well-known name of two elements or more, none beginning with a digit;
/// each element one character or more of `[A-Za-z0-9_-]`, joined by `.`; 255 bytes at most. The
/// error says what is wrong.
pub(super) fn check_bus_name(name: &str) -> Result<(), &'static str> {
    check_length(name)?;
    let is_allowed = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-';
    match name.strip_prefix(':') {
        Some(elements) => check_elements(elements, is_allowed, true),
        None => check_elements(name, is_allowed, false),
    }
}

/// Whether `name` is an interface name, or an error name, whose rules are the same: two elements
/// or more of `[A-Za-z0-9_]`, joined by `.`, none empty or beginning with a digit; 255 bytes at
/// most. The error says what is wrong.
pub(super) fn check_interface_name(name: &str) -> Result<(), &'static str> {
    check_length(name)?;
    check_elements(name, is_name_byte, false)
}

/// Whether `name` is a member name, a method's or a signal's: one element of `[A-Za-z0-9_]`,
/// not beginning with a digit, without `.`; 255 bytes at most. The error says what is wrong.
pub(super) fn check_member_name(name: &str) -> Result<(), &'static str> {
    check_length(name)?;
    check_element(name, is_name_byte, false)
}

/// What an interface's, a member's or an error's name may hold.
fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn check_length(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("it is empty");
    }
    if name.len() > MAX_NAME {
        return Err("it is longer than 255 bytes");
    }
    Ok(())
}

/// Whether `elements` are two or more, joined by `.`, each a valid element.
fn check_elements(
    elements: &str,
    is_allowed: impl Fn(u8) -> bool,
    digit_first: bool,
) -> Result<(), &'static str> {
    let mut count = 0;
    for element in elements.split('.') {
        check_element(element, &is_allowed, digit_first)?;
        count += 1;
    }
    if count < 2 {
        return Err("it has one element, not two or more joined by '.'");
    }
    Ok(())
}

/// Whether `element` is one character or more that `is_allowed` takes, beginning with a digit
/// only when `digit_first`.
fn check_element(
    element: &str,
    is_allowed: impl Fn(u8) -> bool,
    digit_first: bool,
) -> Result<(), &'static str> {
    let Some(first) = element.bytes().next() else {
        return Err("it has an empty element");
    };
    if !digit_first && first.is_ascii_digit() {
        return Err("an element begins with a digit");
    }
    if !element.bytes().all(is_allowed) {
        return Err("it holds a character no such name may hold");
    }
    Ok(())
}
