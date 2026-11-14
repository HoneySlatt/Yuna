//! What Yuna's X11 window manager reads of a window's properties, decoded from the bytes
//! `GetProperty` gives (ICCCM 2.0, EWMH 1.5, and Xwayland's and GTK's own). The manager's thread
//! reads them (`manager.rs`); Yuna's loop acts on the result (`windows.rs`).
//!
//! Every read is bounded, so that one X11 client cannot make the manager copy without end:
//! [`MAX_TEXT_BYTES`] for text, [`MAX_LIST`] atoms for a list, and for a property of fixed
//! size, that size. A value of the wrong type or format, or whose numbers make no sense, is
//! malformed: it is ignored, as if the property were not set, and the manager says so once per
//! window. Text beyond the bound is cut, at a character's edge.

use std::fmt;

/// The most bytes of text read from a title or a class: 64 KiB, far beyond any title a window
/// shows, and small enough that a client changing its title again and again costs little.
pub(crate) const MAX_TEXT_BYTES: u32 = 64 * 1024;

/// The most atoms read from a list (`WM_PROTOCOLS`, `_NET_WM_STATE`, `_NET_WM_WINDOW_TYPE`): the
/// specifications name about twenty, a client lists a few.
pub(crate) const MAX_LIST: u32 = 64;

/// The most rectangles read from `_XWAYLAND_RANDR_EMU_MONITOR_RECTS`: Xwayland keeps 16 emulated
/// modes per client (`XWL_CLIENT_MAX_EMULATED_MODES`).
pub(crate) const MAX_EMULATED: u32 = 16;

/// The widest frame extent taken from `_GTK_FRAME_EXTENTS`, in X11 pixels: a shadow is tens of
/// pixels; X11's sizes stop at 32,767.
pub(crate) const MAX_FRAME_EXTENT: u32 = 4_096;

/// The largest size X11 gives a window, on each axis: sizes are 16-bit.
pub(crate) const MAX_X11_SIDE: i32 = 32_767;

/// A rectangle in X11 pixels, relative to the root's origin.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct X11Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Everything Yuna reads of a window, as its properties say now. A property not set, or
/// malformed, leaves its field at its default.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Properties {
    /// `_NET_WM_NAME`, in UTF-8.
    pub net_wm_name: Option<String>,
    /// `WM_NAME`, in any of ICCCM's encodings.
    pub wm_name: Option<String>,
    /// `WM_CLASS`'s class.
    pub class: Option<String>,
    /// `WM_NORMAL_HINTS`.
    pub size_hints: SizeHints,
    /// `WM_HINTS`' input field: whether it takes the focus itself (ICCCM 4.1.7); unknown, it
    /// does.
    pub accepts_input: Option<bool>,
    /// `WM_PROTOCOLS`.
    pub protocols: Protocols,
    /// `WM_TRANSIENT_FOR`: the window it is a dialog of.
    pub transient_for: Option<u32>,
    /// `_NET_WM_WINDOW_TYPE`.
    pub window_type: WindowType,
    /// `_MOTIF_WM_HINTS`' decorations: read, never followed (ADR-0024).
    pub decorations: Option<bool>,
    /// `_GTK_FRAME_EXTENTS`: none set is all zero.
    pub frame_extents: FrameExtents,
    /// `_NET_WM_PID`, as the client says it.
    pub pid: Option<i32>,
    /// What `_NET_WM_STATE` asks before the window is mapped: Yuna sets it from then on.
    pub wishes: Wishes,
    /// `_XWAYLAND_RANDR_EMU_MONITOR_RECTS`.
    pub emulated: Vec<X11Rect>,
}

impl Properties {
    /// Its title: `_NET_WM_NAME`, else `WM_NAME`.
    pub(crate) fn title(&self) -> Option<&str> {
        self.net_wm_name.as_deref().or(self.wm_name.as_deref())
    }
}

