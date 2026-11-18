//! AT-SPI's names of roles and states, as `atspi_role_get_name` and the state set give them:
//! `AtspiRole` and `AtspiStateType` of at-spi2-core 2.62.0 (`atspi/atspi-constants.h`), each
//! value's name lowercased, its words joined by spaces (`ATSPI_ROLE_CHECK_BOX` is `check box`).
//! Both enumerations number their values from 0 without gaps; at-spi2-core adds new ones at
//! their end.

/// The roles, by number: `GetRole` gives the number.
pub(super) const ROLES: [&str; 131] = [
    "invalid",
    "accelerator label",
    "alert",
    "animation",
    "arrow",
    "calendar",
    "canvas",
    "check box",
    "check menu item",
    "color chooser",
    "column header",
    "combo box",
    "date editor",
    "desktop icon",
    "desktop frame",
    "dial",
    "dialog",
    "directory pane",
    "drawing area",
    "file chooser",
    "filler",
    "focus traversable",
    "font chooser",
    "frame",
    "glass pane",
    "html container",
    "icon",
    "image",
    "internal frame",
    "label",
    "layered pane",
    "list",
    "list item",
    "menu",
    "menu bar",
    "menu item",
    "option pane",
    "page tab",
    "page tab list",
    "panel",
    "password text",
    "popup menu",
    "progress bar",
    "button",
    "radio button",
    "radio menu item",
    "root pane",
    "row header",
    "scroll bar",
    "scroll pane",
    "separator",
    "slider",
    "spin button",
    "split pane",
    "status bar",
    "table",
    "table cell",
    "table column header",
    "table row header",
    "tearoff menu item",
    "terminal",
    "text",
    "toggle button",
    "tool bar",
    "tool tip",
    "tree",
    "tree table",
    "unknown",
    "viewport",
    "window",
    "extended",
    "header",
    "footer",
    "paragraph",
    "ruler",
    "application",
    "autocomplete",
    "editbar",
    "embedded",
    "entry",
    "chart",
    "caption",
    "document frame",
    "heading",
    "page",
    "section",
    "redundant object",
    "form",
    "link",
    "input method window",
    "table row",
    "tree item",
    "document spreadsheet",
    "document presentation",
    "document text",
    "document web",
    "document email",
    "comment",
    "list box",
    "grouping",
    "image map",
    "notification",
    "info bar",
    "level bar",
    "title bar",
    "block quote",
    "audio",
    "video",
    "definition",
    "article",
    "landmark",
    "log",
    "marquee",
    "math",
    "rating",
    "timer",
    "static",
    "math fraction",
    "math root",
    "subscript",
    "superscript",
    "description list",
    "description term",
    "description value",
    "footnote",
    "content deletion",
    "content insertion",
    "mark",
    "suggestion",
    "push button menu",
    "switch",
];

/// The states, by bit: `GetState` gives them as two 32-bit words, bit `n` of the pair state
/// `n`.
pub(super) const STATES: [&str; 44] = [
    "invalid",
    "active",
    "armed",
    "busy",
    "checked",
    "collapsed",
    "defunct",
    "editable",
    "enabled",
    "expandable",
    "expanded",
    "focusable",
    "focused",
    "has tooltip",
    "horizontal",
    "iconified",
    "modal",
    "multi line",
    "multiselectable",
    "opaque",
    "pressed",
    "resizable",
    "selectable",
    "selected",
    "sensitive",
    "showing",
    "single line",
    "stale",
    "transient",
    "vertical",
    "visible",
    "manages descendants",
    "indeterminate",
    "required",
    "truncated",
    "animated",
    "invalid entry",
    "supports autocompletion",
    "selectable text",
    "is default",
    "visited",
    "checkable",
    "has popup",
    "read only",
];

/// The role desktops and the registry's root take: `desktop frame`.
pub(super) const DESKTOP_FRAME: u32 = 14;

/// A role's name; a number past the table, from a newer AT-SPI, as `role <n>`.
pub(super) fn role_name(role: u32) -> String {
    usize::try_from(role)
        .ok()
        .and_then(|index| ROLES.get(index))
        .map_or_else(|| format!("role {role}"), |name| (*name).to_owned())
}

/// Whether `name` is a role AT-SPI names.
pub(super) fn is_role(name: &str) -> bool {
    ROLES.contains(&name)
}

/// Whether `name` is a state AT-SPI names.
pub(super) fn is_state(name: &str) -> bool {
    STATES.contains(&name)
}

/// The names of the states set in `words`, as `GetState` gives them, in AT-SPI's order; a bit
/// past the table is left out.
pub(super) fn state_names(words: &[u32]) -> Vec<&'static str> {
    STATES
        .iter()
        .enumerate()
        .filter(|(bit, _)| {
            words
                .get(bit / 32)
                .is_some_and(|word| word & (1 << (bit % 32)) != 0)
        })
        .map(|(_, name)| *name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_and_states_are_named_as_at_spi_names_them() {
        // at-spi2-core's registry answers 14 for its own root, `desktop frame`.
        assert_eq!(role_name(DESKTOP_FRAME), "desktop frame");
        // ATSPI_ROLE_BUTTON, which GTK 4 and Qt give a push button.
        assert_eq!(role_name(43), "button");
        assert_eq!(role_name(75), "application");
        assert_eq!(role_name(130), "switch");
        assert_eq!(role_name(131), "role 131");
        assert_eq!(role_name(u32::MAX), format!("role {}", u32::MAX));
        assert!(is_role("push button menu") && !is_role("push button"));
        // `focused` is bit 12, `showing` 25 and `read only` 43, in the second word.
        assert_eq!(
            state_names(&[(1 << 12) | (1 << 25), 1 << 11]),
            ["focused", "showing", "read only"]
        );
        assert_eq!(state_names(&[]), Vec::<&str>::new());
        // A bit past the table, from a newer AT-SPI, is left out.
        assert_eq!(state_names(&[0, 1 << 31]), Vec::<&str>::new());
        assert!(is_state("visible") && !is_state("Visible"));
    }
}
