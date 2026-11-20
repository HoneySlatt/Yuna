//! An agent's seat's keymap: the XKB layout it is compiled from, and which keys type each keysym
//! through it ([ADR-0095](../../../docs/adr/0095-an-agents-seat-through-mcp-its-tools-input-keyboard-layouts-and-closing.md)).
//!
//! An agent's seat starts with the `us` layout, and its agent may change it at any time
//! (`State::set_keyboard_layout`): layouts, their variants and XKB's options, under the evdev
//! rules and the pc105 model, never the machine's `XKB_DEFAULT_*` variables. A layout or variant
//! XKB does not know fails to compile, and is refused. XKB ignores an option its rules do not
//! know, so Yuna reads the options the rules file names and refuses any other; where it finds no
//! rules file, every option passes to XKB.
//!
//! When a keymap is set, Yuna reads which keysym each of its keys types in each of its groups
//! (its layouts), alone or with the keys that pick a level held (Shift, AltGr for level 3, level
//! 5's key where the keymap has one), and how its keys lock another group: the keymap's own way,
//! such as `grp:alt_shift_toggle`'s Alt and Shift, else evdev's layout key. Everything is found by
//! running the keys through xkbcommon's own state, as Smithay's keyboard and every client run
//! them, so that what Yuna plans is what a client reads. Typing a keysym then takes the chord of
//! the fewest keys among a PC keyboard's keys first ([`Tier`]), never a key X11 does not carry:
//! in the keyboard's group if it is there, else after the chords that lock the nearest group that
//! has it ([`KeyPlanner`]). A tap or a press of a keysym takes a key that types it alone first,
//! in any tier ([`SeatKeymap::key_named`]). A key that reaches a group only while held, as
//! `grp:switch`'s, is never held: the group is locked.
//! AltGr is Right Alt where the layout puts level 3 on it, as `fr` and `de` do; else it is XKB's
//! fake key `<LVL3>`, which every evdev keymap has and no keyboard: a program that reads physical
//! keys, as Chromium, SDL and Wine do, knows Right Alt.

use std::collections::{HashMap, HashSet, VecDeque};

use smithay::input::keyboard::XkbConfig;
use xkbcommon::xkb::{self, Keycode, Keysym};

use crate::Error;

/// The rules and the keyboard model of every agent's seat: Linux's evdev keycodes, a PC keyboard
/// of 105 keys, the same on every machine and in CI.
const RULES: &str = "evdev";
const MODEL: &str = "pc105";

/// The longest a layout's field may be, in bytes: room for four layouts, their variants and a
/// line of options, and a bound on the work one change asks of XKB.
pub const MAX_LAYOUT_NAME: usize = 256;

/// The most layouts one keymap holds: XKB's groups, four in X11 and in the keymap's text format
/// that clients read.
const MAX_GROUPS: u32 = 4;

/// The keysyms of the keys that pick a level while held.
const LEVEL_KEYSYMS: [Keysym; 4] = [
    Keysym::Shift_L,
    Keysym::Shift_R,
    Keysym::ISO_Level3_Shift,
    Keysym::ISO_Level5_Shift,
];

/// The keysyms of every modifier key, which a group's chord may hold: `ctrl+shift`, `super+space`.
const MODIFIER_KEYSYMS: [Keysym; 14] = [
    Keysym::Shift_L,
    Keysym::Shift_R,
    Keysym::Control_L,
    Keysym::Control_R,
    Keysym::Alt_L,
    Keysym::Alt_R,
    Keysym::Meta_L,
    Keysym::Meta_R,
    Keysym::Super_L,
    Keysym::Super_R,
    Keysym::Hyper_L,
    Keysym::Hyper_R,
    Keysym::ISO_Level3_Shift,
    Keysym::ISO_Level5_Shift,
];

/// The keysyms that lock another group, which XKB's group options put on keys.
const GROUP_KEYSYMS: [Keysym; 9] = [
    Keysym::ISO_Next_Group,
    Keysym::ISO_Prev_Group,
    Keysym::ISO_First_Group,
    Keysym::ISO_Last_Group,
    Keysym::ISO_Next_Group_Lock,
    Keysym::ISO_Prev_Group_Lock,
    Keysym::ISO_First_Group_Lock,
    Keysym::ISO_Last_Group_Lock,
    Keysym::ISO_Group_Lock,
];

/// The most modifier keys held before the key of a chord that locks a group. XKB's group options
/// hold one, such as Alt before Shift's key in `grp:alt_shift_toggle`; two leaves room.
const MAX_SWITCH_MODIFIERS: usize = 2;

/// A keyboard layout as XKB's rules name it: what an agent's seat's keymap is compiled from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyboardLayout {
    /// One layout or several, joined by commas: `fr`, `us,ru`.
    pub layout: String,
    /// Their variants, one for each layout, joined by commas; empty for their default.
    pub variant: String,
    /// XKB's options, joined by commas: `grp:alt_shift_toggle`.
    pub options: String,
}

impl KeyboardLayout {
    /// The layout every agent's seat starts with:
    /// [`AGENT_KEYBOARD_LAYOUT`](crate::AGENT_KEYBOARD_LAYOUT), no variant, no option.
    pub fn agents_default() -> KeyboardLayout {
        KeyboardLayout {
            layout: crate::AGENT_KEYBOARD_LAYOUT.to_owned(),
            variant: String::new(),
            options: String::new(),
        }
    }

    /// What an agent's seat's keyboard is made with, for Smithay to compile.
    pub(crate) fn xkb_config(&self) -> XkbConfig<'_> {
        XkbConfig {
            rules: RULES,
            model: MODEL,
            layout: &self.layout,
            variant: &self.variant,
            options: Some(self.options.clone()),
        }
    }

    /// Refuses a layout no keymap can be compiled from: none named, an empty name between two
    /// commas, which XKB would fill with its own default, a field past [`MAX_LAYOUT_NAME`], or a
    /// character XKB's names never hold. That last check also keeps a NUL from xkbcommon's
    /// bindings, which panic on one, and a path from its file lookup.
    fn check_names(&self) -> Result<(), Error> {
        if self.layout.is_empty() {
            return Err(Error::Refused("name a layout, such as \"fr\"".to_owned()));
        }
        if self.layout.split(',').any(str::is_empty) {
            return Err(Error::Refused(format!(
                "{:?} names an empty layout between its commas",
                self.layout
            )));
        }
        let fields = [
            ("layout", &self.layout, false),
            ("variant", &self.variant, false),
            ("options", &self.options, true),
        ];
        for (field, value, with_colon) in fields {
            if value.len() > MAX_LAYOUT_NAME {
                return Err(Error::Refused(format!(
                    "the {field} holds {MAX_LAYOUT_NAME} bytes at most"
                )));
            }
            let allowed = |c: char| {
                c.is_ascii_alphanumeric()
                    || matches!(c, '_' | '-' | ',')
                    || (with_colon && c == ':')
            };
            if let Some(wrong) = value.chars().find(|&c| !allowed(c)) {
                let colon = if with_colon { ", :" } else { "" };
                return Err(Error::Refused(format!(
                    "the {field} holds {wrong:?}: XKB's names hold letters, digits, _, -{colon} \
                     and commas between names"
                )));
            }
        }
        Ok(())
    }
}

