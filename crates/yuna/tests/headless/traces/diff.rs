//! A unified diff of two texts, line by line, for a trace that differs from the one held: only
//! the lines that differ, with three lines of context, as `diff -u` shows them, each hunk's
//! header naming the client and the object its first change falls in, as `diff -p` names a
//! function.

/// Lines of context around each change.
const CONTEXT: usize = 3;
/// The most cells the table of common lines may have, past which the lines between the common
/// start and end are shown all removed then all added: 16 MiB of `u32`, for two traces of
/// 2,000 lines that differ all through.
const MAX_CELLS: usize = 4 << 20;

/// One line of the diff: kept, removed from the held text, or added by the new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line<'a> {
    Kept(&'a str),
    Removed(&'a str),
    Added(&'a str),
}

/// The unified diff from `held` to `now`, `---` and `+++` named after them; empty when they are
/// the same.
pub fn unified_diff(held: &str, now: &str) -> String {
    let held: Vec<&str> = held.lines().collect();
    let now: Vec<&str> = now.lines().collect();
    let lines = diff_lines(&held, &now);
    let mut text = String::new();
    // Each hunk is a range of `lines` around changes closer than twice the context.
    let changed: Vec<usize> = (0..lines.len())
        .filter(|&index| !matches!(lines[index], Line::Kept(_)))
        .collect();
    let mut index = 0;
    while index < changed.len() {
        let first = changed[index];
        let mut last = first;
        while index + 1 < changed.len() && changed[index + 1] - last <= 2 * CONTEXT {
            index += 1;
            last = changed[index];
        }
        index += 1;
        let from = first.saturating_sub(CONTEXT);
        let to = (last + CONTEXT + 1).min(lines.len());
        push_hunk(&mut text, &lines, (from, to), &heading(&lines[..=first]));
    }
    if text.is_empty() {
        return text;
    }
    format!("--- held\n+++ now\n{text}")
}

/// What the last of `lines` falls in, as a trace's headings say: the last `client` line and the
/// last object's line before it, the object named alone, without the message that made it.
fn heading<'a>(lines: &[Line<'a>]) -> String {
    let content = |line: &Line<'a>| -> &'a str {
        match *line {
            Line::Kept(text) | Line::Removed(text) | Line::Added(text) => text,
        }
    };
    let mut client = None;
    let mut object = None;
    for line in lines.iter().rev().map(content) {
        if line.starts_with(' ') || line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix("client ") {
            client = Some(name);
            break;
        }
        if object.is_none() {
            object = line.split(" = ").next();
        }
    }
    match (client, object) {
        (Some(client), Some(object)) => format!(" client {client}: {object}"),
        (Some(client), None) => format!(" client {client}"),
        (None, Some(object)) => format!(" {object}"),
        (None, None) => String::new(),
    }
}

