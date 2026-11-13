//! Captures through MCP ([ADR-0034](../../../../docs/adr/0034-captures.md),
//! [ADR-0038](../../../../docs/adr/0038-captures-coordinates-windows-and-wayland.md)): images
//! of a seat's output, a region and a window, at the logical and the physical size, decoded
//! from the PNG the reply carries, and `pointer` aimed through a capture's pixels.
//!
//! A seat here has one output of 960 × 540 pixels at scale 1.5: 640 × 360 logical units, so that
//! a window's column is (640 − 3 × 16) / 2 = 296 units wide and 360 − 2 × 16 = 328 tall, at
//! (16, 16).

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use yuna::mcp::RECENT_CAPTURES;
use yuna::yuna_layout::SeatId;
use yuna::{ClockMode, OutputMode};

use crate::client::Client;
use crate::mcp::{Mcp, modern};
use crate::{BLUE, LEAD, RED, run_armed_deadlines};

/// The background where no window is, as `read_pixels` gives it: R, G, B.
fn background(mcp: &mut Mcp, output: u64) -> [u8; 3] {
    let pixel = mcp
        .server
        .compositor()
        .read_pixels(
            yuna::yuna_layout::OutputId(output),
            smithay::utils::Rectangle::new((0, 0).into(), (1, 1).into()),
        )
        .expect("a pixel");
    [pixel[0], pixel[1], pixel[2]]
}

/// Runs the refresh armed, then moves the clock to the boundary its frame is presented at: a
/// capture is answered once its frame's boundary has come.
fn settle(mcp: &mut Mcp) {
    run_armed_deadlines(mcp.server.compositor());
    mcp.server
        .compositor()
        .advance(LEAD)
        .expect("the clock moves");
}

/// A manual-clock server with a seat of the output above, and a window of `colour` on it,
/// drawn; returns the seat, the window's client and its window's id.
fn seat_with_a_window(colour: u32) -> (Mcp, u64, Client, u64) {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let created = mcp.call(
        "seat",
        json!({"action": "create", "width": 960, "height": 540, "scale": 1.5}),
    );
    let seat = created["structuredContent"]["seat"]
        .as_u64()
        .expect("a handle");
    let mut client = Client::connect_to(mcp.server.compositor(), SeatId(seat));
    let window = client.create_window();
    client.commit(window);
    client.roundtrip(mcp.server.compositor());
    client.ack(window);
    let configure = client
        .window(window)
        .configures
        .last()
        .expect("a configure")
        .clone();
    assert_eq!((configure.width, configure.height), (296, 328));
    client.attach(window, configure.width, configure.height, colour);
    client.commit(window);
    client.roundtrip(mcp.server.compositor());
    settle(&mut mcp);
    let windows = mcp.call("windows", json!({"seat": seat}));
    let id = windows["structuredContent"]["windows"][0]["id"]
        .as_u64()
        .expect("a window");
    (mcp, seat, client, id)
}

/// Sends `capture` with `arguments` as request `id` and waits, up to 10 s of real time, for its
/// reply, without moving the clock.
fn capture_reply(mcp: &mut Mcp, id: u64, arguments: Value) -> Value {
    mcp.send(&json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
                     "params": modern(json!({"name": "capture", "arguments": arguments}))}));
    wait_for(mcp, id, Duration::from_secs(10)).unwrap_or_else(|| panic!("no reply to capture {id}"))
}