/// A keyboard's state as `wl_keyboard.modifiers` tells it to clients, which read keys with it:
/// the modifiers depressed, latched and locked, and the group in effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct KeyboardState {
    pub depressed: u32,
    pub latched: u32,
    pub locked: u32,
    pub group: u32,
}

impl KeyboardState {
    /// `group`, with no modifier.
    fn plain(group: u32) -> KeyboardState {
        KeyboardState {
            depressed: 0,
            latched: 0,
            locked: 0,
            group,
        }
    }

    fn is_plain(&self) -> bool {
        self.depressed == 0 && self.latched == 0 && self.locked == 0
    }

    fn of(state: &xkb::State) -> KeyboardState {
        KeyboardState {
            depressed: state.serialize_mods(xkb::STATE_MODS_DEPRESSED),
            latched: state.serialize_mods(xkb::STATE_MODS_LATCHED),
            locked: state.serialize_mods(xkb::STATE_MODS_LOCKED),
            group: state.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE),
        }
    }
}

/// Keys pressed in this order and released in the reverse order: the keys held first, then the
/// key that types.
pub(crate) type Chord = Vec<Keycode>;

/// Which keysym each key of one keymap types, from one state: the chord of the fewest keys for
/// each.
type Typed = HashMap<Keysym, Chord>;

/// An agent's seat's keymap: its layout's names, the keymap compiled from them, and how its keys
/// type and switch groups, read once when it is set.
pub(crate) struct SeatKeymap {
    layout: KeyboardLayout,
    keymap: xkb::Keymap,
    /// Its keys, by [`Tier`], then by keycode.
    keys: Vec<Keycode>,
    /// For each group, its keys that pick a level while held, one for each set of modifiers they
    /// depress: Shift, AltGr, level 5's.
    level_keys: Vec<Vec<Keycode>>,
    /// For each group, what its keys type with no modifier depressed, latched or locked.
    typed: Vec<Typed>,
    /// For each group, the key that types each keysym alone, with no modifier: what a tap of
    /// that keysym presses first.
    alone: Vec<HashMap<Keysym, Keycode>>,
    /// For each group, the chords that lock another group, and that group.
    switches: Vec<Vec<(u32, Chord)>>,
}

impl SeatKeymap {
    /// Compiles `layout` and reads its keys. Refused, as [`KeyboardLayout::check_names`] says, for
    /// more than four layouts, an option XKB's rules do not know ([`unknown_option`]), and a
    /// layout or variant XKB fails to compile.
    pub(crate) fn compile(layout: KeyboardLayout) -> Result<SeatKeymap, Error> {
        layout.check_names()?;
        if layout.layout.split(',').count() > MAX_GROUPS as usize {
            return Err(Error::Refused(format!(
                "a keymap holds {MAX_GROUPS} layouts at most"
            )));
        }
        let context = xkb::Context::new(xkb::CONTEXT_NO_ENVIRONMENT_NAMES);
        if let Some(unknown) = unknown_option(&context, &layout.options) {
            return Err(Error::Refused(format!(
                "XKB's {RULES} rules know no option {unknown:?}"
            )));
        }
        let keymap = xkb::Keymap::new_from_names(
            &context,
            RULES,
            MODEL,
            &layout.layout,
            &layout.variant,
            Some(layout.options.clone()),
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .ok_or_else(|| {
            Error::Refused(format!(
                "XKB has no layout {:?} with the variant {:?} and the options {:?}",
                layout.layout, layout.variant, layout.options
            ))
        })?;
        let keys = keys_by_tier(&keymap);
        let mut level_keys = Vec::new();
        let mut typed = Vec::new();
        let mut alone = Vec::new();
        let mut switches = Vec::new();
        for group in 0..keymap.num_layouts() {
            let modifiers = modifier_keys(&keymap, &keys, group);
            let levels = level_keys_among(&modifiers);
            let plain = KeyboardState::plain(group);
            typed.push(typed_from(&keymap, &keys, &levels, plain, &HashSet::new()));
            alone.push(alone_in(&keymap, &keys, group));
            switches.push(switches_of(&keymap, &keys, &modifiers, group));
            level_keys.push(levels);
        }
        Ok(SeatKeymap {
            layout,
            keymap,
            keys,
            level_keys,
            typed,
            alone,
            switches,
        })
    }

    pub(crate) fn layout(&self) -> &KeyboardLayout {
        &self.layout
    }

    /// The keymap's text, as clients are sent it: what Smithay's keyboard compiles its own from
    /// on a change, so that the keymap Yuna types through and the one its clients read are one.
    pub(crate) fn text(&self) -> String {
        self.keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1)
    }

    /// The key that `keysym` names, for a key pressed alone: in `group`, then in each other
    /// group, a key that types it alone if one does, in any tier, so that a tap of `parenleft`
    /// presses the keypad's `(` rather than 9 without its Shift; else the key of the chord that
    /// types it with the fewest keys held, whose other keys the tap names itself: `shift+a`.
    pub(crate) fn key_named(&self, keysym: Keysym, group: u32) -> Option<Keycode> {
        let others = (0..self.typed.len()).filter(|&other| other != group as usize);
        std::iter::once(group as usize)
            .chain(others)
            .find_map(|group| {
                let alone = self.alone.get(group)?.get(&keysym).copied();
                let chord = || self.typed.get(group)?.get(&keysym)?.last().copied();
                alone.or_else(chord)
            })
    }

    /// Whether `key` types `keysym` at some level of some group: a key held that a release
    /// names.
    pub(crate) fn carries(&self, key: Keycode, keysym: Keysym) -> bool {
        (0..self.keymap.num_layouts_for_key(key)).any(|group| {
            (0..self.keymap.num_levels_for_key(key, group)).any(|level| {
                self.keymap
                    .key_get_syms_by_level(key, group, level)
                    .contains(&keysym)
            })
        })
    }

    /// The chords that lock `group` from `from`, checked by running them: none when the
    /// keymap's switches do not lead there from that state.
    pub(crate) fn chords_to_group(&self, from: KeyboardState, group: u32) -> Option<Vec<Chord>> {
        if from.group == group {
            return Some(Vec::new());
        }
        let chords = self.path(from.group, group)?;
        (run(&self.keymap, from, &chords).1.group == group).then_some(chords)
    }