/// The protocols of `WM_PROTOCOLS` the manager knows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Protocols {
    /// `WM_DELETE_WINDOW`: it closes when asked, rather than having its client ended.
    pub delete_window: bool,
    /// `WM_TAKE_FOCUS`: it is told when it may take the focus (ICCCM 4.1.7).
    pub take_focus: bool,
    /// `_NET_WM_PING`: it answers pings; Yuna sends none yet.
    pub ping: bool,
}

/// What a window's `_NET_WM_STATE` asks before it is mapped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Wishes {
    /// `_NET_WM_STATE_FULLSCREEN`.
    pub fullscreen: bool,
    /// `_NET_WM_STATE_MAXIMIZED_HORZ`: full width in Yuna's layout.
    pub maximized: bool,
}

/// A property's value, as `GetProperty` gave it: its type, its format, its bytes in the
/// connection's byte order, and whether more was left unread past the bound.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Raw<'a> {
    pub type_: u32,
    pub format: u8,
    pub value: &'a [u8],
    pub truncated: bool,
}

impl Raw<'_> {
    /// Whether the property is not set: `GetProperty` gives the type None.
    fn is_unset(&self) -> bool {
        self.type_ == 0
    }

    /// Its 32-bit values, for format 32; none for another format.
    fn words(&self) -> Option<Vec<u32>> {
        if self.format != 32 {
            return None;
        }
        let (words, rest) = self.value.as_chunks::<4>();
        rest.is_empty()
            .then(|| words.iter().map(|word| u32::from_ne_bytes(*word)).collect())
    }
}

/// Why a property's value is ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Malformed(pub &'static str);

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// A decoded value, none when the property is not set, or why it is ignored.
pub(crate) type Decoded<T> = Result<Option<T>, Malformed>;

/// The text encodings a property's type names, as the manager's atoms number them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TextTypes {
    pub utf8_string: u32,
    pub string: u32,
    pub compound_text: u32,
}

/// A title, read from `_NET_WM_NAME` or `WM_NAME`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Text {
    pub text: String,
    /// Whether it was `COMPOUND_TEXT`, read as Latin-1: right for ASCII and Latin-1, wrong for
    /// the other character sets it can switch to, which the manager logs.
    pub compound: bool,
}

/// Text in one of the encodings ICCCM allows: `UTF8_STRING`; `STRING`, Latin-1; `COMPOUND_TEXT`,
/// read as Latin-1. Trailing NULs are dropped; UTF-8 cut by the bound ends at the last whole
/// character. Invalid UTF-8, another type or another format is malformed.
pub(crate) fn decode_text(raw: Raw<'_>, types: TextTypes) -> Decoded<Text> {
    if raw.is_unset() {
        return Ok(None);
    }
    if raw.format != 8 {
        return Err(Malformed("a text property not of format 8"));
    }
    let bytes = trim_nuls(raw.value);
    let (text, compound) = if raw.type_ == types.utf8_string {
        (utf8(bytes, raw.truncated)?, false)
    } else if raw.type_ == types.string {
        (latin1(bytes), false)
    } else if raw.type_ == types.compound_text {
        (latin1(bytes), true)
    } else {
        return Err(Malformed("a text property of an unknown encoding"));
    };
    Ok(Some(Text { text, compound }))
}

/// `WM_CLASS`'s class, the second of its two NUL-ended Latin-1 strings, which Yuna gives as the
/// window's application id. A value with no NUL holds no class.
pub(crate) fn decode_class(raw: Raw<'_>, string: u32) -> Decoded<String> {
    if raw.is_unset() {
        return Ok(None);
    }
    if raw.type_ != string || raw.format != 8 {
        return Err(Malformed("WM_CLASS not a STRING of format 8"));
    }
    let mut parts = raw.value.split(|&byte| byte == 0);
    let _instance = parts.next();
    let class = parts.next().filter(|class| !class.is_empty());
    Ok(class.map(latin1))
}