/// The reply to request `id`, waited for up to `time` of real time; none if it did not come.
fn wait_for(mcp: &mut Mcp, id: u64, time: Duration) -> Option<Value> {
    let until = Instant::now() + time;
    while Instant::now() < until {
        if let Some(reply) = mcp.replies().into_iter().find(|r| r["id"] == json!(id)) {
            return Some(reply["result"].clone());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    None
}

/// The image a capture's result carries, decoded: its width, height and R, G, B pixels.
fn image_of(result: &Value) -> (u32, u32, Vec<u8>) {
    assert_eq!(result["isError"], json!(false), "{result}");
    let content = &result["content"][1];
    assert_eq!(content["type"], json!("image"));
    assert_eq!(content["mimeType"], json!("image/png"));
    let png = decode_base64(content["data"].as_str().expect("base64"));
    assert_eq!(result["structuredContent"]["png_bytes"], json!(png.len()));
    let mut reader = png::Decoder::new(std::io::Cursor::new(png))
        .read_info()
        .expect("a PNG");
    let mut pixels = vec![0; reader.output_buffer_size().expect("a size")];
    let info = reader.next_frame(&mut pixels).expect("its pixels");
    assert_eq!(info.color_type, png::ColorType::Rgb);
    pixels.truncate(info.buffer_size());
    (info.width, info.height, pixels)
}

/// Pixel (`x`, `y`) of an image `width` wide: R, G, B.
fn at(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * width + x) * 3) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2]]
}

fn decode_base64(text: &str) -> Vec<u8> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => panic!("not base64: {c}"),
    };
    let mut bytes = Vec::new();
    for chunk in text.as_bytes().chunks(4) {
        let digits: Vec<u8> = chunk
            .iter()
            .take_while(|&&c| c != b'=')
            .map(|&c| value(c))
            .collect();
        let n = digits.iter().fold(0u32, |n, &d| n << 6 | u32::from(d)) << (6 * (4 - digits.len()));
        bytes.extend_from_slice(&n.to_be_bytes()[1..digits.len()]);
    }
    bytes
}

const RED_RGB: [u8; 3] = [255, 0, 0];
const BLUE_RGB: [u8; 3] = [0, 0, 255];

/// A whole output's capture is its logical image: a pixel per logical unit, the window's edges
/// on whole units, the background around it; its mapping is the identity, and it says when its
/// frame is presented.
#[test]
fn an_outputs_capture_is_its_logical_image() {
    let (mut mcp, seat, _client, _) = seat_with_a_window(RED);
    let background = background(&mut mcp, 1);
    let result = capture_reply(&mut mcp, 900, json!({"seat": seat}));
    let (width, height, pixels) = image_of(&result);
    assert_eq!((width, height), (640, 360));
    let structured = &result["structuredContent"];
    assert_eq!(structured["capture"], json!(1));
    assert_eq!(structured["output"], json!(1));
    assert_eq!(structured["window"], Value::Null);
    assert_eq!(
        (structured["width"].clone(), structured["height"].clone()),
        (json!(640), json!(360))
    );
    assert_eq!(
        (structured["x"].clone(), structured["y"].clone()),
        (json!(0.0), json!(0.0))
    );
    assert_eq!(structured["units_per_pixel"], json!(1.0));
    let presented = structured["presented_ns"].as_u64().expect("a time");
    assert!(presented <= mcp.server.compositor().now().as_nanos() as u64);
    // The window spans units 16 to 311, and 16 to 343: its edges on whole pixels.
    for (x, y, colour) in [
        (15, 100, background),
        (16, 100, RED_RGB),
        (311, 100, RED_RGB),
        (312, 100, background),
        (100, 15, background),
        (100, 16, RED_RGB),
        (100, 343, RED_RGB),
        (100, 344, background),
        (639, 359, background),
    ] {
        assert_eq!(at(&pixels, width, x, y), colour, "({x}, {y})");
    }
}

