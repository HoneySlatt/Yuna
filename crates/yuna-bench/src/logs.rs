//! What the bench reads from the compositors' and the clients' own logs and counters: the frames
//! each compositor drew in a window, `weston-presentation-shm`'s presentation log, and mpv's
//! counters of dropped and delayed frames over its IPC.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// The value at percentile `p` of `sorted`, by the nearest rank; none if it is empty.
pub fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    Some(sorted[rank.clamp(1, sorted.len()) - 1])
}

/// KWin's frames in `[from_ns, to_ns]` of CLOCK_MONOTONIC, from its `KWIN_LOG_PERFORMANCE_DATA`
/// CSV (`src/core/renderloop.cpp`): a row per frame presented, its second column the time it was
/// presented. Gives the frames and the median of the sixth column, the refresh's length in ns.
pub fn kwin_frames(csv: &str, from_ns: u64, to_ns: u64) -> (u64, Option<f64>) {
    let mut frames = 0;
    let mut refreshes = Vec::new();
    for line in csv.lines().skip(1) {
        let columns: Vec<&str> = line.split(',').collect();
        let (Some(presented), Some(refresh)) = (
            columns.get(1).and_then(|c| c.trim().parse::<u64>().ok()),
            columns.get(5).and_then(|c| c.trim().parse::<f64>().ok()),
        ) else {
            continue;
        };
        if (from_ns..=to_ns).contains(&presented) {
            frames += 1;
            refreshes.push(refresh);
        }
    }
    refreshes.sort_by(f64::total_cmp);
    (frames, percentile(&refreshes, 50.0))
}

/// The output Yuna made, from its log line `virtual output created output=1 width=3840
/// height=2160 millihertz=240000 scale=1.5`, as `3840x2160@240.000,1.5`.
pub fn yuna_output(log: &str) -> Option<String> {
    let line = log
        .lines()
        .find(|line| line.contains("virtual output created"))?;
    let field = |key: &str| {
        line.split_whitespace()
            .find_map(|word| word.strip_prefix(key)?.strip_prefix('='))
    };
    let millihertz: f64 = field("millihertz")?.parse().ok()?;
    Some(format!(
        "{}x{}@{:.3},{}",
        field("width")?,
        field("height")?,
        millihertz / 1000.0,
        field("scale")?
    ))
}

/// An output as the run's file writes it: `3840x2160@240.000,1.5`.
fn output_text(width: u64, height: u64, hertz: f64, scale: f64) -> String {
    format!("{width}x{height}@{hertz:.3},{scale}")
}

/// The output `name` of `wlr-randr --json`, enabled, at its current mode.
pub fn wlr_randr_output(json: &str, name: &str) -> Option<String> {
    let outputs: serde_json::Value = serde_json::from_str(json).ok()?;
    let output = outputs
        .as_array()?
        .iter()
        .find(|output| output["name"] == name && output["enabled"] == true)?;
    let mode = output["modes"]
        .as_array()?
        .iter()
        .find(|mode| mode["current"] == true)?;
    Some(output_text(
        mode["width"].as_u64()?,
        mode["height"].as_u64()?,
        mode["refresh"].as_f64()?,
        output["scale"].as_f64()?,
    ))
}

/// The outputs of `umbriel outputs --json`: its reply's `ok`, or the reply itself.
fn umbriel_outputs(json: &str) -> Option<Vec<serde_json::Value>> {
    let reply: serde_json::Value = serde_json::from_str(json).ok()?;
    let outputs = if reply.get("ok").is_some() {
        &reply["ok"]
    } else {
        &reply
    };
    outputs.as_array().cloned()
}

/// The output `name` of `umbriel outputs --json`, enabled, at its current mode.
pub fn umbriel_output(json: &str, name: &str) -> Option<String> {
    let outputs = umbriel_outputs(json)?;
    let output = outputs
        .iter()
        .find(|output| output["name"] == name && output["enabled"] == true)?;
    let mode = output["modes"]
        .as_array()?
        .iter()
        .find(|mode| mode["current"] == true)?;
    Some(output_text(
        mode["width"].as_u64()?,
        mode["height"].as_u64()?,
        mode["refresh_mhz"].as_f64()? / 1000.0,
        output["scale"].as_f64()?,
    ))
}