/// What `WM_NORMAL_HINTS` gives, in X11 pixels: each a width and a height.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SizeHints {
    pub min: Option<(u32, u32)>,
    pub max: Option<(u32, u32)>,
    pub base: Option<(u32, u32)>,
    pub increment: Option<(u32, u32)>,
}

/// `WM_NORMAL_HINTS`, a `WM_SIZE_HINTS` of 15 or 18 values (ICCCM 4.1.2.3): the flags, four
/// unused, the minimum, the maximum, the increments, the aspects, then the base size and the
/// gravity. A negative minimum or base is malformed; a maximum or an increment of 0 or less means
/// none. Sizes are cut to X11's largest. Without a minimum, the base stands for it, as ICCCM says.
pub(crate) fn decode_size_hints(raw: Raw<'_>, size_hints: u32) -> Decoded<SizeHints> {
    const MIN: u32 = 1 << 4;
    const MAX: u32 = 1 << 5;
    const INCREMENT: u32 = 1 << 6;
    const BASE: u32 = 1 << 8;
    if raw.is_unset() {
        return Ok(None);
    }
    let words = raw
        .words()
        .filter(|words| raw.type_ == size_hints && words.len() >= 15)
        .ok_or(Malformed(
            "WM_NORMAL_HINTS not a WM_SIZE_HINTS of 15 values",
        ))?;
    let flags = words[0];
    let pair = |at: usize| -> Option<(i32, i32)> {
        let w = *words.get(at)? as i32;
        let h = *words.get(at + 1)? as i32;
        Some((w, h))
    };
    let side = |value: i32| value.min(MAX_X11_SIDE) as u32;
    let sized = |(w, h): (i32, i32)| (side(w), side(h));
    let mut hints = SizeHints::default();
    if flags & MIN != 0 {
        let min = pair(5).ok_or(Malformed("WM_NORMAL_HINTS too short"))?;
        if min.0 < 0 || min.1 < 0 {
            return Err(Malformed("a negative minimum size"));
        }
        hints.min = Some(sized(min));
    }
    if flags & MAX != 0 {
        let max = pair(7).ok_or(Malformed("WM_NORMAL_HINTS too short"))?;
        hints.max = (max.0 > 0 && max.1 > 0).then(|| sized(max));
    }
    if flags & INCREMENT != 0 {
        let increment = pair(9).ok_or(Malformed("WM_NORMAL_HINTS too short"))?;
        hints.increment = (increment.0 > 0 && increment.1 > 0).then(|| sized(increment));
    }
    if flags & BASE != 0
        && let Some(base) = pair(15)
    {
        if base.0 < 0 || base.1 < 0 {
            return Err(Malformed("a negative base size"));
        }
        hints.base = Some(sized(base));
    }
    if hints.min.is_none() {
        hints.min = hints.base;
    }
    Ok(Some(hints))
}

/// `WM_HINTS`' input field (ICCCM 4.1.2.4, 4.1.7): whether the window takes the input focus
/// itself, its InputHint flag set; none without the flag.
pub(crate) fn decode_input_hint(raw: Raw<'_>, wm_hints: u32) -> Decoded<bool> {
    const INPUT: u32 = 1;
    if raw.is_unset() {
        return Ok(None);
    }
    let words = raw
        .words()
        .filter(|words| raw.type_ == wm_hints && !words.is_empty())
        .ok_or(Malformed("WM_HINTS not a WM_HINTS of format 32"))?;
    if words[0] & INPUT == 0 {
        return Ok(None);
    }
    let input = words.get(1).ok_or(Malformed("WM_HINTS too short"))?;
    Ok(Some(*input != 0))
}

/// A list of atoms: `WM_PROTOCOLS`, `_NET_WM_STATE`, `_NET_WM_WINDOW_TYPE`.
pub(crate) fn decode_atoms(raw: Raw<'_>, atom: u32) -> Decoded<Vec<u32>> {
    if raw.is_unset() {
        return Ok(None);
    }
    raw.words()
        .filter(|_| raw.type_ == atom)
        .map(Some)
        .ok_or(Malformed("an atom list not of type ATOM and format 32"))
}