/// A capture asked while a frame is on its way waits for that frame, and is answered at the
/// boundary the frame is presented at, not before: on the manual clock, once the clock reaches it.
#[test]
fn a_capture_shows_the_frame_on_its_way_and_comes_at_its_boundary() {
    let (mut mcp, seat, mut client, _) = seat_with_a_window(RED);
    // A frame just drawn: the next refresh waits for a boundary, a lead before it.
    let configure = client
        .window(0)
        .configures
        .last()
        .expect("a configure")
        .clone();
    client.attach(0, configure.width, configure.height, BLUE);
    client.commit(0);
    client.roundtrip(mcp.server.compositor());
    let refresh = mcp
        .server
        .compositor()
        .next_deadline()
        .expect("a refresh armed");
    assert!(
        refresh > mcp.server.compositor().now(),
        "the output is not idle"
    );
    mcp.send(&json!({"jsonrpc": "2.0", "id": 901, "method": "tools/call",
                     "params": modern(json!({"name": "capture", "arguments": {"seat": seat}}))}));
    assert_eq!(wait_for(&mut mcp, 901, Duration::from_millis(100)), None);
    // The refresh draws the blue frame, which is presented a lead later.
    let now = mcp.server.compositor().now();
    mcp.server
        .compositor()
        .advance(refresh - now)
        .expect("the clock moves");
    assert_eq!(
        wait_for(&mut mcp, 901, Duration::from_millis(300)),
        None,
        "answered before its boundary"
    );
    mcp.server
        .compositor()
        .advance(LEAD)
        .expect("the clock moves");
    let result =
        wait_for(&mut mcp, 901, Duration::from_secs(10)).expect("answered at its boundary");
    let (width, _, pixels) = image_of(&result);
    assert_eq!(at(&pixels, width, 100, 100), BLUE_RGB);
    assert_eq!(
        result["structuredContent"]["presented_ns"].as_u64(),
        Some((refresh + LEAD).as_nanos() as u64)
    );
}

/// A window's capture is its rectangle on its output; `pointer` aims through a capture's pixels,
/// at the logical and at the physical size, and through a region's.
#[test]
fn pointer_aims_through_a_windows_a_regions_and_a_physical_capture() {
    let (mut mcp, seat, _client, window) = seat_with_a_window(RED);
    let background = background(&mut mcp, 1);
    let logical = capture_reply(&mut mcp, 910, json!({"seat": seat, "window": window}));
    let (width, height, pixels) = image_of(&logical);
    assert_eq!((width, height), (296, 328));
    let structured = &logical["structuredContent"];
    assert_eq!(structured["window"], json!(window));
    assert_eq!(
        (structured["x"].clone(), structured["y"].clone()),
        (json!(16.0), json!(16.0))
    );
    assert!(pixels.as_chunks::<3>().0.iter().all(|p| *p == RED_RGB));
    let id = structured["capture"].as_u64().expect("an id");
    let moved = mcp.call(
        "pointer",
        json!({"action": "move", "seat": seat, "capture": id, "x": 10, "y": 20}),
    );
    let pointer = &moved["structuredContent"]["pointer"];
    assert_eq!(
        (pointer["x"].clone(), pointer["y"].clone()),
        (json!(26.0), json!(36.0))
    );
    assert_eq!(pointer["window"], json!(window));
    assert_eq!(pointer["surface_position"], json!({"x": 10.0, "y": 20.0}));

    let physical = capture_reply(
        &mut mcp,
        911,
        json!({"seat": seat, "window": window, "size": "physical"}),
    );
    let (width, height, _) = image_of(&physical);
    assert_eq!((width, height), (444, 492));
    let structured = &physical["structuredContent"];
    assert_eq!(
        (structured["x"].clone(), structured["y"].clone()),
        (json!(16.0), json!(16.0))
    );
    assert!((structured["units_per_pixel"].as_f64().expect("a number") - 1.0 / 1.5).abs() < 1e-12);
    let id = structured["capture"].as_u64().expect("an id");
    let clicked = mcp.call(
        "pointer",
        json!({"action": "click", "seat": seat, "capture": id, "x": 15, "y": 30}),
    );
    let pointer = &clicked["structuredContent"]["pointer"];
    assert!(
        (pointer["x"].as_f64().expect("x") - 26.0).abs() < 1e-9,
        "{pointer}"
    );
    assert!(
        (pointer["y"].as_f64().expect("y") - 36.0).abs() < 1e-9,
        "{pointer}"
    );

    let region = capture_reply(
        &mut mcp,
        912,
        json!({"seat": seat, "region": {"x": 10, "y": 10, "width": 20, "height": 10}}),
    );
    let (width, height, pixels) = image_of(&region);
    assert_eq!((width, height), (20, 10));
    assert_eq!(at(&pixels, width, 5, 5), background);
    assert_eq!(at(&pixels, width, 6, 6), RED_RGB);
    let id = region["structuredContent"]["capture"]
        .as_u64()
        .expect("an id");

    // A point off the image, a capture and an output together, and an unknown capture fail.
    for arguments in [
        json!({"action": "move", "seat": seat, "capture": id, "x": 21, "y": 0}),
        json!({"action": "move", "seat": seat, "capture": id, "x": -1, "y": 0}),
        json!({"action": "move", "seat": seat, "capture": id, "output": 1, "x": 1, "y": 1}),
        json!({"action": "move", "seat": seat, "capture": 99, "x": 1, "y": 1}),
        json!({"action": "press", "seat": seat, "capture": id}),
    ] {
        let refused = mcp.call("pointer", arguments.clone());
        assert_eq!(refused["isError"], json!(true), "{arguments}");
    }
}