/// The commits Umbriel counted on output `name` so far, from `umbriel outputs --json`: each
/// frame it drew or scanned out (`direct_scanout.commits`, docs/user/ipc.md).
pub fn umbriel_commits(json: &str, name: &str) -> Option<u64> {
    umbriel_outputs(json)?
        .iter()
        .find(|output| output["name"] == name)?["direct_scanout"]["commits"]
        .as_u64()
}

/// The monitor `name` of `hyprctl -j monitors all`, enabled, and every other monitor and
/// whether it is disabled: `3840x2160@240.000,1.5; WAYLAND-1 disabled`.
pub fn hyprctl_output(json: &str, name: &str) -> Option<String> {
    let monitors: serde_json::Value = serde_json::from_str(json).ok()?;
    let monitors = monitors.as_array()?;
    let monitor = monitors
        .iter()
        .find(|monitor| monitor["name"] == name && monitor["disabled"] == false)?;
    let mut text = output_text(
        monitor["width"].as_u64()?,
        monitor["height"].as_u64()?,
        monitor["refreshRate"].as_f64()?,
        monitor["scale"].as_f64()?,
    );
    for other in monitors.iter().filter(|other| other["name"] != name) {
        let state = if other["disabled"] == true {
            "disabled"
        } else {
            "enabled"
        };
        text.push_str(&format!(
            "; {} {state}",
            other["name"].as_str().unwrap_or("?")
        ));
    }
    Some(text)
}

/// The first monitor of `gdctl show`, its current mode and its logical monitor's scale:
/// `3840x2160@240.000,1.5`.
pub fn gdctl_output(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    // Each line is a tree's branch: `├──`, `└──` and `│` before its text.
    let bare = |line: &str| {
        line.trim_start_matches(|c: char| c.is_whitespace() || "│├└─".contains(c))
            .to_owned()
    };
    let current = lines.iter().position(|line| bare(line) == "Current mode")?;
    let mode = bare(lines.get(current + 1)?);
    let scale = lines
        .iter()
        .find_map(|line| bare(line).strip_prefix("Scale: ").map(str::to_owned))?;
    Some(format!("{mode},{scale}"))
}

/// The frame counter's frames in `[from_ns, to_ns]`: its `frame <ns>` lines.
pub fn counted_frames(log: &str, from_ns: u64, to_ns: u64) -> u64 {
    log.lines()
        .filter_map(|line| line.strip_prefix("frame ")?.trim().parse::<u64>().ok())
        .filter(|ns| (from_ns..=to_ns).contains(ns))
        .count() as u64
}

/// What `weston-presentation-shm`'s feedback mode printed: a line per frame presented,
/// `<n>: f2c <ms> ms, c2p <ms> ms, f2p <ms> ms, p2p <us> us, t2p <us>, [<flags>], seq <msc>`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PresentationLog {
    /// Each frame's time from the previous presentation to its own, `p2p`, in µs.
    pub between_us: Vec<i64>,
    /// The frames it saw discarded.
    pub discarded: u64,
    /// The lines that start as a frame's, `<n>:`, but whose `p2p` could not be read.
    pub unread: u64,
}

/// Reads a stretch of the presentation log.
pub fn parse_presentation_log(log: &str) -> PresentationLog {
    let mut read = PresentationLog::default();
    for line in log.lines().map(str::trim) {
        if line.starts_with("discarded ") {
            read.discarded += 1;
            continue;
        }
        let Some((number, rest)) = line.split_once(':') else {
            continue;
        };
        if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let between = rest
            .split(',')
            .map(str::trim)
            .find_map(|part| part.strip_prefix("p2p "))
            .and_then(|value| value.split_whitespace().next()?.parse().ok());
        match between {
            Some(us) => read.between_us.push(us),
            None => read.unread += 1,
        }
    }
    read
}

/// The properties of mpv read over its IPC: its counters of frames dropped and delayed.
pub const MPV_COUNTERS: [&str; 4] = [
    "frame-drop-count",
    "decoder-frame-drop-count",
    "vo-delayed-frame-count",
    "mistimed-frame-count",
];

/// Reads property `name` of the mpv whose IPC socket is `socket`, as a number.
pub fn mpv_property(socket: &Path, name: &str) -> Result<Option<f64>, String> {
    mpv_value(socket, name).map(|value| value.as_f64())
}

