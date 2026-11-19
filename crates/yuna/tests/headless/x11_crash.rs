//! What an Xwayland that crashes leaves on screen: nothing. Its menus drawn alone leave the output
//! whether Yuna was idle or busy when it died. A crash is counted as one even when the process's
//! reaping of every child, `yuna mcp`'s, takes its status first
//! ([ADR-0067](../../../../docs/adr/0067-one-xwayland-per-seat-on-demand-behind-a-cookie.md),
//! [ADR-0080](../../../../docs/adr/0080-what-survives-a-failure-and-yunas-own-panics.md)).

use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, WaitOptions, kill_process, waitpid};
use yuna::{CaptureSize, CaptureTarget, FIRST_PAUSE};

use crate::x11_client::{Image, WindowSpec, step};
use crate::x11_windows::{X11Seat, capture_until, open_drawn, pump, wait};

/// A tooltip drawn alone, by a program with no managed window, then Xwayland killed: once
/// Xwayland is reaped, the output must no longer show the tooltip.
#[test]
fn a_lone_menu_of_a_crashed_xwayland_leaves_the_output() {
    let mut seat = X11Seat::at_both("crash-menu");
    let output = seat.outputs[0];
    let client = seat.client();
    let spec = WindowSpec {
        window_type: Some(client.atoms._NET_WM_WINDOW_TYPE_TOOLTIP),
        ..WindowSpec::menu("tooltip", 300, 150, 150, 60)
    };
    let (client, (tooltip, image)) = step(&mut seat.compositor, client, move |client| {
        let window = client.open(&spec)?;
        let image = Image::menu(150, 60);
        client.draw(window, &image)?;
        Ok((window, image))
    });
    wait(&mut seat.compositor, "the tooltip is drawn", |compositor| {
        compositor
            .drawn_x11_menus(output)
            .into_iter()
            .find(|drawn| drawn.window == tooltip)
    });
    let region = (300, 150, 150, 60);
    let whole = CaptureTarget::Output {
        output: Some(output),
        region: None,
    };
    let shows_tooltip = |frame: &yuna::CapturedFrame| {
        Image::crop(&frame.image(), frame.mapping.width, region) == image.pixels.concat()
    };
    let before = capture_until(
        &mut seat.compositor,
        seat.seat,
        whole,
        CaptureSize::Physical,
        shows_tooltip,
    );
    assert!(
        shows_tooltip(&before),
        "the tooltip is drawn before the crash"
    );

    let pid = seat
        .compositor
        .x11_display(seat.seat)
        .and_then(|display| display.server)
        .expect("Xwayland runs");
    kill_process(Pid::from_raw(pid).expect("a pid"), Signal::KILL).expect("Xwayland is killed");
    drop(client);
    let seat_id = seat.seat;
    wait(&mut seat.compositor, "Xwayland is reaped", |compositor| {
        compositor
            .x11_display(seat_id)
            .is_some_and(|display| display.server.is_none())
            .then_some(())
    });
    let quiet = Instant::now() + Duration::from_millis(500);
    while Instant::now() < quiet {
        pump(&mut seat.compositor);
    }
    let told = seat.compositor.drawn_x11_menus(output);
    let ticket = seat
        .compositor
        .capture(seat.seat, whole, CaptureSize::Physical)
        .expect("a capture");
    let after = wait(&mut seat.compositor, "a capture", |compositor| {
        compositor
            .take_captures()
            .into_iter()
            .find(|(taken, _)| *taken == ticket)
            .map(|(_, frame)| frame)
    })
    .expect("a frame captured");
    assert!(told.is_empty(), "Yuna's state still holds the tooltip");
    assert!(
        !shows_tooltip(&after),
        "the output still shows the tooltip of an Xwayland that is gone"
    );
}

/// Whether process `pid` is a zombie: ended, its exit not yet reaped.
fn is_zombie(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            let after_name = stat.rsplit_once(')')?.1.trim_start().to_owned();
            after_name.chars().next()
        })
        == Some('Z')
}