/// A capture refused: its reason.
fn refused(mcp: &mut Mcp, arguments: Value) -> String {
    let result = mcp.call("capture", arguments.clone());
    assert_eq!(result["isError"], json!(true), "{arguments}");
    result["content"][0]["text"]
        .as_str()
        .expect("a reason")
        .to_owned()
}

/// Another seat's window and output are unknown to a seat's capture: on the real clock, since a
/// manual-clock Yuna holds one seat.
#[test]
fn another_seats_window_and_output_are_unknown() {
    let mut mcp = Mcp::new(ClockMode::Real);
    let seat = mcp.seat();
    let other = mcp.seat();
    let mut client = Client::connect_to(mcp.server.compositor(), SeatId(seat));
    let window = client.create_window();
    client.commit(window);
    client.roundtrip(mcp.server.compositor());
    client.ack(window);
    let configure = client
        .window(window)
        .configures
        .last()
        .expect("a configure")
        .clone();
    client.attach(window, configure.width, configure.height, RED);
    client.commit(window);
    client.roundtrip(mcp.server.compositor());
    let windows = mcp.call("windows", json!({"seat": seat, "wait_ms": 5_000}));
    let id = windows["structuredContent"]["windows"][0]["id"]
        .as_u64()
        .expect("a window");
    assert!(refused(&mut mcp, json!({"seat": other, "window": id})).contains("no window"));
    let output = refused(&mut mcp, json!({"seat": other, "output": 1}));
    assert!(output.contains("no output 1"), "{output}");
    let own = capture_reply(&mut mcp, 925, json!({"seat": seat, "window": id}));
    assert_eq!(own["isError"], json!(false), "{own}");
}

/// What a capture cannot show is refused at once, with what to do: a hidden window, a region off
/// the output, a window with a region, and a second capture of a seat while one is in flight; a
/// capture cancelled is never answered, and frees its seat for the next.
#[test]
fn captures_that_cannot_be_are_refused_and_one_of_a_seat_is_in_flight() {
    let (mut mcp, seat, mut client, window) = seat_with_a_window(RED);
    let off = refused(
        &mut mcp,
        json!({"seat": seat, "region": {"x": 600, "y": 0, "width": 41, "height": 1}}),
    );
    assert!(off.contains("640 × 360"), "{off}");
    refused(
        &mut mcp,
        json!({"seat": seat, "window": window, "region": {"x": 0, "y": 0, "width": 1, "height": 1}}),
    );
    refused(&mut mcp, json!({"seat": seat, "size": "huge"}));
    refused(&mut mcp, json!({"seat": 99}));

    // A capture waits for a frame on its way: a second one of the seat is refused meanwhile.
    let configure = client
        .window(0)
        .configures
        .last()
        .expect("a configure")
        .clone();
    client.attach(0, configure.width, configure.height, BLUE);
    client.commit(0);
    client.roundtrip(mcp.server.compositor());
    mcp.send(&json!({"jsonrpc": "2.0", "id": 920, "method": "tools/call",
                     "params": modern(json!({"name": "capture", "arguments": {"seat": seat}}))}));
    assert_eq!(wait_for(&mut mcp, 920, Duration::from_millis(20)), None);
    assert!(refused(&mut mcp, json!({"seat": seat})).contains("in flight"));
    mcp.send(
        &json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
                     "params": {"requestId": 920, "reason": "the test"}}),
    );
    settle(&mut mcp);
    assert_eq!(wait_for(&mut mcp, 920, Duration::from_millis(200)), None);
    let next = capture_reply(&mut mcp, 921, json!({"seat": seat}));
    let (width, _, pixels) = image_of(&next);
    assert_eq!(at(&pixels, width, 100, 100), BLUE_RGB);

    // Hidden on a workspace not shown, a window is refused with what shows it.
    for action in ["create", "focus"] {
        let shown = mcp.call(
            "workspaces",
            json!({"action": action, "seat": seat, "name": "empty"}),
        );
        assert_eq!(shown["isError"], json!(false), "{shown}");
    }
    settle(&mut mcp);
    let hidden = refused(&mut mcp, json!({"seat": seat, "window": window}));
    assert!(
        hidden.contains("hidden") && hidden.contains("workspaces focus"),
        "{hidden}"
    );
}