/// Reads property `name` of the mpv whose IPC socket is `socket`, as mpv gives it; `null` when
/// it is unavailable.
pub fn mpv_value(socket: &Path, name: &str) -> Result<serde_json::Value, String> {
    let mut stream = UnixStream::connect(socket).map_err(|e| format!("mpv's IPC: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| format!("mpv's IPC: {e}"))?;
    let request = serde_json::json!({"command": ["get_property", name], "request_id": 1});
    writeln!(stream, "{request}").map_err(|e| format!("mpv's IPC: {e}"))?;
    for line in BufReader::new(stream).lines() {
        let line = line.map_err(|e| format!("mpv's IPC: {e}"))?;
        let reply: serde_json::Value =
            serde_json::from_str(&line).map_err(|e| format!("mpv's IPC: {e}"))?;
        // Events come on the same socket; the reply carries the request's id.
        if reply["request_id"] == 1 {
            return Ok(reply["data"].clone());
        }
    }
    Err("mpv's IPC closed without a reply".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presentation_lines_are_read_and_the_unreadable_counted() {
        let log = "presentation-shm: feedback mode\n\
                   \u{20}    1: f2c  0 ms, c2p  4 ms, f2p  4 ms, p2p     0 us, t2p   4170, [s___], seq 7\n\
                   \u{20}    2: f2c  1 ms, c2p  3 ms, f2p  4 ms, p2p  4166 us, t2p  -12, [sc_z], seq 8\n\
                   \u{20}    3: f2c  1 ms, c2p  3 ms, f2p  4 ms, p2p    cut\n\
                   discarded 3\n\
                   presentation-shm exiting\n";
        assert_eq!(
            parse_presentation_log(log),
            PresentationLog {
                between_us: vec![0, 4166],
                discarded: 1,
                unread: 1,
            }
        );
    }

    #[test]
    fn kwins_rows_are_counted_by_their_presentation_time() {
        let csv = "target pageflip timestamp,pageflip timestamp,render start,render end,\
                   safety margin,refresh duration,vrr,tearing,predicted render time\n\
                   100,110,90,95,1000,4166666,0,0,500\n\
                   200,210,190,195,1000,4166666,0,0,500\n\
                   300,310,290,295,1000,4166667,0,0,500\n\
                   garbage\n";
        assert_eq!(kwin_frames(csv, 105, 250), (2, Some(4_166_666.0)));
        assert_eq!(kwin_frames(csv, 400, 500), (0, None));
    }

    #[test]
    fn yunas_output_and_the_counters_frames_are_read() {
        let log = "DEBUG yuna::frames: frame drawn output=1 took_us=200\n";
        let counter = "session 3840x2160\nframe 100\nframe 200\nframe 300\n";
        assert_eq!(counted_frames(counter, 150, 300), 2);
        let created = "INFO yuna::output: virtual output created output=1 width=3840 height=2160 \
                       millihertz=240000 scale=1.5\n";
        assert_eq!(
            yuna_output(created).as_deref(),
            Some("3840x2160@240.000,1.5")
        );
        assert_eq!(yuna_output(log), None);
    }

    #[test]
    fn wlr_randrs_output_is_read_at_its_current_mode() {
        // As wlr-randr 0.5.0 printed dwl's output once the bench set its mode (abridged).
        let json = r#"[{"name": "HEADLESS-1", "description": "Headless output 1",
            "enabled": true, "modes": [{"width": 3840, "height": 2160, "refresh": 240.000000,
            "preferred": false, "current": true}], "position": {"x": 0, "y": 0},
            "transform": "normal", "scale": 1.500000, "adaptive_sync": false}]"#;
        assert_eq!(
            wlr_randr_output(json, "HEADLESS-1").as_deref(),
            Some("3840x2160@240.000,1.5")
        );
        assert_eq!(wlr_randr_output(json, "HEADLESS-2"), None);
        // A disabled output, or one without a current mode, is not the bench's yet.
        let disabled = json.replace("\"enabled\": true", "\"enabled\": false");
        assert_eq!(wlr_randr_output(&disabled, "HEADLESS-1"), None);
        let no_mode = json.replace("\"current\": true", "\"current\": false");
        assert_eq!(wlr_randr_output(&no_mode, "HEADLESS-1"), None);
        assert_eq!(wlr_randr_output("not json", "HEADLESS-1"), None);
    }

    #[test]
    fn umbriels_output_and_commits_are_read() {
        // As `umbriel outputs --json` printed it at rest (abridged).
        let json = r#"[{"active_workspace":"HEADLESS-1:1","direct_scanout":{"active":false,
            "commit_seq":3,"commits":3,"reason":"composited","scanout_commits":0,"window":null},
            "enabled":true,"logical_size":{"height":1440,"width":2560},"modes":[{"current":true,
            "height":2160,"preferred":false,"refresh_mhz":240000,"width":3840}],
            "name":"HEADLESS-1","powered":true,"scale":1.5}]"#;
        assert_eq!(
            umbriel_output(json, "HEADLESS-1").as_deref(),
            Some("3840x2160@240.000,1.5")
        );
        assert_eq!(umbriel_commits(json, "HEADLESS-1"), Some(3));
        // The same in an IPC reply's `ok`; no count while the output is off.
        let reply = format!("{{\"ok\": {json}}}");
        assert_eq!(umbriel_commits(&reply, "HEADLESS-1"), Some(3));
        let off = json.replace(
            r#""direct_scanout":{"active":false,
            "commit_seq":3,"commits":3,"reason":"composited","scanout_commits":0,"window":null}"#,
            r#""direct_scanout":null"#,
        );
        assert_eq!(umbriel_commits(&off, "HEADLESS-1"), None);
        assert_eq!(umbriel_output(json, "DP-1"), None);
    }

    #[test]
    fn hyprlands_headless_monitor_is_read_with_its_parents_disabled() {
        // As `hyprctl -j monitors all` printed it (abridged): the window on its parent, and the
        // output the bench made.
        let json = r#"[{"id": 0, "name": "WAYLAND-1", "width": 1280, "height": 720,
            "refreshRate": 60.00000, "scale": 2, "disabled": true,
            "hardwareDetails": {"backend": "wayland"}},
            {"id": 1, "name": "HEADLESS-1", "width": 3840, "height": 2160,
            "refreshRate": 240.00000, "scale": 1.5, "disabled": false,
            "hardwareDetails": {"backend": "headless"}}]"#;
        assert_eq!(
            hyprctl_output(json, "HEADLESS-1").as_deref(),
            Some("3840x2160@240.000,1.5; WAYLAND-1 disabled")
        );
        let both = json.replace("\"disabled\": true", "\"disabled\": false");
        assert_eq!(
            hyprctl_output(&both, "HEADLESS-1").as_deref(),
            Some("3840x2160@240.000,1.5; WAYLAND-1 enabled")
        );
        assert_eq!(hyprctl_output(json, "FALLBACK"), None);
    }

    #[test]
    fn gdctls_current_mode_and_scale_are_read() {
        // As Mutter 51.0's `gdctl show` printed its virtual monitor at the bench's scale.
        let shown = "Monitors:\n\
                     └──Monitor Meta-0 (MetaVendor)\n\
                     \u{20}  ├──Vendor: MetaVendor\n\
                     \u{20}  ├──Product: MetaVirtualMonitor\n\
                     \u{20}  ├──Serial: 0x00\n\
                     \u{20}  ├──Current mode\n\
                     \u{20}  │   └──3840x2160@240.000\n\
                     \u{20}  └──Preferences\n\
                     \u{20}      └──Backlight: None\n\
                     \n\
                     Logical monitors:\n\
                     └──Logical monitor #1\n\
                     \u{20}  ├──Position: (0, 0)\n\
                     \u{20}  ├──Scale: 1.5\n\
                     \u{20}  ├──Transform: normal\n";
        assert_eq!(
            gdctl_output(shown).as_deref(),
            Some("3840x2160@240.000,1.5")
        );
        assert_eq!(gdctl_output("Monitors:\n"), None);
    }

    #[test]
    fn percentiles_take_the_nearest_rank() {
        let values: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&values, 50.0), Some(50.0));
        assert_eq!(percentile(&values, 99.0), Some(99.0));
        assert_eq!(percentile(&[7.0], 99.0), Some(7.0));
        assert_eq!(percentile(&[], 50.0), None);
    }
}