    /// The state a keyboard of this keymap is left in when it is made, or when Smithay switches
    /// to it, with `held` pressed: a fresh state, every key held pressed again.
    pub(crate) fn state_with(&self, held: &HashSet<Keycode>) -> KeyboardState {
        let mut state = xkb::State::new(&self.keymap);
        for &key in held {
            state.update_key(key, xkb::KeyDirection::Down);
        }
        KeyboardState::of(&state)
    }

    /// The shortest chain of chords that locks group `to` from group `from`, through the
    /// keymap's switches.
    fn path(&self, from: u32, to: u32) -> Option<Vec<Chord>> {
        let mut paths: HashMap<u32, Vec<Chord>> = HashMap::from([(from, Vec::new())]);
        let mut next = VecDeque::from([from]);
        while let Some(group) = next.pop_front() {
            if group == to {
                return paths.remove(&to);
            }
            let here = paths.get(&group).cloned().unwrap_or_default();
            for (reached, chord) in self.switches.get(group as usize).into_iter().flatten() {
                if !paths.contains_key(reached) {
                    let mut longer = here.clone();
                    longer.push(chord.clone());
                    paths.insert(*reached, longer);
                    next.push_back(*reached);
                }
            }
        }
        None
    }
}

/// Plans the keys that type a text, character after character, from a keyboard's state, as that
/// state changes with each chord: what `type` presses.
pub(crate) struct KeyPlanner<'a> {
    keymap: &'a SeatKeymap,
    /// The keys held before typing starts, which a chord never takes as a key held.
    held: &'a HashSet<Keycode>,
    start: KeyboardState,
    state: KeyboardState,
    /// What the keys type from each state met that is not plain, such as one with Caps Lock
    /// locked, or Shift held before typing started.
    tables: HashMap<KeyboardState, Typed>,
}

impl<'a> KeyPlanner<'a> {
    pub(crate) fn new(
        keymap: &'a SeatKeymap,
        start: KeyboardState,
        held: &'a HashSet<Keycode>,
    ) -> KeyPlanner<'a> {
        KeyPlanner {
            keymap,
            held,
            start,
            state: start,
            tables: HashMap::new(),
        }
    }

    /// The chords that type `keysym` next: in the group in effect if it has it, else after the
    /// chords that lock the nearest group that has it. None when no key of the keymap types it
    /// from the state typing reached, and nothing is planned for it.
    pub(crate) fn chords_for(&mut self, keysym: Keysym) -> Option<Vec<Chord>> {
        if let Some(chords) = self.after_path(Vec::new(), keysym) {
            return Some(chords);
        }
        let keymap = self.keymap;
        let mut paths: Vec<Vec<Chord>> = (0..keymap.keymap.num_layouts())
            .filter(|&group| group != self.state.group)
            .filter(|&group| keymap.typed[group as usize].contains_key(&keysym))
            .filter_map(|group| keymap.path(self.state.group, group))
            .collect();
        paths.sort_by_key(Vec::len);
        paths
            .into_iter()
            .find_map(|path| self.after_path(path, keysym))
    }

    /// The chords that type `keysym` once `path`'s have locked another group, `path`'s first.
    /// They are run again from where typing is, as the client will read them, and kept, the
    /// state moving on, only if the last one types `keysym`.
    fn after_path(&mut self, path: Vec<Chord>, keysym: Keysym) -> Option<Vec<Chord>> {
        let keymap = &self.keymap.keymap;
        let at_group = if path.is_empty() {
            self.state
        } else {
            run(keymap, self.state, &path).1
        };
        let chord = self.chord_from(at_group, keysym)?;
        let mut chords = path;
        chords.push(chord);
        let (typed, after) = run(keymap, self.state, &chords);
        if typed.last() != Some(&keysym) {
            return None;
        }
        self.state = after;
        Some(chords)
    }

    /// The chords that lock the group typing started in again, if a character moved it; none
    /// when the keymap's switches do not lead back from where typing ended, which `type` then
    /// logs.
    pub(crate) fn finish(self) -> Option<Vec<Chord>> {
        self.keymap.chords_to_group(self.state, self.start.group)
    }

    /// The group typing started in.
    pub(crate) fn start_group(&self) -> u32 {
        self.start.group
    }

    /// The chord that types `keysym` from `state`, from the table read when the keymap was set
    /// for a plain state, or from one read now for another. Only with keys X11 carries, so that
    /// the character reaches an X11 window too.
    fn chord_from(&mut self, state: KeyboardState, keysym: Keysym) -> Option<Chord> {
        let keymap = self.keymap;
        let level_keys = keymap.level_keys.get(state.group as usize)?;
        let chord = if state.is_plain() {
            keymap.typed.get(state.group as usize)?.get(&keysym)
        } else {
            let held = self.held;
            self.tables
                .entry(state)
                .or_insert_with(|| {
                    typed_from(&keymap.keymap, &keymap.keys, level_keys, state, held)
                })
                .get(&keysym)
        }?;
        chord
            .iter()
            .all(|&key| reaches_x11(key))
            .then(|| chord.clone())
    }
}

/// A state of `keymap` set to `from`, as a client sets its own from `wl_keyboard.modifiers`.
fn state_at(keymap: &xkb::Keymap, from: KeyboardState) -> xkb::State {
    let mut state = xkb::State::new(keymap);
    state.update_mask(from.depressed, from.latched, from.locked, 0, 0, from.group);
    state
}

/// Runs `chords` from `from` through xkbcommon's state, as Smithay's keyboard runs them: the
/// keysym each chord's last key types, read as its client reads it at the press, and the state
/// they leave.
fn run(
    keymap: &xkb::Keymap,
    from: KeyboardState,
    chords: &[Chord],
) -> (Vec<Keysym>, KeyboardState) {
    let mut state = state_at(keymap, from);
    let mut typed = Vec::with_capacity(chords.len());
    for chord in chords {
        let Some((&key, held)) = chord.split_last() else {
            continue;
        };
        for &holding in held {
            state.update_key(holding, xkb::KeyDirection::Down);
        }
        typed.push(state.key_get_one_sym(key));
        state.update_key(key, xkb::KeyDirection::Down);
        state.update_key(key, xkb::KeyDirection::Up);
        for &holding in held.iter().rev() {
            state.update_key(holding, xkb::KeyDirection::Up);
        }
    }
    (typed, KeyboardState::of(&state))
}

/// The last key of a PC keyboard, Linux's `KEY_COMPOSE` (127) as XKB numbers it: evdev's keys
/// from `KEY_ESC` to it are a 105-key keyboard's and the Japanese and Korean keyboards' extra
/// keys, and one of XKB's fake keys, `<LVL3>`.
const LAST_PC_KEY: u32 = 135;