/// A seat closed with a capture in flight answers it plainly; its captures are forgotten.
#[test]
fn a_seat_closed_fails_its_capture_in_flight() {
    let (mut mcp, seat, mut client, _) = seat_with_a_window(RED);
    let first = capture_reply(&mut mcp, 930, json!({"seat": seat}));
    let id = first["structuredContent"]["capture"]
        .as_u64()
        .expect("an id");
    let configure = client
        .window(0)
        .configures
        .last()
        .expect("a configure")
        .clone();
    client.attach(0, configure.width, configure.height, BLUE);
    client.commit(0);
    client.roundtrip(mcp.server.compositor());
    mcp.send(&json!({"jsonrpc": "2.0", "id": 931, "method": "tools/call",
                     "params": modern(json!({"name": "capture", "arguments": {"seat": seat}}))}));
    assert_eq!(wait_for(&mut mcp, 931, Duration::from_millis(20)), None);
    // Both replies come in the round that closes the seat.
    mcp.send(&json!({"jsonrpc": "2.0", "id": 932, "method": "tools/call",
                     "params": modern(json!({"name": "seat",
                                             "arguments": {"action": "close", "seat": seat}}))}));
    let replies = mcp.replies();
    let reply = |id: u64| {
        replies
            .iter()
            .find(|r| r["id"] == json!(id))
            .map(|r| r["result"].clone())
    };
    assert_eq!(reply(932).expect("closed")["isError"], json!(false));
    let failed = reply(931).expect("answered");
    assert_eq!(failed["isError"], json!(true));
    assert!(
        failed["content"][0]["text"]
            .as_str()
            .expect("a reason")
            .contains("closed")
    );
    let stale = mcp.call(
        "pointer",
        json!({"action": "move", "seat": seat, "capture": id, "x": 1, "y": 1}),
    );
    assert_eq!(stale["isError"], json!(true));
}

/// `pointer` takes a seat's last 16 captures; an older one is refused, with what to do.
#[test]
fn pointer_takes_a_seats_last_16_captures() {
    let (mut mcp, seat, _client, _) = seat_with_a_window(RED);
    let mut ids = Vec::new();
    for request in 0..=RECENT_CAPTURES as u64 {
        let result = capture_reply(
            &mut mcp,
            940 + request,
            json!({"seat": seat, "region": {"x": 0, "y": 0, "width": 8, "height": 8}}),
        );
        ids.push(
            result["structuredContent"]["capture"]
                .as_u64()
                .expect("an id"),
        );
    }
    assert_eq!(ids, (1..=RECENT_CAPTURES as u64 + 1).collect::<Vec<u64>>());
    let oldest = mcp.call(
        "pointer",
        json!({"action": "move", "seat": seat, "capture": ids[0], "x": 1, "y": 1}),
    );
    assert_eq!(oldest["isError"], json!(true));
    assert!(
        oldest["content"][0]["text"]
            .as_str()
            .expect("a reason")
            .contains("capture again")
    );
    let kept = mcp.call(
        "pointer",
        json!({"action": "move", "seat": seat, "capture": ids[1], "x": 1, "y": 1}),
    );
    assert_eq!(kept["isError"], json!(false), "{kept}");
}