/// One window, or none: `WM_TRANSIENT_FOR`. A window of 0 is none.
pub(crate) fn decode_window(raw: Raw<'_>, window: u32) -> Decoded<u32> {
    if raw.is_unset() {
        return Ok(None);
    }
    let words = raw
        .words()
        .filter(|words| raw.type_ == window && !words.is_empty())
        .ok_or(Malformed("WM_TRANSIENT_FOR not a WINDOW"))?;
    Ok(Some(words[0]).filter(|&window| window != 0))
}

/// `_NET_WM_PID`, a process id; 0, or one beyond Linux's ids, is malformed.
pub(crate) fn decode_pid(raw: Raw<'_>, cardinal: u32) -> Decoded<i32> {
    if raw.is_unset() {
        return Ok(None);
    }
    let pid = raw
        .words()
        .filter(|words| raw.type_ == cardinal && !words.is_empty())
        .map(|words| words[0])
        .ok_or(Malformed("_NET_WM_PID not a CARDINAL"))?;
    i32::try_from(pid)
        .ok()
        .filter(|&pid| pid > 0)
        .map(Some)
        .ok_or(Malformed("_NET_WM_PID out of range"))
}

/// `_MOTIF_WM_HINTS`' decorations: whether the window wants its manager's decorations, when its
/// flags say (bit 1). Yuna draws none whatever it says (ADR-0024).
pub(crate) fn decode_motif_decorations(raw: Raw<'_>, motif_hints: u32) -> Decoded<bool> {
    const DECORATIONS: u32 = 1 << 1;
    if raw.is_unset() {
        return Ok(None);
    }
    let words = raw
        .words()
        .filter(|words| raw.type_ == motif_hints && words.len() >= 3)
        .ok_or(Malformed("_MOTIF_WM_HINTS not of 5 values"))?;
    Ok((words[0] & DECORATIONS != 0).then_some(words[2] != 0))
}

/// The margins of a window's shadow inside its X11 window, in X11 pixels: what lies between the
/// X11 window's edges and the window its user sees.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FrameExtents {
    pub left: u32,
    pub right: u32,
    pub top: u32,
    pub bottom: u32,
}

/// `_GTK_FRAME_EXTENTS`: four CARDINALs, left, right, top and bottom, each at most
/// [`MAX_FRAME_EXTENT`].
pub(crate) fn decode_frame_extents(raw: Raw<'_>, cardinal: u32) -> Decoded<FrameExtents> {
    if raw.is_unset() {
        return Ok(None);
    }
    let words = raw
        .words()
        .filter(|words| raw.type_ == cardinal && words.len() == 4)
        .ok_or(Malformed("_GTK_FRAME_EXTENTS not of 4 CARDINALs"))?;
    if words.iter().any(|&extent| extent > MAX_FRAME_EXTENT) {
        return Err(Malformed("a frame extent beyond its bound"));
    }
    Ok(Some(FrameExtents {
        left: words[0],
        right: words[1],
        top: words[2],
        bottom: words[3],
    }))
}

/// `_XWAYLAND_RANDR_EMU_MONITOR_RECTS`, which Xwayland sets on a client's windows: for each
/// output whose mode the client emulates, the output's X11 position and the mode's size
/// (`xwayland-output.c`), four CARDINALs each. An empty or zero-sized rectangle is malformed.
pub(crate) fn decode_emulated(raw: Raw<'_>, cardinal: u32) -> Decoded<Vec<X11Rect>> {
    if raw.is_unset() {
        return Ok(None);
    }
    let words = raw
        .words()
        .filter(|words| raw.type_ == cardinal && words.len() % 4 == 0 && !words.is_empty())
        .ok_or(Malformed(
            "_XWAYLAND_RANDR_EMU_MONITOR_RECTS not of 4 CARDINALs each",
        ))?;
    let rects: Vec<X11Rect> = words
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&[x, y, width, height]| X11Rect {
            // Xwayland writes its outputs' signed positions into CARDINALs.
            x: x as i32,
            y: y as i32,
            width,
            height,
        })
        .collect();
    if rects
        .iter()
        .any(|rect| rect.width == 0 || rect.height == 0 || rect.width > MAX_X11_SIDE as u32)
    {
        return Err(Malformed("an emulated mode of no size"));
    }
    Ok(Some(rects))
}