/// Writes the hunk of `lines[from..to]` with its header: where it starts in each text, from 1,
/// how many lines of each it holds, and what it falls in.
fn push_hunk(text: &mut String, lines: &[Line<'_>], (from, to): (usize, usize), heading: &str) {
    let in_held = |line: &Line<'_>| !matches!(line, Line::Added(_));
    let in_now = |line: &Line<'_>| !matches!(line, Line::Removed(_));
    let held_start = lines[..from].iter().filter(|line| in_held(line)).count();
    let now_start = lines[..from].iter().filter(|line| in_now(line)).count();
    let held_count = lines[from..to].iter().filter(|line| in_held(line)).count();
    let now_count = lines[from..to].iter().filter(|line| in_now(line)).count();
    text.push_str(&format!(
        "@@ -{},{held_count} +{},{now_count} @@{heading}\n",
        held_start + usize::from(held_count > 0),
        now_start + usize::from(now_count > 0),
    ));
    for line in &lines[from..to] {
        let (sign, content) = match line {
            Line::Kept(content) => (' ', content),
            Line::Removed(content) => ('-', content),
            Line::Added(content) => ('+', content),
        };
        text.push(sign);
        text.push_str(content);
        text.push('\n');
    }
}

/// Every line of both texts, kept, removed or added, through their longest common subsequence
/// once their common start and end are set aside.
fn diff_lines<'a>(held: &[&'a str], now: &[&'a str]) -> Vec<Line<'a>> {
    let common_start = held
        .iter()
        .zip(now)
        .take_while(|(held, now)| held == now)
        .count();
    let common_end = held[common_start..]
        .iter()
        .rev()
        .zip(now[common_start..].iter().rev())
        .take_while(|(held, now)| held == now)
        .count();
    let held_middle = &held[common_start..held.len() - common_end];
    let now_middle = &now[common_start..now.len() - common_end];
    let mut lines: Vec<Line<'a>> = held[..common_start].iter().map(|l| Line::Kept(l)).collect();
    lines.extend(middle_lines(held_middle, now_middle));
    lines.extend(
        held[held.len() - common_end..]
            .iter()
            .map(|line| Line::Kept(line)),
    );
    lines
}

/// The lines of two texts that differ at both ends, through the table of their longest common
/// subsequences, or all removed then all added past [`MAX_CELLS`].
fn middle_lines<'a>(held: &[&'a str], now: &[&'a str]) -> Vec<Line<'a>> {
    let (rows, columns) = (held.len() + 1, now.len() + 1);
    if rows.saturating_mul(columns) > MAX_CELLS {
        let removed = held.iter().map(|line| Line::Removed(line));
        return removed
            .chain(now.iter().map(|line| Line::Added(line)))
            .collect();
    }
    // common[i][j]: the length of the longest common subsequence of held[i..] and now[j..].
    let mut common = vec![0_u32; rows * columns];
    for i in (0..held.len()).rev() {
        for j in (0..now.len()).rev() {
            common[i * columns + j] = if held[i] == now[j] {
                common[(i + 1) * columns + j + 1] + 1
            } else {
                common[(i + 1) * columns + j].max(common[i * columns + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut lines = Vec::with_capacity(held.len() + now.len());
    while i < held.len() || j < now.len() {
        if i < held.len() && j < now.len() && held[i] == now[j] {
            lines.push(Line::Kept(held[i]));
            (i, j) = (i + 1, j + 1);
        } else if i < held.len()
            && (j == now.len() || common[(i + 1) * columns + j] >= common[i * columns + j + 1])
        {
            // Removed lines first, as `diff -u` shows them.
            lines.push(Line::Removed(held[i]));
            i += 1;
        } else {
            lines.push(Line::Added(now[j]));
            j += 1;
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_texts_have_no_diff() {
        assert_eq!(unified_diff("a\nb\n", "a\nb\n"), "");
        assert_eq!(unified_diff("", ""), "");
    }

    /// One line changed among many, no heading above it: its hunk holds three lines of context on
    /// each side, and says where it starts in each text.
    #[test]
    fn a_changed_line_shows_with_its_context() {
        let held: String = (1..=20).map(|n| format!(" {n}\n")).collect();
        let now = held.replace(" 10\n", " ten\n");
        assert_eq!(
            unified_diff(&held, &now),
            "--- held\n+++ now\n@@ -7,7 +7,7 @@\n  7\n  8\n  9\n- 10\n+ ten\n  11\n  12\n  13\n"
        );
    }

    /// A line added, and two changes far apart: two hunks.
    #[test]
    fn changes_far_apart_make_two_hunks() {
        let held: String = (1..=20).map(|n| format!(" {n}\n")).collect();
        let now: String = (1..=20)
            .filter(|&n| n != 18)
            .map(|n| {
                if n == 2 {
                    " 2\n new\n".to_owned()
                } else {
                    format!(" {n}\n")
                }
            })
            .collect();
        assert_eq!(
            unified_diff(&held, &now),
            "--- held\n+++ now\n\
             @@ -1,5 +1,6 @@\n  1\n  2\n+ new\n  3\n  4\n  5\n\
             @@ -15,6 +16,5 @@\n  15\n  16\n  17\n- 18\n  19\n  20\n"
        );
    }

    /// A hunk's header names the client and the object its first change falls in.
    #[test]
    fn a_hunk_names_the_client_and_the_object_it_falls_in() {
        let held = "# a trace\nclient a\nwl_display#1\n  -> get_registry(new wl_registry#1)\n\
                    wl_surface#1 = wl_compositor#1.create_surface\n  -> commit()\n  -> commit()\n";
        let now = held.replacen(
            "  -> commit()\n  -> commit()",
            "  -> commit()\n  -> attach()",
            1,
        );
        let diff = unified_diff(held, &now);
        assert!(
            diff.contains("@@ -4,4 +4,4 @@ client a: wl_surface#1\n"),
            "{diff}"
        );
        assert_eq!(heading(&[Line::Kept("# a trace")]), "");
    }

    /// A held file that does not exist yet reads empty: every line is added.
    #[test]
    fn a_new_text_is_all_added() {
        assert_eq!(
            unified_diff("", "  a\n  b\n"),
            "--- held\n+++ now\n@@ -0,0 +1,2 @@\n+  a\n+  b\n"
        );
    }
}