/// An image past what Claude's API takes is refused, with what to do: over 8,000 pixels a side,
/// or over 5 MB of base64, as a window of noise at the physical size is.
#[test]
fn an_image_too_large_for_a_model_is_refused() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let wide = mcp.call(
        "seat",
        json!({"action": "create", "width": 8192, "height": 64, "scale": 1}),
    );
    let wide = wide["structuredContent"]["seat"]
        .as_u64()
        .expect("a handle");
    // A new output is not idle: its first frame is drawn at its first boundary.
    settle(&mut mcp);
    let result = capture_reply(&mut mcp, 950, json!({"seat": wide, "size": "physical"}));
    assert_eq!(result["isError"], json!(true));
    assert!(
        result["content"][0]["text"]
            .as_str()
            .expect("a reason")
            .contains("8000")
    );
    mcp.call("seat", json!({"action": "close", "seat": wide}));

    let created = mcp.call(
        "seat",
        json!({"action": "create", "width": 2048, "height": 2048, "scale": 1}),
    );
    let seat = created["structuredContent"]["seat"]
        .as_u64()
        .expect("a handle");
    let mut client = Client::connect_to(mcp.server.compositor(), SeatId(seat));
    let window = client.create_window();
    client.commit(window);
    client.roundtrip(mcp.server.compositor());
    client.ack(window);
    let configure = client
        .window(window)
        .configures
        .last()
        .expect("a configure")
        .clone();
    let mut noise = 0x1234_5678u32;
    let pixels: Vec<u32> = (0..configure.width * configure.height)
        .map(|_| {
            noise = noise.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            0xFF00_0000 | noise >> 8
        })
        .collect();
    client.attach_pixels(window, configure.width, configure.height, &pixels);
    client.commit(window);
    client.roundtrip(mcp.server.compositor());
    settle(&mut mcp);
    let windows = mcp.call("windows", json!({"seat": seat}));
    let id = windows["structuredContent"]["windows"][0]["id"]
        .as_u64()
        .expect("a window");
    let result = capture_reply(&mut mcp, 951, json!({"seat": seat, "window": id}));
    assert_eq!(
        result["isError"],
        json!(true),
        "{}",
        result["structuredContent"]
    );
    let reason = result["content"][0]["text"].as_str().expect("a reason");
    assert!(reason.contains("5000000"), "{reason}");
}

/// An output whose size does not divide by its scale is captured with its last, partial unit.
#[test]
fn an_output_that_does_not_divide_by_its_scale_keeps_its_last_unit() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let mode = OutputMode::new(1366, 768, 60.0, 1.5).expect("a mode");
    let created = mcp.call(
        "seat",
        json!({"action": "create", "width": mode.width(), "height": mode.height(), "rate": 60, "scale": 1.5}),
    );
    let seat = created["structuredContent"]["seat"]
        .as_u64()
        .expect("a handle");
    settle(&mut mcp);
    let result = capture_reply(&mut mcp, 960, json!({"seat": seat}));
    let (width, height, _) = image_of(&result);
    assert_eq!((width, height), (911, 512));
}