/// XKB's fake keys, which evdev's keycodes give the modifiers that no key of a keyboard carries
/// (`keycodes/evdev`, "Fake keycodes for virtual keys"): `<LVL3>` is 92, the others above 200.
const FAKE_KEYS: [&str; 6] = ["LVL3", "LVL5", "ALT", "META", "SUPR", "HYPR"];

/// Which keys a chord takes first, in this order: a PC keyboard's, then the others X11 carries,
/// such as the keypad's parentheses, `<I187>` and `<I188>`, and XKB's fake keys, then the rest.
/// An X11 keycode is a byte, and Xwayland drops a key past 255, such as evdev's `<I443>`, which
/// types `EuroSign`, or `<I592>`, Linux's `KEY_KBD_LAYOUT_NEXT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Tier {
    Pc,
    X11,
    Beyond,
}

fn tier(keymap: &xkb::Keymap, key: Keycode) -> Tier {
    let fake = || {
        keymap
            .key_get_name(key)
            .is_some_and(|name| FAKE_KEYS.contains(&name))
    };
    match key.raw() {
        raw if raw <= LAST_PC_KEY && !fake() => Tier::Pc,
        raw if xkb::keycode_is_legal_x11(raw) => Tier::X11,
        _ => Tier::Beyond,
    }
}

/// Every key of `keymap` that types something in a group, by tier, then by keycode: an evdev
/// keymap names about 700 keycodes, and leaves two thirds of them empty.
fn keys_by_tier(keymap: &xkb::Keymap) -> Vec<Keycode> {
    let mut keys: Vec<Keycode> = (keymap.min_keycode().raw()..=keymap.max_keycode().raw())
        .map(Keycode::new)
        .filter(|&key| keymap.num_layouts_for_key(key) > 0)
        .collect();
    keys.sort_by_key(|&key| (tier(keymap, key), key.raw()));
    keys
}

/// `keys`, ordered by tier, cut into one slice for each tier.
fn tiers_of<'a>(keymap: &xkb::Keymap, keys: &'a [Keycode]) -> impl Iterator<Item = &'a [Keycode]> {
    keys.chunk_by(|&a, &b| tier(keymap, a) == tier(keymap, b))
}

/// Whether X11 carries `key`, so that a character typed with it reaches an X11 window too.
fn reaches_x11(key: Keycode) -> bool {
    xkb::keycode_is_legal_x11(key.raw())
}

/// Which key types each keysym alone in `group`, with no modifier, by tier, then by keycode.
fn alone_in(keymap: &xkb::Keymap, keys: &[Keycode], group: u32) -> HashMap<Keysym, Keycode> {
    let state = state_at(keymap, KeyboardState::plain(group));
    let mut alone = HashMap::new();
    for &key in keys {
        let keysym = state.key_get_one_sym(key);
        if keysym != Keysym::NoSymbol {
            alone.entry(keysym).or_insert(key);
        }
    }
    alone
}

/// The first of `options`, joined by commas, that XKB's rules file does not name: XKB would
/// ignore it without a word. The rules file is the first `rules/evdev` among the context's
/// include paths, where XKB reads it; its option sections name each option in the column their
/// header calls `option`. None when every option is named, and when no rules file is read, or
/// none of its sections names an option: XKB then has the last word.
fn unknown_option(context: &xkb::Context, options: &str) -> Option<String> {
    let given: Vec<&str> = options
        .split(',')
        .filter(|option| !option.is_empty())
        .collect();
    if given.is_empty() {
        return None;
    }
    let rules = context
        .include_paths()
        .find_map(|path| std::fs::read_to_string(path.join("rules").join(RULES)).ok())?;
    let known = options_of_rules(&rules);
    if known.is_empty() {
        return None;
    }
    given
        .into_iter()
        .find(|option| !known.contains(option))
        .map(str::to_owned)
}

/// The options a rules file names. A section starts with a header, `! model layout option =
/// symbols`; each line under it names one value for each of the header's columns before `=`.
fn options_of_rules(rules: &str) -> HashSet<&str> {
    let mut known = HashSet::new();
    let mut column = None;
    for line in rules.lines().map(str::trim) {
        if let Some(header) = line.strip_prefix('!') {
            let names = header.split('=').next().unwrap_or_default();
            column = names.split_whitespace().position(|name| name == "option");
        } else if let Some(column) = column
            && !line.is_empty()
            && !line.starts_with("//")
            && let Some(option) = line.split_whitespace().nth(column)
        {
            known.insert(option);
        }
    }
    known
}

/// The modifier keys of `group`: those X11 carries, whose first level is one of
/// [`MODIFIER_KEYSYMS`], and that depress modifiers while held, each with its keysym and what it
/// depresses, found by pressing it.
fn modifier_keys(
    keymap: &xkb::Keymap,
    keys: &[Keycode],
    group: u32,
) -> Vec<(Keycode, Keysym, u32)> {
    let plain = KeyboardState::plain(group);
    let state = state_at(keymap, plain);
    keys.iter()
        .copied()
        .filter(|&key| reaches_x11(key))
        .filter_map(|key| {
            let keysym = state.key_get_one_sym(key);
            if !MODIFIER_KEYSYMS.contains(&keysym) {
                return None;
            }
            let mut pressing = state_at(keymap, plain);
            pressing.update_key(key, xkb::KeyDirection::Down);
            let pressed = KeyboardState::of(&pressing);
            let only_depressed = pressed.depressed != 0
                && pressed.latched == 0
                && pressed.locked == 0
                && pressed.group == group;
            only_depressed.then_some((key, keysym, pressed.depressed))
        })
        .collect()
}

/// The keys among `modifiers` that pick a level while held, the first key for each set of
/// modifiers they depress: one Shift, one AltGr, one level 5's key.
fn level_keys_among(modifiers: &[(Keycode, Keysym, u32)]) -> Vec<Keycode> {
    let mut seen = Vec::new();
    modifiers
        .iter()
        .filter(|(_, keysym, depressed)| {
            let new = LEVEL_KEYSYMS.contains(keysym) && !seen.contains(depressed);
            if new {
                seen.push(*depressed);
            }
            new
        })
        .map(|&(key, _, _)| key)
        .collect()
}