/// The same, but Xwayland dies while Yuna's loop is busy elsewhere: Yuna dispatches again only
/// once Xwayland is a zombie, so that its connection's end and its pidfd come in one round, as
/// they do when Xwayland dies during a long frame. The output must no longer show the tooltip.
#[test]
fn a_lone_menu_of_xwayland_crashed_while_yuna_was_busy_leaves_the_output() {
    let mut seat = X11Seat::at_both("crash-menu-busy");
    let output = seat.outputs[0];
    let client = seat.client();
    let spec = WindowSpec {
        window_type: Some(client.atoms._NET_WM_WINDOW_TYPE_TOOLTIP),
        ..WindowSpec::menu("tooltip", 300, 150, 150, 60)
    };
    let (client, (tooltip, image)) = step(&mut seat.compositor, client, move |client| {
        let window = client.open(&spec)?;
        let image = Image::menu(150, 60);
        client.draw(window, &image)?;
        Ok((window, image))
    });
    wait(&mut seat.compositor, "the tooltip is drawn", |compositor| {
        compositor
            .drawn_x11_menus(output)
            .into_iter()
            .find(|drawn| drawn.window == tooltip)
    });
    let region = (300, 150, 150, 60);
    let whole = CaptureTarget::Output {
        output: Some(output),
        region: None,
    };
    let shows_tooltip = |frame: &yuna::CapturedFrame| {
        Image::crop(&frame.image(), frame.mapping.width, region) == image.pixels.concat()
    };
    let before = capture_until(
        &mut seat.compositor,
        seat.seat,
        whole,
        CaptureSize::Physical,
        shows_tooltip,
    );
    assert!(
        shows_tooltip(&before),
        "the tooltip is drawn before the crash"
    );
    // Let the compositor go quiet, so that nothing else redraws the output.
    let quiet = Instant::now() + Duration::from_millis(300);
    while Instant::now() < quiet {
        pump(&mut seat.compositor);
    }

    let pid = seat
        .compositor
        .x11_display(seat.seat)
        .and_then(|display| display.server)
        .expect("Xwayland runs");
    kill_process(Pid::from_raw(pid).expect("a pid"), Signal::KILL).expect("Xwayland is killed");
    // Yuna is busy: it does not dispatch until Xwayland is a zombie.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !is_zombie(pid) {
        assert!(Instant::now() < deadline, "Xwayland never ended");
        std::thread::sleep(Duration::from_millis(1));
    }
    drop(client);
    let seat_id = seat.seat;
    wait(&mut seat.compositor, "Xwayland is reaped", |compositor| {
        compositor
            .x11_display(seat_id)
            .is_some_and(|display| display.server.is_none())
            .then_some(())
    });
    let quiet = Instant::now() + Duration::from_millis(500);
    while Instant::now() < quiet {
        pump(&mut seat.compositor);
    }
    let told = seat.compositor.drawn_x11_menus(output);
    let ticket = seat
        .compositor
        .capture(seat.seat, whole, CaptureSize::Physical)
        .expect("a capture");
    let after = wait(&mut seat.compositor, "a capture", |compositor| {
        compositor
            .take_captures()
            .into_iter()
            .find(|(taken, _)| *taken == ticket)
            .map(|(_, frame)| frame)
    })
    .expect("a frame captured");
    assert!(told.is_empty(), "Yuna's state still holds the tooltip");
    assert!(
        !shows_tooltip(&after),
        "the output still shows the tooltip of an Xwayland that is gone"
    );
}

/// Xwayland killed twice in a row once its manager has taken charge, each time reaped by the
/// test's process before Yuna's loop runs again, as `yuna mcp`'s reaping of every child may reap
/// it before Yuna's own wait on it, and its status handed to the compositor as `yuna mcp` hands
/// it (`Compositor::child_reaped`): each end counts as the crash it was, so that the second
/// pauses the display [`FIRST_PAUSE`] before Xwayland starts again, as outside `yuna mcp`.
#[test]
fn a_crash_whose_status_another_reaper_took_still_counts() {
    let mut seat = X11Seat::at_both("crash-reaped-first");
    let seat_id = seat.seat;
    for round in 0..2 {
        let client = seat.client();
        // Listed once Yuna has handled its manager's events, the first of which says it took
        // charge: the crash comes after `WM_S0`.
        let (client, ..) = open_drawn(
            &mut seat.compositor,
            client,
            &WindowSpec::named(&format!("window {round}")),
        );
        let pid = seat
            .compositor
            .x11_display(seat_id)
            .and_then(|display| display.server)
            .expect("Xwayland runs");
        let pid = Pid::from_raw(pid).expect("a pid");
        kill_process(pid, Signal::KILL).expect("Xwayland is killed");
        let (reaped, status) = waitpid(Some(pid), WaitOptions::empty())
            .expect("Xwayland is waited for")
            .expect("its status");
        assert_eq!(reaped, pid);
        assert_eq!(status.terminating_signal(), Some(Signal::KILL.as_raw()));
        seat.compositor.child_reaped(reaped, status);
        drop(client);
        wait(&mut seat.compositor, "Xwayland's end", |compositor| {
            compositor
                .x11_display(seat_id)
                .is_some_and(|display| display.server.is_none())
                .then_some(())
        });
    }
    let began = Instant::now();
    let _client = seat.client();
    let paused = began.elapsed();
    eprintln!(
        "the third Xwayland served its first client {} ms after the second crash",
        paused.as_millis()
    );
    assert!(
        paused >= FIRST_PAUSE - Duration::from_millis(100),
        "the second crash in a row did not pause: {paused:?}"
    );
}