/// A window whose workspace is shown by an arrangement still to come is captured at that
/// arrangement's refresh, as it will be drawn, not refused as hidden.
#[test]
fn a_window_shown_by_an_arrangement_to_come_is_captured_there() {
    let (mut mcp, seat, _client, window) = seat_with_a_window(RED);
    for action in ["create", "focus"] {
        let shown = mcp.call(
            "workspaces",
            json!({"action": action, "seat": seat, "name": "empty"}),
        );
        assert_eq!(shown["isError"], json!(false), "{shown}");
    }
    settle(&mut mcp);
    let moved = mcp.call(
        "workspaces",
        json!({"action": "move_window", "seat": seat, "window": window, "workspace": "empty"}),
    );
    assert_eq!(moved["isError"], json!(false), "{moved}");
    mcp.send(&json!({"jsonrpc": "2.0", "id": 990, "method": "tools/call",
                     "params": modern(json!({"name": "capture",
                                             "arguments": {"seat": seat, "window": window}}))}));
    assert_eq!(wait_for(&mut mcp, 990, Duration::from_millis(20)), None);
    settle(&mut mcp);
    let result = wait_for(&mut mcp, 990, Duration::from_secs(10)).expect("answered");
    let (width, height, pixels) = image_of(&result);
    assert_eq!((width, height), (296, 328));
    assert_eq!(at(&pixels, width, 100, 100), RED_RGB);

    // A window hidden by the arrangement to come is refused once it is arranged.
    for action in ["create", "focus"] {
        mcp.call(
            "workspaces",
            json!({"action": action, "seat": seat, "name": "other"}),
        );
    }
    mcp.send(&json!({"jsonrpc": "2.0", "id": 991, "method": "tools/call",
                     "params": modern(json!({"name": "capture",
                                             "arguments": {"seat": seat, "window": window}}))}));
    settle(&mut mcp);
    let hidden = wait_for(&mut mcp, 991, Duration::from_secs(10)).expect("answered");
    assert_eq!(hidden["isError"], json!(true));
    let reason = hidden["content"][0]["text"].as_str().expect("a reason");
    assert!(reason.contains("hidden"), "{reason}");
}

/// A capture's image spans [0, width) × [0, height): the point just past its last pixel is
/// off it, as a point just past an output's edge is.
#[test]
fn the_point_past_a_captures_last_pixel_is_off_it() {
    let (mut mcp, seat, _client, window) = seat_with_a_window(RED);
    let logical = capture_reply(&mut mcp, 980, json!({"seat": seat, "window": window}));
    let id = logical["structuredContent"]["capture"]
        .as_u64()
        .expect("an id");
    for (x, y, refused) in [(295.5, 0.0, false), (296.0, 0.0, true), (0.0, 328.0, true)] {
        let moved = mcp.call(
            "pointer",
            json!({"action": "move", "seat": seat, "capture": id, "x": x, "y": y}),
        );
        assert_eq!(moved["isError"], json!(refused), "({x}, {y}): {moved}");
    }
}

/// An agent's capture waiting on an output that is removed fails, saying so.
#[test]
fn a_capture_waiting_on_a_removed_output_fails() {
    let (mut mcp, seat, _client, _) = seat_with_a_window(RED);
    let created = mcp.call(
        "outputs",
        json!({"action": "create", "seat": seat, "width": 640, "height": 360, "scale": 1}),
    );
    let output = created["structuredContent"]["output"]["id"]
        .as_u64()
        .expect("an output");
    // A new output's first frame waits for its first boundary.
    mcp.send(&json!({"jsonrpc": "2.0", "id": 995, "method": "tools/call",
                     "params": modern(json!({"name": "capture",
                                             "arguments": {"seat": seat, "output": output}}))}));
    assert_eq!(wait_for(&mut mcp, 995, Duration::from_millis(20)), None);
    mcp.send(&json!({"jsonrpc": "2.0", "id": 996, "method": "tools/call",
                     "params": modern(json!({"name": "outputs",
                                             "arguments": {"action": "remove", "seat": seat, "output": output}}))}));
    let replies = mcp.replies();
    let failed = replies
        .iter()
        .find(|r| r["id"] == json!(995))
        .map(|r| r["result"].clone())
        .expect("answered with the removal");
    assert_eq!(failed["isError"], json!(true));
    let reason = failed["content"][0]["text"].as_str().expect("a reason");
    assert!(reason.contains("removed"), "{reason}");
}