/// A window's type, from `_NET_WM_WINDOW_TYPE`, its first type Yuna knows (EWMH lists them by
/// preference); normal without one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum WindowType {
    #[default]
    Normal,
    Dialog,
    Utility,
    Toolbar,
    Splash,
    /// A menu, a combo box's list, a drag's icon, a tooltip or a notification: what
    /// override-redirect windows usually are.
    Transient,
    /// A panel: the shell's, never a window of the layout.
    Dock,
    /// A desktop's background: never a window of the layout.
    Desktop,
}

impl WindowType {
    /// Whether a window of this type is a window of the layout when it asks to be mapped: not a
    /// dock nor a desktop.
    pub(crate) fn is_managed(self) -> bool {
        !matches!(self, WindowType::Dock | WindowType::Desktop)
    }
}

/// Bytes without their trailing NULs, which some clients count in their text's length.
fn trim_nuls(bytes: &[u8]) -> &[u8] {
    let end = bytes
        .iter()
        .rposition(|&byte| byte != 0)
        .map_or(0, |last| last + 1);
    &bytes[..end]
}

/// Latin-1 bytes as text: each byte is the character of its number.
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&byte| char::from(byte)).collect()
}

/// UTF-8 bytes as text; when `truncated`, a character the bound cut is dropped. Invalid UTF-8 is
/// malformed.
fn utf8(bytes: &[u8], truncated: bool) -> Result<String, Malformed> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok(text.to_owned()),
        Err(error) if truncated && error.error_len().is_none() => {
            Ok(String::from_utf8_lossy(&bytes[..error.valid_up_to()]).into_owned())
        }
        Err(_) => Err(Malformed("invalid UTF-8")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UTF8: u32 = 300;
    const STRING: u32 = 31;
    const COMPOUND: u32 = 301;
    const TYPES: TextTypes = TextTypes {
        utf8_string: UTF8,
        string: STRING,
        compound_text: COMPOUND,
    };
    const CARDINAL: u32 = 6;
    const SIZE_HINTS: u32 = 41;

    fn text(type_: u32, value: &[u8]) -> Raw<'_> {
        Raw {
            type_,
            format: 8,
            value,
            truncated: false,
        }
    }

    fn words_of(values: &[u32]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_ne_bytes())
            .collect()
    }

    fn of32(type_: u32, bytes: &[u8]) -> Raw<'_> {
        Raw {
            type_,
            format: 32,
            value: bytes,
            truncated: false,
        }
    }

    #[test]
    fn titles_are_read_in_each_encoding() {
        let read = |raw| decode_text(raw, TYPES).map(|text| text.map(|text| text.text));
        assert_eq!(
            read(text(UTF8, "Café ☕".as_bytes())),
            Ok(Some("Café ☕".to_owned()))
        );
        // Latin-1's é is one byte, 0xE9.
        assert_eq!(
            read(text(STRING, b"Caf\xE9\0")),
            Ok(Some("Café".to_owned()))
        );
        let compound = decode_text(text(COMPOUND, b"xterm"), TYPES).expect("read");
        assert_eq!(
            compound,
            Some(Text {
                text: "xterm".to_owned(),
                compound: true
            })
        );
        assert_eq!(read(text(0, b"")), Ok(None));
    }

    #[test]
    fn a_malformed_title_is_ignored() {
        let read = |raw| decode_text(raw, TYPES);
        assert!(read(text(UTF8, b"bad \xFF here")).is_err());
        assert!(read(text(999, b"title")).is_err());
        let mut sixteen = text(UTF8, b"ab");
        sixteen.format = 16;
        assert!(read(sixteen).is_err());
    }

    #[test]
    fn utf8_cut_by_the_bound_ends_at_a_whole_character() {
        // "é" is two bytes; the bound kept its first.
        let raw = Raw {
            truncated: true,
            ..text(UTF8, b"caf\xC3")
        };
        assert_eq!(
            decode_text(raw, TYPES).map(|text| text.map(|text| text.text)),
            Ok(Some("caf".to_owned()))
        );
        // Not cut, the same bytes are malformed.
        assert!(decode_text(text(UTF8, b"caf\xC3"), TYPES).is_err());
    }

    #[test]
    fn the_class_is_the_second_string_of_wm_class() {
        assert_eq!(
            decode_class(text(STRING, b"xterm\0XTerm\0"), STRING),
            Ok(Some("XTerm".to_owned()))
        );
        assert_eq!(
            decode_class(text(STRING, b"xterm\0XTerm"), STRING),
            Ok(Some("XTerm".to_owned()))
        );
        assert_eq!(decode_class(text(STRING, b"xterm"), STRING), Ok(None));
        assert!(decode_class(text(UTF8, b"a\0b\0"), STRING).is_err());
    }

    #[test]
    fn size_hints_give_their_flagged_fields() {
        // Minimum 100 × 50, maximum 800 × 600, increments 6 × 13, base 4 × 2.
        let mut values = vec![0_u32; 18];
        values[0] = (1 << 4) | (1 << 5) | (1 << 6) | (1 << 8);
        values[5..11].copy_from_slice(&[100, 50, 800, 600, 6, 13]);
        values[15..17].copy_from_slice(&[4, 2]);
        let bytes = words_of(&values);
        assert_eq!(
            decode_size_hints(of32(SIZE_HINTS, &bytes), SIZE_HINTS),
            Ok(Some(SizeHints {
                min: Some((100, 50)),
                max: Some((800, 600)),
                base: Some((4, 2)),
                increment: Some((6, 13)),
            }))
        );
        // Without a minimum, the base stands for it; a maximum of 0 is none.
        values[0] = (1 << 5) | (1 << 8);
        values[7] = 0;
        let bytes = words_of(&values);
        let hints = decode_size_hints(of32(SIZE_HINTS, &bytes), SIZE_HINTS)
            .expect("read")
            .expect("set");
        assert_eq!((hints.min, hints.max), (Some((4, 2)), None));
        // The 15 values of old clients: no base.
        let bytes = words_of(&values[..15]);
        let hints = decode_size_hints(of32(SIZE_HINTS, &bytes), SIZE_HINTS)
            .expect("read")
            .expect("set");
        assert_eq!(hints.base, None);
    }

    #[test]
    fn malformed_size_hints_are_ignored() {
        let mut values = vec![0_u32; 18];
        values[0] = 1 << 4;
        values[5] = (-5_i32) as u32;
        let bytes = words_of(&values);
        assert!(decode_size_hints(of32(SIZE_HINTS, &bytes), SIZE_HINTS).is_err());
        let short = words_of(&[0; 10]);
        assert!(decode_size_hints(of32(SIZE_HINTS, &short), SIZE_HINTS).is_err());
        assert!(decode_size_hints(of32(CARDINAL, &bytes), SIZE_HINTS).is_err());
        // Huge sizes are cut to X11's largest.
        values[5] = u32::MAX / 2;
        let bytes = words_of(&values);
        let hints = decode_size_hints(of32(SIZE_HINTS, &bytes), SIZE_HINTS)
            .expect("read")
            .expect("set");
        assert_eq!(hints.min, Some((MAX_X11_SIDE as u32, 0)));
    }

    #[test]
    fn the_input_hint_is_read_only_when_flagged() {
        const WM_HINTS: u32 = 35;
        let set = words_of(&[1, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            decode_input_hint(of32(WM_HINTS, &set), WM_HINTS),
            Ok(Some(false))
        );
        let unflagged = words_of(&[0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            decode_input_hint(of32(WM_HINTS, &unflagged), WM_HINTS),
            Ok(None)
        );
        let short = words_of(&[1]);
        assert!(decode_input_hint(of32(WM_HINTS, &short), WM_HINTS).is_err());
    }

    #[test]
    fn frame_extents_are_bounded() {
        let extents = words_of(&[26, 26, 23, 29]);
        assert_eq!(
            decode_frame_extents(of32(CARDINAL, &extents), CARDINAL),
            Ok(Some(FrameExtents {
                left: 26,
                right: 26,
                top: 23,
                bottom: 29
            }))
        );
        let huge = words_of(&[26, 26, 23, 1_000_000]);
        assert!(decode_frame_extents(of32(CARDINAL, &huge), CARDINAL).is_err());
        let three = words_of(&[1, 2, 3]);
        assert!(decode_frame_extents(of32(CARDINAL, &three), CARDINAL).is_err());
    }

    #[test]
    fn emulated_modes_are_rectangles_at_their_outputs() {
        let rects = words_of(&[2880, 0, 1920, 1080, (-100_i32) as u32, 0, 800, 600]);
        assert_eq!(
            decode_emulated(of32(CARDINAL, &rects), CARDINAL),
            Ok(Some(vec![
                X11Rect {
                    x: 2880,
                    y: 0,
                    width: 1920,
                    height: 1080
                },
                X11Rect {
                    x: -100,
                    y: 0,
                    width: 800,
                    height: 600
                },
            ]))
        );
        let empty = words_of(&[0, 0, 0, 1080]);
        assert!(decode_emulated(of32(CARDINAL, &empty), CARDINAL).is_err());
        let ragged = words_of(&[0, 0, 1920]);
        assert!(decode_emulated(of32(CARDINAL, &ragged), CARDINAL).is_err());
    }

    #[test]
    fn small_values_are_read_and_checked() {
        const WINDOW: u32 = 33;
        const ATOM: u32 = 4;
        const MOTIF: u32 = 400;
        assert_eq!(
            decode_window(of32(WINDOW, &words_of(&[0x40_0001])), WINDOW),
            Ok(Some(0x40_0001))
        );
        assert_eq!(
            decode_window(of32(WINDOW, &words_of(&[0])), WINDOW),
            Ok(None)
        );
        assert_eq!(
            decode_pid(of32(CARDINAL, &words_of(&[4242])), CARDINAL),
            Ok(Some(4242))
        );
        assert!(decode_pid(of32(CARDINAL, &words_of(&[0])), CARDINAL).is_err());
        assert!(decode_pid(of32(CARDINAL, &words_of(&[u32::MAX])), CARDINAL).is_err());
        assert_eq!(
            decode_atoms(of32(ATOM, &words_of(&[7, 8])), ATOM),
            Ok(Some(vec![7, 8]))
        );
        assert!(decode_atoms(of32(CARDINAL, &words_of(&[7])), ATOM).is_err());
        // Decorations asked off, as a client-side decorated window does.
        let motif = words_of(&[2, 0, 0, 0, 0]);
        assert_eq!(
            decode_motif_decorations(of32(MOTIF, &motif), MOTIF),
            Ok(Some(false))
        );
        let unflagged = words_of(&[1, 0, 0, 0, 0]);
        assert_eq!(
            decode_motif_decorations(of32(MOTIF, &unflagged), MOTIF),
            Ok(None)
        );
        // A 32-bit value cut in its middle is malformed, whatever its type.
        assert!(decode_pid(of32(CARDINAL, &[1, 2, 3]), CARDINAL).is_err());
    }
}
