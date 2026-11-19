use super::*;

fn normalised(raw: &str, clock: Clock) -> String {
    normalise(raw, clock).unwrap_or_else(|why| panic!("the trace is not normalised: {why}"))
}

/// A client that binds `wl_compositor` and makes a surface: the start most tests share.
const BOUND: &str = "\
[ 123456.789][rs] -> wl_display@1.get_registry(wl_registry@2)
[1234567.000][rs] <- wl_registry@2.global, (7, Some(\"wl_compositor\"), 6)
[1.000][rs] -> wl_registry@2.bind(7, Some(\"wl_compositor\"), 6, wl_compositor@3)
[1.000][rs] -> wl_compositor@3.create_surface(wl_surface@4)
";

/// The objects of [`BOUND`], as the trace holds them, followed by `surface`'s lines for the
/// surface.
fn bound(surface: &str) -> String {
    format!(
        "wl_compositor#1 = wl_registry#1.bind\n  -> create_surface(new wl_surface#1)\n\
         wl_display#1\n  -> get_registry(new wl_registry#1)\n\
         wl_registry#1 = wl_display#1.get_registry\n  \
         <- global(name of wl_compositor#1, \"wl_compositor\", 6)\n  \
         -> bind(name of wl_compositor#1, \"wl_compositor\", 6, new wl_compositor#1)\n\
         wl_surface#1 = wl_compositor#1.create_surface\n{surface}"
    )
}

/// The timestamp goes, an event's stray comma with it, and each argument is read by its type and
/// written in the trace's own format: a string unquoted from `Debug` and quoted again, a fixed
/// number exactly, a null object, an array of 32-bit values.
#[test]
fn a_message_is_read_by_its_types_and_written_in_the_traces_own_format() {
    let raw = format!(
        "{BOUND}\
         [1.000][rs] <- wl_surface@4.preferred_buffer_scale, (2)\n\
         [1.000][rs] -> wl_surface@4.attach(<anonymous>@0, 0, 0)\n"
    );
    assert_eq!(
        normalised(&raw, Clock::Real),
        bound("  <- preferred_buffer_scale(2)\n  -> attach(null, 0, 0)\n")
    );
    let interfaces = known_interfaces();
    let fixed = read_value(ArgumentType::Fixed, "0.0039", &interfaces);
    assert_eq!(fixed, Ok(Value::Fixed(1)));
    let fixed = read_value(ArgumentType::Fixed, "-30.2500", &interfaces);
    assert_eq!(fixed, Ok(Value::Fixed(-30 * 256 - 64)));
    let array = read_value(ArgumentType::Array, "[4, 0, 0, 0, 1, 0, 0, 0]", &interfaces);
    assert_eq!(array, Ok(Value::Array(vec![4, 0, 0, 0, 1, 0, 0, 0])));
    assert_eq!(array_text(&[4, 0, 0, 0, 1, 0, 0, 0]), "[4, 1]");
    assert_eq!(array_text(&[1, 2, 3]), "bytes[1, 2, 3]");
}

/// A string comes back byte for byte from what `Debug` writes of a `CStr`: quotes,
/// backslashes, controls, characters past ASCII and bytes that are no UTF-8.
#[test]
fn a_strings_escapes_are_read_back() {
    let bytes = b"a \"b\" \\ \t\n\x7f \xc3\xa9 \xff".to_vec();
    let string = std::ffi::CString::new(bytes.clone()).expect("no NUL inside");
    let debug = format!("{:?}", string.as_c_str());
    assert_eq!(unescape(&debug), Some(bytes));
    assert_eq!(unescape(r#""\0\u{e9}""#), Some(b"\0\xc3\xa9".to_vec()));
    assert_eq!(unescape(r#""\q""#), None);
    assert_eq!(unescape("no quotes"), None);
}

/// The harness's roundtrips are left out, their callbacks, `done` and `delete_id` with them; a
/// frame callback is ranked among frame callbacks alone; `delete_id` is held on the object it
/// deletes, and an id used again after its deletion is a new object.
#[test]
fn roundtrips_are_left_out_and_an_id_used_again_is_a_new_object() {
    let raw = format!(
        "{BOUND}\
         [1.000][rs] -> wl_display@1.sync(wl_callback@5)\n\
         [1.000][rs] <- wl_callback@5.done, (0)\n\
         [1.000][rs] <- wl_display@1.delete_id, (5)\n\
         [1.000][rs] -> wl_surface@4.frame(wl_callback@5)\n\
         [1.000][rs] <- wl_callback@5.done, (7)\n\
         [1.000][rs] <- wl_display@1.delete_id, (5)\n\
         [1.000][rs] -> wl_surface@4.frame(wl_callback@5)\n"
    );
    assert_eq!(
        normalised(&raw, Clock::Real),
        "wl_callback#1 = wl_surface#1.frame\n  <- done(<time>)\n  <- delete_id\n\
         wl_callback#2 = wl_surface#1.frame\n"
            .to_owned()
            + &bound("  -> frame(new wl_callback#1)\n  -> frame(new wl_callback#2)\n")
    );
    let deleted = "[1.000][rs] <- wl_display@1.delete_id, (3)\n";
    assert!(normalise(deleted, Clock::Real).is_err());
}

/// Two objects' messages, interleaved in two ways: each object holds its own in order, and the
/// traces are the same; one object's own order is held.
#[test]
fn each_object_holds_its_own_messages_whatever_their_interleaving() {
    let surface = "[1.000][rs] <- wl_surface@4.preferred_buffer_scale, (2)\n";
    let registry = "[1.000][rs] <- wl_registry@2.global, (9, Some(\"wl_shm\"), 2)\n";
    let commit = "[1.000][rs] -> wl_surface@4.commit()\n";
    let one = format!("{BOUND}{surface}{registry}{commit}");
    let other = format!("{BOUND}{registry}{surface}{commit}");
    assert_eq!(
        normalised(&one, Clock::Real),
        normalised(&other, Clock::Real)
    );
    let swapped = format!("{BOUND}{commit}{registry}{surface}");
    assert_ne!(
        normalised(&one, Clock::Real),
        normalised(&swapped, Clock::Real)
    );
}

/// Globals announced in another order, under other registry names, bound in that order under
/// other ids: the trace is the same. Two outputs of one registry are told apart by what they
/// said, not by the order they came in.
#[test]
fn the_order_globals_come_in_shows_nowhere() {
    let output = |id: u32, name: &str| {
        format!(
            "[1.000][rs] <- wl_output@{id}.name, (Some(\"{name}\"))\n\
             [1.000][rs] <- wl_output@{id}.done, ()\n"
        )
    };
    let announced = |globals: &[(u32, &str, u32)]| {
        let mut raw = "[1.000][rs] -> wl_display@1.get_registry(wl_registry@2)\n".to_owned();
        for &(name, interface, version) in globals {
            raw += &format!(
                "[1.000][rs] <- wl_registry@2.global, ({name}, Some(\"{interface}\"), {version})\n"
            );
        }
        for (id, &(name, interface, version)) in (3..).zip(globals) {
            let bound = version.min(4);
            raw += &format!(
                "[1.000][rs] -> wl_registry@2.bind({name}, Some(\"{interface}\"), {bound}, {interface}@{id})\n"
            );
        }
        raw
    };
    let one = announced(&[(9, "wl_shm", 2), (26, "wl_output", 4), (27, "wl_output", 4)])
        + &output(4, "HEADLESS-1")
        + &output(5, "HEADLESS-2")
        + "[1.000][rs] <- wl_registry@2.global_remove, (27)\n";
    let other = announced(&[(30, "wl_output", 4), (31, "wl_output", 4), (2, "wl_shm", 2)])
        + &output(3, "HEADLESS-2")
        + &output(4, "HEADLESS-1")
        + "[1.000][rs] <- wl_registry@2.global_remove, (30)\n";
    let trace = normalised(&one, Clock::Real);
    assert_eq!(trace, normalised(&other, Clock::Real));
    assert!(
        trace.contains("wl_output#2 = wl_registry#1.bind\n  <- name(\"HEADLESS-2\")\n"),
        "{trace}"
    );
    assert!(
        trace.contains("  <- global_remove(name of wl_output#2)\n"),
        "{trace}"
    );
}

/// A descriptor is found by the message's description; a keymap's size and an error's text are
/// masked, the error's object and code kept.
#[test]
fn a_descriptor_a_keymaps_size_and_an_errors_text_are_masked() {
    let raw = "[1.000][rs] -> wl_display@1.get_registry(wl_registry@2)\n\
               [1.000][rs] <- wl_registry@2.global, (9, Some(\"wl_shm\"), 2)\n\
               [1.000][rs] -> wl_registry@2.bind(9, Some(\"wl_shm\"), 1, wl_shm@3)\n\
               [1.000][rs] -> wl_shm@3.create_pool(wl_shm_pool@4, 23, 16)\n\
               [1.000][rs] <- wl_display@1.error, (wl_shm@3, 1, Some(\"invalid format 7\"))\n";
    let trace = normalised(raw, Clock::Real);
    assert!(
        trace.contains("  -> create_pool(new wl_shm_pool#1, <fd>, 16)\n"),
        "{trace}"
    );
    assert!(
        trace.contains("  <- error(wl_shm#1, 1, <message>)\n"),
        "{trace}"
    );
    let keymap = "[1.000][rs] -> wl_display@1.get_registry(wl_registry@2)\n\
                  [1.000][rs] <- wl_registry@2.global, (1, Some(\"wl_seat\"), 9)\n\
                  [1.000][rs] -> wl_registry@2.bind(1, Some(\"wl_seat\"), 1, wl_seat@3)\n\
                  [1.000][rs] -> wl_seat@3.get_keyboard(wl_keyboard@4)\n\
                  [1.000][rs] <- wl_keyboard@4.keymap, (1, 31, 35758)\n";
    let trace = normalised(keymap, Clock::Real);
    assert!(
        trace.contains("  <- keymap(1, <fd>, <keymap size>)\n"),
        "{trace}"
    );
}

/// The start of a client with a toplevel's `xdg_surface`, for the serials' tests.
const XDG_SURFACE: &str = "\
[1.000][rs] <- wl_registry@2.global, (2, Some(\"xdg_wm_base\"), 7)
[1.000][rs] -> wl_registry@2.bind(2, Some(\"xdg_wm_base\"), 7, xdg_wm_base@6)
[1.000][rs] -> xdg_wm_base@6.get_xdg_surface(xdg_surface@7, wl_surface@4)
";

/// A serial is named by the event that first carried it, which says `serial`; a request that
/// sends it back names that event, and a serial never sent says so. One serial more, a ping,
/// changes the lines of its own object alone.
#[test]
fn a_serial_is_named_by_the_event_that_carried_it() {
    let configures = "[1.000][rs] <- xdg_surface@7.configure, (4021)\n\
                      [1.000][rs] <- xdg_surface@7.configure, (4022)\n\
                      [1.000][rs] -> xdg_surface@7.ack_configure(4022)\n\
                      [1.000][rs] <- xdg_wm_base@6.ping, (4030)\n\
                      [1.000][rs] -> xdg_wm_base@6.pong(4030)\n\
                      [1.000][rs] -> xdg_wm_base@6.pong(77)\n";
    let raw = format!("{BOUND}{XDG_SURFACE}{configures}");
    let trace = normalised(&raw, Clock::Real);
    let surface = "xdg_surface#1 = xdg_wm_base#1.get_xdg_surface\n  <- configure(serial)\n  \
                   <- configure(serial)\n  -> ack_configure(serial of xdg_surface#1.configure[2])\n";
    assert!(trace.contains(surface), "{trace}");
    assert!(
        trace.contains(
            "  <- ping(serial)\n  -> pong(serial of xdg_wm_base#1.ping[1])\n  \
             -> pong(a serial this client was never sent)\n"
        ),
        "{trace}"
    );
    let one_more =
        format!("{BOUND}{XDG_SURFACE}[1.000][rs] <- xdg_wm_base@6.ping, (4000)\n{configures}");
    let other = normalised(&one_more, Clock::Real);
    assert_ne!(trace, other);
    assert!(other.contains(surface), "{other}");
    let changed = unified_lines(&trace, &other);
    assert!(
        changed
            .iter()
            .all(|line| line.contains("ping") || line.contains("pong")),
        "{changed:?}"
    );
}

/// The lines one text adds or removes against another, as `+line` and `-line`.
fn unified_lines(held: &str, now: &str) -> Vec<String> {
    super::super::diff::unified_diff(held, now)
        .lines()
        .filter(|line| {
            (line.starts_with('+') || line.starts_with('-'))
                && !line.starts_with("+++")
                && !line.starts_with("---")
        })
        .map(str::to_owned)
        .collect()
}

/// The earliest frame callback's time is the origin of the manual clock's times: a presentation
/// 3.166666 ms after it, its refresh count kept. On the real clock, every time and count is
/// masked.
#[test]
fn times_count_from_the_first_frame_on_the_manual_clock_and_are_masked_on_the_real_one() {
    let raw = format!(
        "{BOUND}\
         [1.000][rs] -> wl_surface@4.frame(wl_callback@5)\n\
         [1.000][rs] -> wl_surface@4.frame(wl_callback@6)\n\
         [1.000][rs] <- wl_callback@6.done, (1000005)\n\
         [1.000][rs] <- wl_callback@5.done, (1000001)\n\
         [1.000][rs] <- wl_registry@2.global, (3, Some(\"wp_presentation\"), 2)\n\
         [1.000][rs] -> wl_registry@2.bind(3, Some(\"wp_presentation\"), 1, wp_presentation@7)\n\
         [1.000][rs] -> wp_presentation@7.feedback(wl_surface@4, wp_presentation_feedback@8)\n\
         [1.000][rs] <- wp_presentation_feedback@8.presented, (0, 1000, 4166666, 4166666, 0, 1, 1)\n"
    );
    let manual = normalised(&raw, Clock::Manual);
    assert!(
        manual.contains(
            "wl_callback#1 = wl_surface#1.frame\n  <- done(t+0ms)\n\
             wl_callback#2 = wl_surface#1.frame\n  <- done(t+4ms)\n"
        ),
        "{manual}"
    );
    assert!(
        manual.contains("  <- presented(t+3.166666ms, 4166666, 1, 1)\n"),
        "{manual}"
    );
    let real = normalised(&raw, Clock::Real);
    assert!(
        real.contains("= wl_surface#1.frame\n  <- done(<time>)\n"),
        "{real}"
    );
    assert!(
        real.contains("  <- presented(<time>, 4166666, <count>, 1)\n"),
        "{real}"
    );
}

/// A time in milliseconds before the first frame, and across the 32-bit clock's wrap, reads as
/// the signed difference.
#[test]
fn a_time_before_the_first_frame_and_across_the_wrap_is_signed() {
    let raw = "[1.000][rs] -> wl_display@1.get_registry(wl_registry@2)\n\
               [1.000][rs] <- wl_registry@2.global, (1, Some(\"wl_seat\"), 9)\n\
               [1.000][rs] <- wl_registry@2.global, (3, Some(\"wl_compositor\"), 6)\n\
               [1.000][rs] -> wl_registry@2.bind(1, Some(\"wl_seat\"), 1, wl_seat@3)\n\
               [1.000][rs] -> wl_registry@2.bind(3, Some(\"wl_compositor\"), 6, wl_compositor@4)\n\
               [1.000][rs] -> wl_seat@3.get_pointer(wl_pointer@5)\n\
               [1.000][rs] <- wl_pointer@5.motion, (4294967294, 10.0000, 20.5000)\n\
               [1.000][rs] -> wl_compositor@4.create_surface(wl_surface@6)\n\
               [1.000][rs] -> wl_surface@6.frame(wl_callback@7)\n\
               [1.000][rs] <- wl_callback@7.done, (3)\n";
    let trace = normalised(raw, Clock::Manual);
    assert!(trace.contains("  <- motion(t-5ms, 10, 20.5)\n"), "{trace}");
    assert!(trace.contains("  <- done(t+0ms)\n"), "{trace}");
}

/// On the manual clock, a time needs a frame callback to count from; on the real clock it does
/// not.
#[test]
fn a_manual_time_needs_a_frame_callback() {
    let raw = "[1.000][rs] -> wl_display@1.get_registry(wl_registry@2)\n\
               [1.000][rs] <- wl_registry@2.global, (1, Some(\"wl_seat\"), 9)\n\
               [1.000][rs] -> wl_registry@2.bind(1, Some(\"wl_seat\"), 1, wl_seat@3)\n\
               [1.000][rs] -> wl_seat@3.get_pointer(wl_pointer@5)\n\
               [1.000][rs] <- wl_pointer@5.motion, (2, 10.0000, 20.0000)\n";
    assert!(normalise(raw, Clock::Manual).is_err());
    assert!(normalise(raw, Clock::Real).is_ok());
}

/// The orders between objects the protocol requires are held: a toplevel's events and its
/// decoration's before their `xdg_surface`'s configure, an `xdg-output`'s before its
/// `wl_output`'s `done` from version 3 alone, an offer's mime types before the selection that
/// names it.
#[test]
fn the_orders_the_protocol_requires_between_objects_are_held() {
    let raw = format!(
        "{BOUND}\
         [1.000][rs] <- wl_registry@2.global, (2, Some(\"xdg_wm_base\"), 7)\n\
         [1.000][rs] <- wl_registry@2.global, (3, Some(\"zxdg_decoration_manager_v1\"), 1)\n\
         [1.000][rs] -> wl_registry@2.bind(2, Some(\"xdg_wm_base\"), 7, xdg_wm_base@5)\n\
         [1.000][rs] -> wl_registry@2.bind(3, Some(\"zxdg_decoration_manager_v1\"), 1, zxdg_decoration_manager_v1@6)\n\
         [1.000][rs] -> xdg_wm_base@5.get_xdg_surface(xdg_surface@7, wl_surface@4)\n\
         [1.000][rs] -> xdg_surface@7.get_toplevel(xdg_toplevel@8)\n\
         [1.000][rs] -> zxdg_decoration_manager_v1@6.get_toplevel_decoration(zxdg_toplevel_decoration_v1@9, xdg_toplevel@8)\n\
         [1.000][rs] <- zxdg_toplevel_decoration_v1@9.configure, (2)\n\
         [1.000][rs] <- xdg_toplevel@8.configure, (10, 20, [])\n\
         [1.000][rs] <- xdg_surface@7.configure, (5)\n\
         [1.000][rs] <- xdg_toplevel@8.configure, (30, 40, [])\n\
         [1.000][rs] <- xdg_surface@7.configure, (6)\n\
         [1.000][rs] <- xdg_toplevel@8.close, ()\n"
    );
    let trace = normalised(&raw, Clock::Real);
    assert!(
        trace.contains(
            "  <- configure(10, 20, []), before xdg_surface#1.configure[1]\n  \
             <- configure(30, 40, []), before xdg_surface#1.configure[2]\n  <- close()\n"
        ),
        "{trace}"
    );
    assert!(
        trace.contains("  <- configure(2), before xdg_surface#1.configure[1]\n"),
        "{trace}"
    );

    let outputs = |version: u32| {
        format!(
            "[1.000][rs] -> wl_display@1.get_registry(wl_registry@2)\n\
             [1.000][rs] <- wl_registry@2.global, (8, Some(\"zxdg_output_manager_v1\"), 3)\n\
             [1.000][rs] <- wl_registry@2.global, (26, Some(\"wl_output\"), 4)\n\
             [1.000][rs] -> wl_registry@2.bind(8, Some(\"zxdg_output_manager_v1\"), {version}, zxdg_output_manager_v1@3)\n\
             [1.000][rs] -> wl_registry@2.bind(26, Some(\"wl_output\"), 4, wl_output@4)\n\
             [1.000][rs] -> zxdg_output_manager_v1@3.get_xdg_output(zxdg_output_v1@5, wl_output@4)\n\
             [1.000][rs] <- wl_output@4.done, ()\n\
             [1.000][rs] <- zxdg_output_v1@5.logical_size, (2560, 1440)\n\
             [1.000][rs] <- wl_output@4.done, ()\n"
        )
    };
    let trace = normalised(&outputs(3), Clock::Real);
    assert!(
        trace.contains("  <- logical_size(2560, 1440), before wl_output#1.done[2]\n"),
        "{trace}"
    );
    let trace = normalised(&outputs(2), Clock::Real);
    assert!(trace.contains("  <- logical_size(2560, 1440)\n"), "{trace}");

    let selection = "[1.000][rs] -> wl_display@1.get_registry(wl_registry@2)\n\
                     [1.000][rs] <- wl_registry@2.global, (1, Some(\"wl_seat\"), 9)\n\
                     [1.000][rs] <- wl_registry@2.global, (11, Some(\"wl_data_device_manager\"), 3)\n\
                     [1.000][rs] -> wl_registry@2.bind(1, Some(\"wl_seat\"), 1, wl_seat@3)\n\
                     [1.000][rs] -> wl_registry@2.bind(11, Some(\"wl_data_device_manager\"), 3, wl_data_device_manager@4)\n\
                     [1.000][rs] -> wl_data_device_manager@4.get_data_device(wl_data_device@5, wl_seat@3)\n\
                     [1.000][rs] <- wl_data_device@5.data_offer, (wl_data_offer@4278190080)\n\
                     [1.000][rs] <- wl_data_offer@4278190080.offer, (Some(\"text/plain\"))\n\
                     [1.000][rs] <- wl_data_device@5.selection, (wl_data_offer@4278190080)\n";
    let trace = normalised(selection, Clock::Real);
    assert!(
        trace.contains(
            "wl_data_offer#1 = wl_data_device#1.data_offer\n  \
             <- offer(\"text/plain\"), before wl_data_device#1.selection[1]\n"
        ),
        "{trace}"
    );
}

/// Sets are sorted: a run of formats on one object, the registry's globals and the binds. A run
/// ends at another message. A run of `linux-dmabuf`'s formats is one line.
#[test]
fn sets_are_sorted_and_the_renderers_formats_collapsed() {
    let raw = "[1.000][rs] -> wl_display@1.get_registry(wl_registry@2)\n\
               [1.000][rs] <- wl_registry@2.global, (9, Some(\"wl_shm\"), 2)\n\
               [1.000][rs] <- wl_registry@2.global, (2, Some(\"zwp_linux_dmabuf_v1\"), 6)\n\
               [1.000][rs] -> wl_registry@2.bind(9, Some(\"wl_shm\"), 1, wl_shm@3)\n\
               [1.000][rs] -> wl_registry@2.bind(2, Some(\"zwp_linux_dmabuf_v1\"), 3, zwp_linux_dmabuf_v1@4)\n\
               [1.000][rs] <- wl_shm@3.format, (875713089)\n\
               [1.000][rs] <- wl_shm@3.format, (1)\n\
               [1.000][rs] <- wl_shm@3.format, (0)\n\
               [1.000][rs] <- zwp_linux_dmabuf_v1@4.modifier, (875713089, 0, 0)\n\
               [1.000][rs] <- zwp_linux_dmabuf_v1@4.modifier, (875713089, 16777215, 4294967295)\n\
               [1.000][rs] <- wl_registry@2.global_remove, (9)\n";
    assert_eq!(
        normalised(raw, Clock::Real),
        "wl_display#1\n  -> get_registry(new wl_registry#1)\n\
         wl_registry#1 = wl_display#1.get_registry\n\
         \x20 <- global(name of wl_shm#1, \"wl_shm\", 2)\n\
         \x20 <- global(name of zwp_linux_dmabuf_v1#1, \"zwp_linux_dmabuf_v1\", 6)\n\
         \x20 -> bind(name of wl_shm#1, \"wl_shm\", 1, new wl_shm#1)\n\
         \x20 -> bind(name of zwp_linux_dmabuf_v1#1, \"zwp_linux_dmabuf_v1\", 3, new zwp_linux_dmabuf_v1#1)\n\
         \x20 <- global_remove(name of wl_shm#1)\n\
         wl_shm#1 = wl_registry#1.bind\n\
         \x20 <- format(0)\n  <- format(1)\n  <- format(875713089)\n\
         zwp_linux_dmabuf_v1#1 = wl_registry#1.bind\n\
         \x20 <- modifier(<the renderer's formats>)\n"
    );
    let capabilities = "[1.000][rs] <- wl_registry@2.global, (2, Some(\"xdg_wm_base\"), 7)\n";
    assert!(
        normalise(capabilities, Clock::Real).is_err(),
        "no registry made"
    );
    assert_eq!(
        sorted_values(&[4, 0, 0, 0, 1, 0, 0, 0]),
        Ok(vec![1, 0, 0, 0, 4, 0, 0, 0])
    );
    assert!(sorted_values(&[4, 0, 0]).is_err());
}

/// Each client marked has objects of its own: the same id in two clients is two objects.
#[test]
fn each_client_has_its_own_objects() {
    let raw = "yuna-trace: client a\n\
               [1.000][rs] -> wl_display@1.get_registry(wl_registry@2)\n\
               yuna-trace: client b\n\
               [1.000][rs] -> wl_display@1.get_registry(wl_registry@2)\n\
               [1.000][rs] <- wl_registry@2.global, (9, Some(\"wl_shm\"), 2)\n\
               yuna-trace: client a\n\
               [1.000][rs] <- wl_registry@2.global_remove, (9)\n";
    assert_eq!(
        normalised(raw, Clock::Real),
        "client a\nwl_display#1\n  -> get_registry(new wl_registry#1)\n\
         wl_registry#1 = wl_display#1.get_registry\n  <- global_remove(name of wl_shm)\n\
         client b\nwl_display#1\n  -> get_registry(new wl_registry#1)\n\
         wl_registry#1 = wl_display#1.get_registry\n  <- global(name of wl_shm, \"wl_shm\", 2)\n"
    );
}

/// A request on a dead object keeps its mark.
#[test]
fn a_discarded_request_says_so() {
    let raw = format!("{BOUND}[1.000][rs][discarded] -> wl_surface@4.commit()\n");
    assert_eq!(
        normalised(&raw, Clock::Real),
        bound("  -> (discarded) commit()\n")
    );
}

/// A line that is not a message, Mesa's or the test harness's, is ignored; one that starts as a
/// message does but cannot be read is refused, as is a message of an object never made or of
/// another interface than its object's.
#[test]
fn a_line_that_is_not_a_message_is_ignored_and_a_broken_message_refused() {
    let raw = "MESA: warning: something\n\
               running 1 test\n\
               [1.000][rs] -> wl_display@1.get_registry(wl_registry@2)\n";
    assert_eq!(
        normalised(raw, Clock::Manual),
        "wl_display#1\n  -> get_registry(new wl_registry#1)\nwl_registry#1 = wl_display#1.get_registry\n"
    );
    for broken in [
        "[1.000][rs] -> wl_display@1.sync(wl_callback@3",
        "[1.000][rs] <- wl_callback@3.done(0)",
        "[1.000][rs] ?? wl_display@1.sync(wl_callback@3)",
        "[1.000][rs] -> wl_display@x.sync(wl_callback@3)",
        "[1.000][rs] -> wl_display@1.sync(wl_callback@3, 4)",
        "[1.000][rs] -> wl_display@1.sync(three)",
        "[1.000][rs] -> wl_display@1.no_such_request()",
        "[1.000][rs] -> zwp_tablet_seat_v2@4.destroy()",
        "[1.000][rs] -> wl_surface@9.commit()",
        "[1.000][rs] -> wl_surface@1.commit()",
    ] {
        assert!(normalise(broken, Clock::Real).is_err(), "{broken}");
    }
}

/// Each serial, time, mask, set and collapse of the tables names an argument or a message of the
/// type it is read as in the generated description, and every interface they name is one the
/// clients may speak.
#[test]
fn the_tables_name_arguments_of_known_messages() {
    let interfaces = known_interfaces();
    let message = |interface: &str, direction: Direction, name: &str| -> &'static MessageDesc {
        let interface = interfaces
            .get(interface)
            .unwrap_or_else(|| panic!("{interface} is not known"));
        let messages = match direction {
            Direction::Request => interface.requests,
            Direction::Event => interface.events,
        };
        messages
            .iter()
            .find(|message| message.name == name)
            .unwrap_or_else(|| panic!("{} has no {name}", interface.name))
    };
    let is_uint = |kind: Option<&ArgumentType>| matches!(kind, Some(ArgumentType::Uint));
    for &(direction, interface, name, index, _) in SERIALS {
        assert!(is_uint(
            message(interface, direction, name).signature.get(index)
        ));
    }
    for &(interface, name, at, _) in TIMES {
        for index in at.positions() {
            let signature = message(interface, Direction::Event, name).signature;
            assert!(is_uint(signature.get(index)), "{interface}.{name}");
        }
    }
    for &(direction, interface, name, index, _, _) in MASKED {
        assert!(
            message(interface, direction, name)
                .signature
                .get(index)
                .is_some()
        );
    }
    for &(interface, name, index, _) in SET_ARRAYS {
        let signature = message(interface, Direction::Event, name).signature;
        assert!(matches!(signature.get(index), Some(ArgumentType::Array)));
    }
    for &(direction, interface, name, _) in SET_MESSAGES {
        message(interface, direction, name);
    }
    for &(direction, interface, name, _) in COLLAPSED {
        message(interface, direction, name);
    }
    for &(direction, name, index) in GLOBAL_NAMES {
        let signature = message("wl_registry", direction, name).signature;
        assert!(is_uint(signature.get(index)));
    }
}