/// Which keysym each key of `keymap` types from `from`, alone, then with one of `level_keys` held,
/// then two, and so on: the chord of the fewest keys for each, by the keys' [`Tier`], then the
/// lowest keycodes first. A level key that `held` holds already, or whose modifiers
/// `from` depresses, is left out: it adds nothing.
fn typed_from(
    keymap: &xkb::Keymap,
    keys: &[Keycode],
    level_keys: &[Keycode],
    from: KeyboardState,
    held: &HashSet<Keycode>,
) -> Typed {
    let usable: Vec<Keycode> = level_keys
        .iter()
        .copied()
        .filter(|key| !held.contains(key))
        .filter(|&key| {
            let mut state = state_at(keymap, KeyboardState::plain(from.group));
            state.update_key(key, xkb::KeyDirection::Down);
            KeyboardState::of(&state).depressed & from.depressed == 0
        })
        .collect();
    let holdings: Vec<(Vec<Keycode>, xkb::State)> = choices(&usable, usable.len())
        .into_iter()
        .map(|holding| {
            let mut state = state_at(keymap, from);
            for &key in &holding {
                state.update_key(key, xkb::KeyDirection::Down);
            }
            (holding, state)
        })
        .collect();
    let mut typed = Typed::new();
    for tier_keys in tiers_of(keymap, keys) {
        for (holding, state) in &holdings {
            for &key in tier_keys.iter().filter(|key| !holding.contains(key)) {
                let keysym = state.key_get_one_sym(key);
                if keysym != Keysym::NoSymbol {
                    typed.entry(keysym).or_insert_with(|| {
                        let mut chord = holding.clone();
                        chord.push(key);
                        chord
                    });
                }
            }
        }
    }
    typed
}

/// The chords of `group` that lock another group, and that group, the first found for each: a key
/// that carries a keysym of [`GROUP_KEYSYMS`], alone, or with one or two modifier keys held
/// before it, each tried from the group with no modifier, by the keys' [`Tier`]: a PC keyboard's
/// first, those of the group options such as `grp:alt_shift_toggle`; evdev's layout key,
/// `<I592>`, last, which X11 never gets: its clients then read the group from the modifiers alone.
fn switches_of(
    keymap: &xkb::Keymap,
    keys: &[Keycode],
    modifiers: &[(Keycode, Keysym, u32)],
    group: u32,
) -> Vec<(u32, Chord)> {
    let triggers: Vec<Keycode> = keys
        .iter()
        .copied()
        .filter(|&key| {
            (0..keymap.num_levels_for_key(key, group)).any(|level| {
                keymap
                    .key_get_syms_by_level(key, group, level)
                    .iter()
                    .any(|keysym| GROUP_KEYSYMS.contains(keysym))
            })
        })
        .collect();
    if triggers.is_empty() {
        return Vec::new();
    }
    let modifiers: Vec<Keycode> = modifiers.iter().map(|&(key, _, _)| key).collect();
    let plain = KeyboardState::plain(group);
    // Each choice with its keys held, so that a trigger is run whole only where they reach the
    // level that carries its group keysym.
    let holdings: Vec<(Vec<Keycode>, xkb::State)> = choices(&modifiers, MAX_SWITCH_MODIFIERS)
        .into_iter()
        .map(|holding| {
            let mut state = state_at(keymap, plain);
            for &key in &holding {
                state.update_key(key, xkb::KeyDirection::Down);
            }
            (holding, state)
        })
        .collect();
    let mut found: Vec<(u32, Chord)> = Vec::new();
    for tier_triggers in tiers_of(keymap, &triggers) {
        for (holding, state) in &holdings {
            let reached = tier_triggers
                .iter()
                .filter(|key| !holding.contains(key))
                .filter(|&&key| GROUP_KEYSYMS.contains(&state.key_get_one_sym(key)));
            for &trigger in reached {
                let mut chord = holding.clone();
                chord.push(trigger);
                let after = run(keymap, plain, std::slice::from_ref(&chord)).1;
                let locks_another = after.group != group && after.is_plain();
                if locks_another && !found.iter().any(|(reached, _)| *reached == after.group) {
                    found.push((after.group, chord));
                }
            }
        }
    }
    found
}

/// Every choice of at most `most` keys among `keys`, the fewest first, each in `keys`' order.
fn choices(keys: &[Keycode], most: usize) -> Vec<Vec<Keycode>> {
    let mut all = vec![Vec::new()];
    // Each choice of the last size, with the index of the first key that may follow it.
    let mut last: Vec<(usize, Vec<Keycode>)> = vec![(0, Vec::new())];
    for _ in 0..most {
        let mut longer = Vec::new();
        for (from, choice) in &last {
            for (index, &key) in keys.iter().enumerate().skip(*from) {
                let mut next = choice.clone();
                next.push(key);
                longer.push((index + 1, next));
            }
        }
        all.extend(longer.iter().map(|(_, choice)| choice.clone()));
        last = longer;
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(layout: &str, variant: &str, options: &str) -> KeyboardLayout {
        KeyboardLayout {
            layout: layout.to_owned(),
            variant: variant.to_owned(),
            options: options.to_owned(),
        }
    }

    fn compiled(name: &str, options: &str) -> SeatKeymap {
        SeatKeymap::compile(layout(name, "", options)).expect("the layout compiles")
    }

    /// The keysym `chord` types, in `group` with no modifier.
    fn keysym_of(keymap: &SeatKeymap, group: u32, chord: &[Keycode]) -> Keysym {
        run(
            &keymap.keymap,
            KeyboardState::plain(group),
            &[chord.to_vec()],
        )
        .0[0]
    }

    fn names(chord: &[Keycode], keymap: &SeatKeymap) -> Vec<String> {
        chord
            .iter()
            .map(|&key| keymap.keymap.key_get_name(key).unwrap_or("?").to_owned())
            .collect()
    }

    fn plan(keymap: &SeatKeymap, text: &str) -> (Vec<Vec<String>>, KeyboardState) {
        let held = HashSet::new();
        let mut planner = KeyPlanner::new(keymap, KeyboardState::plain(0), &held);
        let mut chords = Vec::new();
        for character in text.chars() {
            let keysym = xkb::utf32_to_keysym(u32::from(character));
            chords.extend(planner.chords_for(keysym).expect("typed with keys"));
        }
        let state = planner.state;
        chords.extend(planner.finish().expect("a way back"));
        let chords = chords.iter().map(|chord| names(chord, keymap)).collect();
        (chords, state)
    }

    #[test]
    fn the_default_is_us_with_shift_alone() {
        let us = SeatKeymap::compile(KeyboardLayout::agents_default()).expect("us compiles");
        let (chords, _) = plan(&us, "aA@(");
        assert_eq!(
            chords,
            [
                vec!["AC01"],
                vec!["LFSH", "AC01"],
                vec!["LFSH", "AE02"],
                vec!["LFSH", "AE09"]
            ],
            "a PC keyboard's keys before the keypad's parenthesis"
        );
    }

    /// French: a and q trade places with us's, and @ is AltGr's level of 0's key.
    #[test]
    fn french_types_at_through_altgr() {
        let fr = compiled("fr", "");
        let (chords, _) = plan(&fr, "aq@");
        assert_eq!(chords[0], ["AD01"]);
        assert_eq!(chords[1], ["AC01"]);
        assert_eq!(chords[2], ["RALT", "AE10"], "Right Alt, French's AltGr");
        let ralt = fr.keymap.key_by_name("RALT").expect("Right Alt");
        assert_eq!(keysym_of(&fr, 0, &[ralt]), Keysym::ISO_Level3_Shift);
    }

    /// AltGr is Right Alt where the layout puts level 3 on it; `us`, which has none there, keeps
    /// XKB's fake key, typing nothing at level 3 anyway.
    #[test]
    fn altgr_is_right_alt_where_the_layout_has_it() {
        let name = |keymap: &SeatKeymap| {
            let altgr = keymap.level_keys[0]
                .iter()
                .find(|&&key| keysym_of(keymap, 0, &[key]) == Keysym::ISO_Level3_Shift)
                .and_then(|&key| keymap.keymap.key_get_name(key));
            altgr.map(str::to_owned)
        };
        assert_eq!(name(&compiled("de", "")).as_deref(), Some("RALT"));
        assert_eq!(name(&compiled("fr", "")).as_deref(), Some("RALT"));
        let us = SeatKeymap::compile(KeyboardLayout::agents_default()).expect("us compiles");
        assert_eq!(name(&us).as_deref(), Some("LVL3"));
        assert_eq!(
            us.key_named(Keysym::ISO_Level3_Shift, 0)
                .and_then(|key| us.keymap.key_get_name(key)),
            Some("LVL3")
        );
    }

    #[test]
    fn german_types_at_and_the_euro_through_altgr() {
        let de = compiled("de", "");
        let (chords, _) = plan(&de, "@€z");
        assert_eq!(chords[0].last().map(String::as_str), Some("AD01"));
        assert_eq!(chords[1].last().map(String::as_str), Some("AD03"));
        assert_eq!(chords[0].len(), 2);
        assert_eq!(chords[2], ["AD06"]);
    }

    /// A Cyrillic letter is typed in the second group, which Alt and Shift lock; typing locks the
    /// first group again at its end.
    #[test]
    fn a_second_group_is_locked_through_the_keymaps_own_switch() {
        let both = compiled("us,ru", "grp:alt_shift_toggle");
        assert_eq!(both.switches[0].len(), 1, "{:?}", both.switches);
        let (chords, state) = plan(&both, "aф");
        assert_eq!(state.group, 1);
        assert_eq!(chords.len(), 4, "{chords:?}");
        assert_eq!(chords[0], ["AC01"]);
        assert_eq!(chords[2], ["AC01"], "ф is on a's key");
        assert_eq!(chords[1], chords[3], "the same toggle back");
        let mut toggle = chords[1].clone();
        toggle.sort();
        assert_eq!(toggle, ["LALT", "LFSH"]);
    }

    /// Without a group option, us,ru locks its second group through evdev's layout key alone,
    /// which X11 never gets.
    #[test]
    fn without_an_option_the_layout_key_locks_the_next_group() {
        let both = compiled("us,ru", "");
        let switches: Vec<Vec<String>> = both.switches[0]
            .iter()
            .map(|(_, chord)| names(chord, &both))
            .collect();
        assert_eq!(switches, [["I592"]]);
        let (chords, _) = plan(&both, "фa");
        assert_eq!(
            chords,
            [vec!["I592"], vec!["AC01"], vec!["I592"], vec!["AC01"]]
        );
    }

    /// A character is never typed with a key X11 drops: us's euro, on evdev's `<I443>` alone, is
    /// not typed with keys; it names that key for a tap all the same.
    #[test]
    fn a_character_is_typed_with_keys_x11_carries_alone() {
        let us = SeatKeymap::compile(KeyboardLayout::agents_default()).expect("us compiles");
        let held = HashSet::new();
        let mut planner = KeyPlanner::new(&us, KeyboardState::plain(0), &held);
        assert!(planner.chords_for(Keysym::EuroSign).is_none());
        let euro = us.key_named(Keysym::EuroSign, 0).expect("a key");
        assert!(!reaches_x11(euro));
    }

    /// With Caps Lock locked, a small letter takes Shift and a capital none, as a client reads
    /// them.
    #[test]
    fn caps_lock_locked_is_typed_through() {
        let us = SeatKeymap::compile(KeyboardLayout::agents_default()).expect("us compiles");
        let caps = 1 << us.keymap.mod_get_index(xkb::MOD_NAME_CAPS);
        let locked = KeyboardState {
            locked: caps,
            ..KeyboardState::plain(0)
        };
        let held = HashSet::new();
        let mut planner = KeyPlanner::new(&us, locked, &held);
        let small = planner.chords_for(Keysym::a).expect("a");
        let capital = planner.chords_for(Keysym::A).expect("A");
        assert_eq!(names(&small[0], &us), ["LFSH", "AC01"]);
        assert_eq!(names(&capital[0], &us), ["AC01"]);
    }

    /// A key held before typing is never held again by a chord: with Shift held, a capital is its
    /// key alone, and a small letter has no key.
    #[test]
    fn a_held_shift_is_not_pressed_again() {
        let us = SeatKeymap::compile(KeyboardLayout::agents_default()).expect("us compiles");
        let shift = us.level_keys[0][0];
        let held = HashSet::from([shift]);
        let start = us.state_with(&held);
        assert_ne!(start.depressed, 0);
        let mut planner = KeyPlanner::new(&us, start, &held);
        assert_eq!(
            names(&planner.chords_for(Keysym::A).expect("A")[0], &us),
            ["AC01"]
        );
        assert!(planner.chords_for(Keysym::a).is_none());
    }

    #[test]
    fn names_xkb_does_not_know_are_refused() {
        for (name, variant) in [("nosuchlayout", ""), ("fr", "nosuchvariant"), ("us,zz", "")] {
            let refused = SeatKeymap::compile(layout(name, variant, ""));
            let why = match &refused {
                Err(Error::Refused(why)) => why.as_str(),
                _ => "",
            };
            assert!(
                why.starts_with("XKB has no layout"),
                "{name}({variant}): {:?}",
                refused.err()
            );
        }
    }

    #[test]
    fn names_xkb_never_holds_are_refused_before_xkb_reads_them() {
        for (name, variant, options) in [
            ("", "", ""),
            ("fr\0", "", ""),
            ("../fr", "", ""),
            ("fr", "", "grp alt"),
            ("fr", "a:b", ""),
        ] {
            assert!(
                matches!(
                    SeatKeymap::compile(layout(name, variant, options)),
                    Err(Error::Refused(_))
                ),
                "{name:?} {variant:?} {options:?}"
            );
        }
        let long = "a".repeat(MAX_LAYOUT_NAME + 1);
        assert!(SeatKeymap::compile(layout(&long, "", "")).is_err());
    }

    #[test]
    fn more_than_four_layouts_are_refused() {
        let refused = SeatKeymap::compile(layout("us,fr,de,ru,es", "", ""));
        assert!(refused.is_err());
        assert!(SeatKeymap::compile(layout("us,fr,de,ru", "", "")).is_ok());
    }

    /// A key is named by any keysym it types, in the group in effect first.
    #[test]
    fn a_key_is_named_by_its_keysym_in_the_group_in_effect_first() {
        let both = compiled("us,fr", "");
        let a_in_us = both.key_named(Keysym::a, 0).expect("a");
        let a_in_fr = both.key_named(Keysym::a, 1).expect("a");
        assert_eq!(both.keymap.key_get_name(a_in_us), Some("AC01"));
        assert_eq!(both.keymap.key_get_name(a_in_fr), Some("AD01"));
        // fr types the euro with AltGr and E; a tap of it names us's `<I443>`, which types it
        // alone in either group.
        let euro_in_fr = both.typed[1].get(&Keysym::EuroSign).expect("fr's euro");
        assert_eq!(names(euro_in_fr, &both), ["RALT", "AD03"]);
        let euro = both.key_named(Keysym::EuroSign, 1).expect("a key");
        assert_eq!(both.keymap.key_get_name(euro), Some("I443"));
        assert!(both.key_named(Keysym::Cyrillic_ef, 0).is_none());
    }

    /// A tap of a keysym presses a key that types it alone, in any tier, before the key of a
    /// chord that needs Shift: `parenleft` is the keypad's `(`, not 9.
    #[test]
    fn a_key_named_types_its_keysym_alone_first() {
        let us = SeatKeymap::compile(KeyboardLayout::agents_default()).expect("us compiles");
        let name = |keysym| {
            us.key_named(keysym, 0)
                .and_then(|key| us.keymap.key_get_name(key))
                .map(str::to_owned)
        };
        assert_eq!(name(Keysym::parenleft).as_deref(), Some("I187"));
        assert_eq!(name(Keysym::parenright).as_deref(), Some("I188"));
        assert_eq!(
            name(Keysym::A).as_deref(),
            Some("AC01"),
            "a's key, Shift named apart"
        );
        assert_eq!(name(Keysym::a).as_deref(), Some("AC01"));
        let held = HashSet::new();
        let mut planner = KeyPlanner::new(&us, KeyboardState::plain(0), &held);
        let typed = planner.chords_for(Keysym::parenleft).expect("(");
        assert_eq!(
            names(&typed[0], &us),
            ["LFSH", "AE09"],
            "type takes a PC key"
        );
    }

    #[test]
    fn options_the_rules_do_not_name_are_refused() {
        let misspelt = SeatKeymap::compile(layout("us,ru", "", "grp:alt_shft_toggle"));
        assert!(
            matches!(&misspelt, Err(Error::Refused(why))
                if why == "XKB's evdev rules know no option \"grp:alt_shft_toggle\""),
            "{:?}",
            misspelt.err()
        );
        let several = "grp:alt_shift_toggle,lv3:ralt_switch,compose:rctrl,ctrl:nocaps";
        assert!(SeatKeymap::compile(layout("us,ru", "", several)).is_ok());
        assert!(SeatKeymap::compile(layout("us", "", "ctrl:nocaps,nonsense:x")).is_err());
    }

    #[test]
    fn a_rules_options_are_read_from_their_column() {
        let rules = "! $azerty = be fr\n\
                     ! model = keycodes\n  pc105 = evdev\n\
                     ! layout option = symbols\n  * grp:one = +group(one)\n\
                     // a comment\n\
                     ! option = types\n  ctrl:two = +ctrl(two)\n";
        let known = options_of_rules(rules);
        assert_eq!(known, HashSet::from(["grp:one", "ctrl:two"]));
    }

    #[test]
    fn an_empty_layout_between_commas_is_refused() {
        for name in ["fr,", ",", ",fr", "us,,fr"] {
            assert!(
                SeatKeymap::compile(layout(name, "", "")).is_err(),
                "{name:?}"
            );
        }
        assert!(
            SeatKeymap::compile(layout("us,fr", ",", "")).is_ok(),
            "variants may be empty"
        );
    }

    #[test]
    fn choices_come_fewest_first() {
        let keys = [Keycode::new(1), Keycode::new(2), Keycode::new(3)];
        let raw: Vec<Vec<u32>> = choices(&keys, 2)
            .iter()
            .map(|choice| choice.iter().map(|key| key.raw()).collect())
            .collect();
        assert_eq!(
            raw,
            [
                vec![],
                vec![1],
                vec![2],
                vec![3],
                vec![1, 2],
                vec![1, 3],
                vec![2, 3]
            ]
        );
    }

    #[test]
    fn typing_starts_in_the_group_the_keyboard_is_in() {
        let four = compiled("us,fr,de,ru", "");
        let held = HashSet::new();
        for group in 0..4 {
            let planner = KeyPlanner::new(&four, KeyboardState::plain(group), &held);
            assert_eq!(planner.start_group(), group);
        }
    }

    /// A field of exactly the longest length is accepted; one byte more is refused, in each field.
    #[test]
    fn a_field_may_hold_the_most_bytes_and_no_more() {
        let longest = "a".repeat(MAX_LAYOUT_NAME);
        let too_long = "a".repeat(MAX_LAYOUT_NAME + 1);
        assert!(layout(&longest, "", "").check_names().is_ok());
        assert!(layout("us", &longest, "").check_names().is_ok());
        assert!(layout("us", "", &longest).check_names().is_ok());
        for refused in [
            layout(&too_long, "", ""),
            layout("us", &too_long, ""),
            layout("us", "", &too_long),
        ] {
            assert!(
                matches!(refused.check_names(), Err(Error::Refused(why)) if why.contains("bytes at most")),
                "{refused:?}"
            );
        }
    }

    /// Only the options hold a colon, such as `grp:alt_shift_toggle`; no field holds another
    /// character than letters, digits, `_`, `-` and commas.
    #[test]
    fn a_colon_belongs_to_the_options_alone() {
        assert!(
            layout("us", "", "grp:alt_shift_toggle")
                .check_names()
                .is_ok()
        );
        assert!(layout("us:x", "", "").check_names().is_err());
        assert!(layout("us", "a:b", "").check_names().is_err());
        for wrong in [" ", "/", ".", "\0", "\u{e9}", "="] {
            let options = format!("grp{wrong}x");
            assert!(
                layout("us", "", &options).check_names().is_err(),
                "{options:?}"
            );
        }
    }

    /// The configuration Smithay compiles an agent's keyboard from names the evdev rules, the pc105
    /// model and the layout's own fields.
    #[test]
    fn the_keyboard_is_made_from_the_layouts_own_names() {
        let named = layout("fr,us", "azerty,", "grp:alt_shift_toggle");
        let config = named.xkb_config();
        assert_eq!(config.rules, "evdev");
        assert_eq!(config.model, "pc105");
        assert_eq!(config.layout, "fr,us");
        assert_eq!(config.variant, "azerty,");
        assert_eq!(config.options.as_deref(), Some("grp:alt_shift_toggle"));
    }

    /// The keymap's text is the one clients are sent: it compiles again to the same keys.
    #[test]
    fn the_keymap_text_compiles_again_to_the_same_keymap() {
        let fr = compiled("fr", "");
        let text = fr.text();
        assert!(text.contains("xkb_symbols"), "{text:.80}");
        let context = xkb::Context::new(xkb::CONTEXT_NO_ENVIRONMENT_NAMES);
        let again = xkb::Keymap::new_from_string(
            &context,
            text.clone(),
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .expect("the text is a keymap");
        assert_eq!(again.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1), text);
        let a = fr.keymap.key_by_name("AD01").expect("a key");
        let state = state_at(&again, KeyboardState::plain(0));
        assert_eq!(state.key_get_one_sym(a), Keysym::a);
    }

    /// A key carries a keysym when it types it at any level of any group, and no other.
    #[test]
    fn a_key_carries_every_keysym_it_types_at_some_level_of_some_group() {
        let both = compiled("us,ru", "");
        let a = both.keymap.key_by_name("AC01").expect("a's key");
        for keysym in [
            Keysym::a,
            Keysym::A,
            Keysym::Cyrillic_ef,
            Keysym::Cyrillic_EF,
        ] {
            assert!(both.carries(a, keysym), "{keysym:?}");
        }
        for keysym in [Keysym::b, Keysym::Cyrillic_be, Keysym::Shift_L] {
            assert!(!both.carries(a, keysym), "{keysym:?}");
        }
    }

    /// A tap of a keysym that only another group types takes that group's key.
    #[test]
    fn a_key_is_named_in_another_group_when_the_group_in_effect_lacks_the_keysym() {
        let both = compiled("us,ru", "");
        let key = both.key_named(Keysym::Cyrillic_ef, 0).expect("ru's ef");
        assert_eq!(both.keymap.key_get_name(key), Some("AC01"));
        let key = both.key_named(Keysym::a, 1).expect("us's a");
        assert_eq!(both.keymap.key_get_name(key), Some("AC01"));
    }

    /// The keys that count are those that type something: evdev's keycodes number about 700, and
    /// most are empty.
    #[test]
    fn only_keys_that_type_something_are_kept() {
        let us = compiled("us", "");
        let (min, max) = (us.keymap.min_keycode().raw(), us.keymap.max_keycode().raw());
        let empty =
            (min..=max).filter(|&raw| us.keymap.num_layouts_for_key(Keycode::new(raw)) == 0);
        assert!(
            empty.count() > 0,
            "the keymap has keycodes that type nothing"
        );
        assert!(!us.keys.is_empty());
        for &key in &us.keys {
            assert!(us.keymap.num_layouts_for_key(key) > 0, "{key:?}");
        }
    }

    /// The tiers: a PC keyboard's keys, those X11 carries beyond them, evdev's `<LVL3>` among them,
    /// and the keys past X11's 255.
    #[test]
    fn keys_are_ranked_by_what_reaches_a_pc_keyboard_then_x11_then_nothing() {
        let us = compiled("us", "");
        let tier_of = |name: &str| tier(&us.keymap, us.keymap.key_by_name(name).expect(name));
        assert_eq!(tier_of("AC01"), Tier::Pc);
        assert_eq!(tier_of("LFSH"), Tier::Pc);
        assert_eq!(tier_of("I187"), Tier::X11, "the keypad's parenthesis");
        assert_eq!(tier_of("LVL3"), Tier::X11, "a fake key");
        assert_eq!(tier_of("I443"), Tier::Beyond, "past X11's keycodes");
        assert_eq!(tier_of("I592"), Tier::Beyond);
    }

    /// The keys, in tier order, are cut into one slice for each tier, no tier in two.
    #[test]
    fn the_keys_are_cut_into_one_slice_for_each_tier() {
        let us = compiled("us", "");
        let tiers: Vec<&[Keycode]> = tiers_of(&us.keymap, &us.keys).collect();
        assert_eq!(tiers.len(), 3, "a PC keyboard's, X11's beyond it, the rest");
        let kinds: Vec<Tier> = tiers
            .iter()
            .map(|slice| {
                let first = tier(&us.keymap, slice[0]);
                assert!(slice.iter().all(|&key| tier(&us.keymap, key) == first));
                first
            })
            .collect();
        assert_eq!(kinds, [Tier::Pc, Tier::X11, Tier::Beyond]);
        let counted: usize = tiers.iter().map(|slice| slice.len()).sum();
        assert_eq!(counted, us.keys.len());
    }

    /// A key whose first level is a modifier's keysym but that depresses nothing when pressed is
    /// not a modifier key; a key that locks one when pressed is not a held modifier either.
    #[test]
    fn only_keys_that_depress_a_modifier_while_held_are_modifier_keys() {
        let text = "xkb_keymap {\n\
            xkb_keycodes \"k\" { minimum = 8; maximum = 255; <HELD> = 50; <DEAD> = 51; <LOCK> = 52; };\n\
            xkb_types \"t\" { virtual_modifiers NumLock;\n\
              type \"ONE_LEVEL\" { modifiers = none; map[none] = Level1; level_name[Level1] = \"Any\"; }; };\n\
            xkb_compat \"c\" {\n\
              interpret Shift_L { action = SetMods(modifiers = Shift); };\n\
              interpret Shift_R { action = LockMods(modifiers = Shift); };\n\
            };\n\
            xkb_symbols \"s\" {\n\
              key <HELD> { [ Shift_L ] };\n\
              key <DEAD> { [ Control_L ] };\n\
              key <LOCK> { [ Shift_R ] };\n\
              modifier_map Shift { <HELD>, <LOCK> };\n\
            };\n\
        };\n";
        let context = xkb::Context::new(xkb::CONTEXT_NO_ENVIRONMENT_NAMES);
        let keymap = xkb::Keymap::new_from_string(
            &context,
            text.to_owned(),
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .expect("a small keymap");
        let keys: Vec<Keycode> = [50, 51, 52].into_iter().map(Keycode::new).collect();
        let found = modifier_keys(&keymap, &keys, 0);
        let names: Vec<&str> = found
            .iter()
            .map(|&(key, _, _)| keymap.key_get_name(key).unwrap_or("?"))
            .collect();
        assert_eq!(names, ["HELD"]);
        assert_eq!(found[0].1, Keysym::Shift_L);
    }
}
