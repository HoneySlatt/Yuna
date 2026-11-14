//! `color-management-v1` (ADR-0046, ADR-0047): what Yuna lists at each version, what an HDR and
//! an SDR output announce, the description a surface's window prefers and its changes, each
//! surface's description recorded at its commit, and every protocol error a client may earn.
//!
//! The compositor has two outputs on the owner's seat: 1, HDR, the owner's monitor's colours;
//! 2, SDR. Both are 1920 × 1080 at 60 Hz and scale 1.

use std::time::Duration;

use yuna::colour::SurfaceRole;
use yuna::colour::description::{NamedPrimaries, PG32UCDM, TransferFunction as Tf};
use yuna::yuna_layout::{OutputId, WindowId};
use yuna::{ClockMode, Compositor, Config, OWNER_SEAT, OutputMode};

use crate::colour_client::{
    ColourClient, Feature, Params, Primaries, RenderIntent, TransferFunction,
};
use crate::{RED, assert_rests, run_armed_deadlines};

const HDR_OUTPUT: OutputId = OutputId(1);
const SDR_OUTPUT: OutputId = OutputId(2);

fn compositor() -> Compositor {
    let mode = OutputMode::new(1920, 1080, 60.0, 1.0).expect("a mode");
    Compositor::new(Config {
        outputs: vec![mode.with_hdr(PG32UCDM), mode],
        clock: ClockMode::Manual,
        listen: false,
        ..Config::default()
    })
    .expect("the compositor starts")
}

/// Opens a window of `colour`'s client and maps it: on the seat's focused output, the HDR one.
fn mapped_window(compositor: &mut Compositor, colour: &mut ColourClient) -> usize {
    let client = &mut colour.client;
    let window = client.open_window(compositor);
    client.ack(window);
    client.attach(window, 400, 300, RED);
    client.commit(window);
    colour.roundtrip(compositor);
    run_armed_deadlines(compositor);
    colour.roundtrip(compositor);
    window
}

/// Sets PQ over BT.2020 on `params`, the least an HDR10 client sends.
fn pq_bt2020(params: &Params) {
    params.set_tf_named(TransferFunction::St2084Pq);
    params.set_primaries_named(Primaries::Bt2020);
}

#[test]
fn each_version_lists_kwins_list_and_only_what_it_knows() {
    let mut compositor = compositor();
    for version in 1..=3 {
        let colour = ColourClient::connect(&mut compositor, OWNER_SEAT, version);
        let listed = &colour.state.listed;
        assert!(listed.done);
        assert_eq!(listed.intents, [RenderIntent::Perceptual as u32]);
        assert_eq!(
            listed.features,
            [
                Feature::Parametric as u32,
                Feature::SetPrimaries as u32,
                Feature::SetLuminances as u32,
                Feature::SetMasteringDisplayPrimaries as u32,
            ]
        );
        let mut expected = vec![
            TransferFunction::Bt1886 as u32,
            TransferFunction::Gamma22 as u32,
            TransferFunction::St2084Pq as u32,
            TransferFunction::Hlg as u32,
            TransferFunction::ExtLinear as u32,
        ];
        // compound_power_2_4 exists from version 2; srgb is never listed (ADR-0046).
        if version >= 2 {
            expected.push(TransferFunction::CompoundPower24 as u32);
        }
        assert_eq!(listed.transfer_functions, expected, "version {version}");
        assert_eq!(listed.primaries, (1..=10).collect::<Vec<u32>>());
    }
}

#[test]
fn an_hdr_output_announces_the_monitors_volume_and_an_sdr_one_srgb() {
    let mut compositor = compositor();
    for version in [1, 3] {
        let mut colour = ColourClient::connect(&mut compositor, OWNER_SEAT, version);
        let hdr = colour.output_description(0);
        let sdr = colour.output_description(1);
        colour.roundtrip(&mut compositor);
        colour.information(hdr);
        colour.information(sdr);
        colour.roundtrip(&mut compositor);

        let events = colour.events(hdr);
        assert_eq!(events.ready2, version >= 2);
        let hdr_identity = events.ready.expect("ready");
        let info = events.information.clone().expect("information");
        assert!(info.done);
        assert_eq!(info.tf_named, Some(TransferFunction::St2084Pq as u32));
        assert_eq!(info.primaries_named, Some(Primaries::Bt2020 as u32));
        assert_eq!(
            info.primaries,
            Some(NamedPrimaries::Bt2020.chromaticities().values())
        );
        assert_eq!(info.luminances, Some((50, 10_000, 203)));
        // The test pins the monitor's volume, not the provisional transfer function (ADR-0047).
        assert_eq!(info.target_primaries, Some(PG32UCDM.primaries.values()));
        assert_eq!(info.target_luminance, Some((2, 1_015)));
        assert_eq!(info.target_max_cll, Some(1_015));
        assert_eq!(info.target_max_fall, Some(265));

        let events = colour.events(sdr);
        assert_ne!(events.ready, Some(hdr_identity));
        let info = events.information.clone().expect("information");
        assert_eq!(info.tf_named, Some(TransferFunction::Gamma22 as u32));
        assert_eq!(info.primaries_named, Some(Primaries::Srgb as u32));
        assert_eq!(info.luminances, Some((2_000, 80, 80)));
        assert_eq!(info.target_luminance, Some((2_000, 80)));
        assert_eq!(info.target_max_cll, None);

        // The same output's description again is the same record: the same identity.
        let again = colour.output_description(0);
        colour.roundtrip(&mut compositor);
        assert_eq!(colour.events(again).ready, Some(hdr_identity));
    }
    // The outputs' first frames, then rest: describing outputs wakes nothing.
    run_armed_deadlines(&mut compositor);
    assert_rests(&mut compositor);
}

#[test]
fn a_surfaces_description_is_recorded_at_its_commit_and_unset_with_its_object() {
    let mut compositor = compositor();
    let mut colour = ColourClient::connect(&mut compositor, OWNER_SEAT, 2);
    let window = mapped_window(&mut compositor, &mut colour);
    let surface = colour.client.window(window).surface.clone();

    let params = colour.params();
    pq_bt2020(&params);
    params.set_luminances(0, 10_000, 203);
    params.set_mastering_display_primaries(
        680_000, 320_000, 265_000, 690_000, 150_000, 60_000, 312_700, 329_000,
    );
    params.set_mastering_luminance(50, 1_000);
    params.set_max_cll(1_000);
    params.set_max_fall(400);
    let description = colour.create(params);
    colour.roundtrip(&mut compositor);
    assert!(colour.events(description).ready.is_some());

    let colour_surface = colour.colour_surface(&surface);
    colour_surface.set_image_description(colour.description(description), RenderIntent::Perceptual);
    // Copy semantics: the description may go before the commit.
    colour.description(description).destroy();
    colour.roundtrip(&mut compositor);
    assert_eq!(
        compositor.window_colours(WindowId(1)),
        [],
        "not before its commit"
    );
    colour.client.commit(window);
    colour.roundtrip(&mut compositor);
    let recorded = compositor.window_colours(WindowId(1));
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].role, SurfaceRole::Window);
    let declared = &recorded[0].description;
    assert_eq!(declared.transfer_function, Tf::St2084Pq);
    assert_eq!(declared.named_primaries, Some(NamedPrimaries::Bt2020));
    // PQ's maximum is its minimum plus 10,000 cd/m².
    assert_eq!(
        (
            declared.luminances.min,
            declared.luminances.max,
            declared.luminances.reference
        ),
        (0, 10_000, 203)
    );
    assert_eq!(
        declared.target_primaries,
        NamedPrimaries::DisplayP3.chromaticities()
    );
    assert_eq!(declared.target_luminance, (50, 1_000));
    assert_eq!(
        (declared.max_cll, declared.max_fall),
        (Some(1_000), Some(400))
    );

    // Destroying the colour surface unsets the description at the next commit; another may
    // then be made for the surface.
    colour_surface.destroy();
    colour.roundtrip(&mut compositor);
    assert_eq!(compositor.window_colours(WindowId(1)).len(), 1);
    colour.client.commit(window);
    colour.roundtrip(&mut compositor);
    assert_eq!(compositor.window_colours(WindowId(1)), []);
    let again = colour.colour_surface(&surface);
    again.unset_image_description();
    colour.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    assert_rests(&mut compositor);
}

/// A subsurface prefers its window's description: the HDR output's, where the window is.
#[test]
fn a_subsurface_prefers_its_windows_description() {
    let mut compositor = compositor();
    let mut colour = ColourClient::connect(&mut compositor, OWNER_SEAT, 3);
    let hdr = colour.output_description(0);
    let window = mapped_window(&mut compositor, &mut colour);
    let subsurface = colour.client.add_subsurface(window);
    let surface = colour.client.subsurface(subsurface).surface.clone();
    let feedback = colour.feedback(&surface);
    let preferred = colour.preferred(feedback);
    colour.roundtrip(&mut compositor);
    assert!(colour.events(hdr).ready.is_some());
    assert_eq!(colour.events(preferred).ready, colour.events(hdr).ready);
}

#[test]
fn a_subsurfaces_description_is_read_with_its_window() {
    let mut compositor = compositor();
    let mut colour = ColourClient::connect(&mut compositor, OWNER_SEAT, 3);
    let window = mapped_window(&mut compositor, &mut colour);
    let subsurface = colour.client.add_subsurface(window);
    let surface = colour.client.subsurface(subsurface).surface.clone();
    let params = colour.params();
    pq_bt2020(&params);
    let description = colour.create(params);
    colour.roundtrip(&mut compositor);
    colour
        .colour_surface(&surface)
        .set_image_description(colour.description(description), RenderIntent::Perceptual);
    surface.commit();
    colour.roundtrip(&mut compositor);
    let recorded = compositor.window_colours(WindowId(1));
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].role, SurfaceRole::Subsurface);
    assert_eq!(recorded[0].description.transfer_function, Tf::St2084Pq);
}

#[test]
fn a_windows_preferred_description_follows_its_workspaces_output() {
    let mut compositor = compositor();
    for version in [1, 2] {
        let mut colour = ColourClient::connect(&mut compositor, OWNER_SEAT, version);
        let hdr = colour.output_description(0);
        let sdr = colour.output_description(1);
        colour.roundtrip(&mut compositor);
        let (hdr, sdr) = (
            colour.events(hdr).ready.expect("ready"),
            colour.events(sdr).ready.expect("ready"),
        );

        // Before it maps, a window prefers its seat's focused output's description.
        let window = colour.client.open_window(&mut compositor);
        let surface = colour.client.window(window).surface.clone();
        let feedback = colour.feedback(&surface);
        let preferred = colour.preferred(feedback);
        colour.roundtrip(&mut compositor);
        assert_eq!(colour.events(preferred).ready, Some(hdr));
        colour.client.ack(window);
        colour.client.attach(window, 400, 300, RED);
        colour.client.commit(window);
        colour.roundtrip(&mut compositor);
        run_armed_deadlines(&mut compositor);
        colour.roundtrip(&mut compositor);
        assert_eq!(colour.preferred_changes(feedback), [] as [u64; 0]);

        // Moved to a workspace of the SDR output: told once, with that output's identity.
        let name = format!("sdr-{version}");
        compositor.change_layout(|layout| {
            layout
                .add_workspace(OWNER_SEAT, &name, Some(SDR_OUTPUT))
                .expect("a workspace");
            layout.move_to_workspace(OWNER_SEAT, &name).expect("moved");
        });
        run_armed_deadlines(&mut compositor);
        colour.roundtrip(&mut compositor);
        assert_eq!(colour.preferred_changes(feedback), [sdr]);
        let preferred = colour.preferred(feedback);
        colour.information(preferred);
        colour.roundtrip(&mut compositor);
        assert_eq!(colour.events(preferred).ready, Some(sdr));
        let info = colour
            .events(preferred)
            .information
            .clone()
            .expect("information");
        assert_eq!(info.tf_named, Some(TransferFunction::Gamma22 as u32));

        // Shown back on the HDR output: told again.
        compositor.change_layout(|layout| {
            layout
                .focus_output(OWNER_SEAT, HDR_OUTPUT)
                .expect("focused");
        });
        let back = format!("hdr-{version}");
        compositor.change_layout(|layout| {
            layout
                .focus_output(OWNER_SEAT, SDR_OUTPUT)
                .expect("focused");
            layout
                .add_workspace(OWNER_SEAT, &back, Some(HDR_OUTPUT))
                .expect("a workspace");
            layout.move_to_workspace(OWNER_SEAT, &back).expect("moved");
        });
        run_armed_deadlines(&mut compositor);
        colour.roundtrip(&mut compositor);
        assert_eq!(colour.preferred_changes(feedback), [sdr, hdr]);
        colour.client.close(window);
        colour.client.destroy_surface(window);
        colour.roundtrip(&mut compositor);
        run_armed_deadlines(&mut compositor);
    }
}

#[test]
fn a_removed_outputs_description_fails_at_once() {
    let mut compositor = compositor();
    let mut colour = ColourClient::connect(&mut compositor, OWNER_SEAT, 3);
    compositor.remove_output(SDR_OUTPUT).expect("removed");
    let description = colour.output_description(1);
    colour.roundtrip(&mut compositor);
    let events = colour.events(description);
    assert_eq!(events.ready, None);
    assert_eq!(
        events.failed.as_ref().map(|(cause, _)| *cause),
        Some(3),
        "no_output"
    );
}

#[test]
fn zeros_mean_unset_and_a_clients_own_description_gives_no_information() {
    let mut compositor = compositor();
    let mut colour = ColourClient::connect(&mut compositor, OWNER_SEAT, 1);
    let params = colour.params();
    pq_bt2020(&params);
    // As Mesa and mpv send them when a clip has no metadata.
    params.set_mastering_display_primaries(0, 0, 0, 0, 0, 0, 0, 0);
    params.set_mastering_luminance(0, 0);
    params.set_max_cll(0);
    params.set_max_fall(0);
    let description = colour.create(params);
    colour.roundtrip(&mut compositor);
    let events = colour.events(description);
    assert!(events.ready.is_some() && !events.ready2);
    colour.information(description);
    assert_eq!(
        colour.protocol_error(&mut compositor),
        ("wp_image_description_v1".to_owned(), 1),
        "no_information"
    );
}

/// Each request that breaks the protocol ends its client with the error the protocol names,
/// and Yuna goes on: another client still binds the manager.
#[test]
fn every_protocol_error_ends_its_client_alone() {
    type Step = fn(&mut ColourClient, &mut Compositor);
    let cases: [(&str, u32, &str, Step); 16] = [
        (
            "wp_color_manager_v1",
            1,
            "a second colour surface",
            |colour, compositor| {
                let window = mapped_window(compositor, colour);
                let surface = colour.client.window(window).surface.clone();
                colour.colour_surface(&surface);
                colour.colour_surface(&surface);
            },
        ),
        ("wp_color_manager_v1", 0, "an ICC creator", |colour, _| {
            let qh = colour_queue(colour);
            colour
                .state
                .manager
                .as_ref()
                .expect("manager")
                .create_icc_creator(&qh, ());
        }),
        (
            "wp_image_description_creator_params_v1",
            0,
            "no primaries",
            |colour, _| {
                let params = colour.params();
                params.set_tf_named(TransferFunction::Gamma22);
                colour.create(params);
            },
        ),
        (
            "wp_image_description_creator_params_v1",
            1,
            "two transfer functions",
            |colour, _| {
                let params = colour.params();
                params.set_tf_named(TransferFunction::Gamma22);
                params.set_tf_named(TransferFunction::Gamma22);
            },
        ),
        (
            "wp_image_description_creator_params_v1",
            2,
            "a power curve",
            |colour, _| {
                colour.params().set_tf_power(22_000);
            },
        ),
        (
            "wp_image_description_creator_params_v1",
            3,
            "srgb",
            |colour, _| {
                colour.params().set_tf_named(TransferFunction::Srgb);
            },
        ),
        (
            "wp_image_description_creator_params_v1",
            3,
            "an unknown curve",
            |colour, _| {
                send_unknown_tf(&colour.params());
            },
        ),
        (
            "wp_image_description_creator_params_v1",
            4,
            "unknown primaries",
            |colour, _| {
                send_unknown_primaries(&colour.params());
            },
        ),
        (
            "wp_image_description_creator_params_v1",
            5,
            "a maximum at the minimum",
            |colour, _| {
                colour.params().set_luminances(800_000, 80, 100);
            },
        ),
        ("wp_color_manager_v1", 0, "Windows' scRGB", |colour, _| {
            let qh = colour_queue(colour);
            colour
                .state
                .manager
                .as_ref()
                .expect("manager")
                .create_windows_scrgb(&qh, ());
        }),
        (
            "wp_image_description_creator_params_v1",
            5,
            "a mastering maximum at its minimum",
            |colour, _| {
                colour.params().set_mastering_luminance(10_000, 1);
            },
        ),
        (
            "wp_image_description_v1",
            0,
            "the information of a failed description",
            |colour, compositor| {
                let params = colour.params();
                params.set_tf_named(TransferFunction::Gamma22);
                params.set_primaries(
                    100_000, 100_000, 200_000, 200_000, 300_000, 300_000, 312_700, 329_000,
                );
                let description = colour.create(params);
                colour.roundtrip(compositor);
                colour.information(description);
            },
        ),
        (
            "wp_color_management_surface_feedback_v1",
            0,
            "the preferred description of a gone surface",
            |colour, compositor| {
                let window = mapped_window(compositor, colour);
                let surface = colour.client.window(window).surface.clone();
                let feedback = colour.feedback(&surface);
                colour.client.close(window);
                colour.client.destroy_surface(window);
                colour.roundtrip(compositor);
                colour.preferred(feedback);
            },
        ),
        (
            "wp_image_description_creator_params_v1",
            5,
            "a frame average above the peak",
            |colour, _| {
                let params = colour.params();
                pq_bt2020(&params);
                params.set_max_cll(400);
                params.set_max_fall(500);
                colour.create(params);
            },
        ),
        (
            "wp_color_management_surface_v1",
            0,
            "a relative intent",
            |colour, compositor| {
                let window = mapped_window(compositor, colour);
                let surface = colour.client.window(window).surface.clone();
                let params = colour.params();
                pq_bt2020(&params);
                let description = colour.create(params);
                colour.roundtrip(compositor);
                colour
                    .colour_surface(&surface)
                    .set_image_description(colour.description(description), RenderIntent::Relative);
            },
        ),
        (
            "wp_color_management_surface_v1",
            1,
            "a failed description",
            |colour, compositor| {
                let window = mapped_window(compositor, colour);
                let surface = colour.client.window(window).surface.clone();
                let params = colour.params();
                // Primaries on one line fail gracefully; the description is never ready.
                params.set_tf_named(TransferFunction::Gamma22);
                params.set_primaries(
                    100_000, 100_000, 200_000, 200_000, 300_000, 300_000, 312_700, 329_000,
                );
                let description = colour.create(params);
                colour.roundtrip(compositor);
                assert_eq!(
                    colour.events(description).failed.as_ref().map(|f| f.0),
                    Some(1)
                );
                colour.colour_surface(&surface).set_image_description(
                    colour.description(description),
                    RenderIntent::Perceptual,
                );
            },
        ),
    ];
    let mut compositor = compositor();
    for (interface, code, what, step) in cases {
        let mut colour = ColourClient::connect(&mut compositor, OWNER_SEAT, 3);
        step(&mut colour, &mut compositor);
        assert_eq!(
            colour.protocol_error(&mut compositor),
            (interface.to_owned(), code),
            "{what}"
        );
        run_armed_deadlines(&mut compositor);
    }
    let after = ColourClient::connect(&mut compositor, OWNER_SEAT, 3);
    assert!(after.state.listed.done);
    compositor
        .advance(Duration::from_secs(1))
        .expect("the clock moves");
}

#[test]
fn a_destroyed_surfaces_colour_objects_are_inert() {
    let mut compositor = compositor();
    let mut colour = ColourClient::connect(&mut compositor, OWNER_SEAT, 3);
    let window = mapped_window(&mut compositor, &mut colour);
    let surface = colour.client.window(window).surface.clone();
    let colour_surface = colour.colour_surface(&surface);
    colour.client.close(window);
    colour.client.destroy_surface(window);
    colour.roundtrip(&mut compositor);
    colour_surface.unset_image_description();
    assert_eq!(
        colour.protocol_error(&mut compositor),
        ("wp_color_management_surface_v1".to_owned(), 2),
        "inert"
    );
}

fn colour_queue(
    colour: &ColourClient,
) -> wayland_client::QueueHandle<crate::colour_client::ColourState> {
    colour.queue_handle()
}

/// `set_tf_named` with a value no transfer function has, which the typed request cannot send.
fn send_unknown_tf(params: &Params) {
    crate::colour_client::send_raw_uint(params, 1, 999);
}

/// `set_primaries_named` with a value no primaries have.
fn send_unknown_primaries(params: &Params) {
    crate::colour_client::send_raw_uint(params, 3, 999);
}

/// One request of a random sequence: any value, nonsense included. Indices are taken modulo
/// what exists; a request that names nothing does nothing.
#[derive(Debug, Clone)]
enum Step {
    NewParams,
    SetTf(u32),
    SetPrimariesNamed(u32),
    SetPrimaries([i32; 8]),
    SetLuminances(u32, u32, u32),
    SetMasteringPrimaries([i32; 8]),
    SetMasteringLuminance(u32, u32),
    SetMaxCll(u32),
    SetMaxFall(u32),
    Create,
    Information(usize),
    NewColourSurface,
    SetOnSurface(usize, u32),
    Unset,
    DestroyColourSurface,
    Commit,
    Feedback,
    Preferred(usize),
    OutputDescription(usize),
    DestroyDescription(usize),
}

fn step() -> impl proptest::strategy::Strategy<Value = Step> {
    use proptest::prelude::*;
    let luminance =
        prop_oneof![4 => 0..=20_000u32, 1 => Just(0), 1 => Just(u32::MAX), 1 => any::<u32>()];
    let enum_value = prop_oneof![8 => 0..=15u32, 1 => any::<u32>()];
    let coordinate =
        prop_oneof![8 => -100_000..=1_100_000i32, 1 => Just(i32::MIN), 1 => Just(i32::MAX)];
    let chromaticities = proptest::array::uniform8(coordinate);
    prop_oneof![
        2 => Just(Step::NewParams),
        3 => enum_value.clone().prop_map(Step::SetTf),
        3 => enum_value.clone().prop_map(Step::SetPrimariesNamed),
        1 => chromaticities.clone().prop_map(Step::SetPrimaries),
        2 => (luminance.clone(), luminance.clone(), luminance.clone())
            .prop_map(|(a, b, c)| Step::SetLuminances(a, b, c)),
        1 => chromaticities.prop_map(Step::SetMasteringPrimaries),
        1 => (luminance.clone(), luminance.clone()).prop_map(|(a, b)| Step::SetMasteringLuminance(a, b)),
        1 => luminance.clone().prop_map(Step::SetMaxCll),
        1 => luminance.prop_map(Step::SetMaxFall),
        3 => Just(Step::Create),
        2 => any::<usize>().prop_map(Step::Information),
        2 => Just(Step::NewColourSurface),
        3 => (any::<usize>(), 0..=6u32).prop_map(|(i, intent)| Step::SetOnSurface(i, intent)),
        1 => Just(Step::Unset),
        1 => Just(Step::DestroyColourSurface),
        2 => Just(Step::Commit),
        1 => Just(Step::Feedback),
        2 => any::<usize>().prop_map(Step::Preferred),
        2 => any::<usize>().prop_map(Step::OutputDescription),
        1 => any::<usize>().prop_map(Step::DestroyDescription),
    ]
}

/// What a random sequence's client holds.
struct Random {
    colour: ColourClient,
    window: usize,
    params: Option<Params>,
    descriptions: Vec<usize>,
    surface: Option<crate::colour_client::ColourSurface>,
    feedbacks: Vec<usize>,
}

fn connect_random(compositor: &mut Compositor) -> Random {
    let mut colour = ColourClient::connect(compositor, OWNER_SEAT, 3);
    let window = mapped_window(compositor, &mut colour);
    Random {
        colour,
        window,
        params: None,
        descriptions: Vec::new(),
        surface: None,
        feedbacks: Vec::new(),
    }
}

fn pick(index: usize, len: usize) -> Option<usize> {
    (len > 0).then(|| index % len)
}

fn run_step(random: &mut Random, step: &Step) {
    let surface = random.colour.client.window(random.window).surface.clone();
    let colour = &mut random.colour;
    match step {
        Step::NewParams => random.params = Some(colour.params()),
        Step::SetTf(value) => {
            if let Some(params) = &random.params {
                crate::colour_client::send_raw_uint(params, 1, *value);
            }
        }
        Step::SetPrimariesNamed(value) => {
            if let Some(params) = &random.params {
                crate::colour_client::send_raw_uint(params, 3, *value);
            }
        }
        Step::SetPrimaries([a, b, c, d, e, f, g, h]) => {
            if let Some(params) = &random.params {
                params.set_primaries(*a, *b, *c, *d, *e, *f, *g, *h);
            }
        }
        Step::SetLuminances(min, max, reference) => {
            if let Some(params) = &random.params {
                params.set_luminances(*min, *max, *reference);
            }
        }
        Step::SetMasteringPrimaries([a, b, c, d, e, f, g, h]) => {
            if let Some(params) = &random.params {
                params.set_mastering_display_primaries(*a, *b, *c, *d, *e, *f, *g, *h);
            }
        }
        Step::SetMasteringLuminance(min, max) => {
            if let Some(params) = &random.params {
                params.set_mastering_luminance(*min, *max);
            }
        }
        Step::SetMaxCll(value) => {
            if let Some(params) = &random.params {
                params.set_max_cll(*value);
            }
        }
        Step::SetMaxFall(value) => {
            if let Some(params) = &random.params {
                params.set_max_fall(*value);
            }
        }
        Step::Create => {
            if let Some(params) = random.params.take() {
                random.descriptions.push(colour.create(params));
            }
        }
        Step::Information(i) => {
            if let Some(i) = pick(*i, random.descriptions.len()) {
                colour.information(random.descriptions[i]);
            }
        }
        Step::NewColourSurface => random.surface = Some(colour.colour_surface(&surface)),
        Step::SetOnSurface(i, intent) => {
            if let (Some(i), Some(colour_surface)) =
                (pick(*i, random.descriptions.len()), &random.surface)
            {
                crate::colour_client::set_raw_description(
                    colour_surface,
                    colour.description(random.descriptions[i]),
                    *intent,
                );
            }
        }
        Step::Unset => {
            if let Some(colour_surface) = &random.surface {
                colour_surface.unset_image_description();
            }
        }
        Step::DestroyColourSurface => {
            if let Some(colour_surface) = random.surface.take() {
                colour_surface.destroy();
            }
        }
        Step::Commit => surface.commit(),
        Step::Feedback => random.feedbacks.push(colour.feedback(&surface)),
        Step::Preferred(i) => {
            if let Some(i) = pick(*i, random.feedbacks.len()) {
                random
                    .descriptions
                    .push(colour.preferred(random.feedbacks[i]));
            }
        }
        Step::OutputDescription(i) => {
            if let Some(i) = pick(*i, colour.state.outputs.len()) {
                random.descriptions.push(colour.output_description(i));
            }
        }
        Step::DestroyDescription(i) => {
            if let Some(i) = pick(*i, random.descriptions.len()) {
                colour.description(random.descriptions.remove(i)).destroy();
            }
        }
    }
}

/// Random sequences of colour requests: a client that breaks the protocol is ended and replaced,
/// and Yuna stays up throughout; at the end a new client still binds the manager, and Yuna
/// rests.
#[test]
fn random_colour_requests_keep_yuna_up() {
    use proptest::test_runner::{Config as RunnerConfig, TestRunner};
    let mut runner = TestRunner::new(RunnerConfig {
        // `PROPTEST_CASES=<n>` runs more, as in `random_requests.rs`.
        cases: std::env::var("PROPTEST_CASES")
            .ok()
            .and_then(|cases| cases.parse().ok())
            .unwrap_or(48),
        failure_persistence: None,
        ..RunnerConfig::default()
    });
    runner
        .run(&proptest::collection::vec(step(), 1..40), |steps| {
            let mut compositor = compositor();
            let mut random = connect_random(&mut compositor);
            for step in &steps {
                run_step(&mut random, step);
                if random.colour.client.try_roundtrip(&mut compositor).is_err() {
                    random = connect_random(&mut compositor);
                }
            }
            let after = ColourClient::connect(&mut compositor, OWNER_SEAT, 3);
            assert!(after.state.listed.done);
            drop(random);
            drop(after);
            compositor
                .dispatch(Some(Duration::ZERO))
                .expect("dispatched");
            run_armed_deadlines(&mut compositor);
            run_armed_deadlines(&mut compositor);
            Ok(())
        })
        .expect("Yuna stays up");
}

/// A client's feedback objects cost Yuna a constant time each, made, destroyed or dropped with
/// the client: 20,000 of them, then a disconnection, take far less than the quadratic cost a
/// list scanned at each destruction gave (4 s in a debug build, measured by M1.6's review).
#[test]
fn many_feedback_objects_cost_linear_time() {
    let mut compositor = compositor();
    let mut colour = ColourClient::connect(&mut compositor, OWNER_SEAT, 3);
    let window = mapped_window(&mut compositor, &mut colour);
    let surface = colour.client.window(window).surface.clone();
    // In batches, so that the socket's buffers never fill.
    for _ in 0..40 {
        for _ in 0..500 {
            colour.feedback(&surface);
        }
        colour.roundtrip(&mut compositor);
    }
    let began = std::time::Instant::now();
    drop(colour);
    for _ in 0..4 {
        compositor
            .dispatch(Some(Duration::ZERO))
            .expect("dispatched");
    }
    let after = ColourClient::connect(&mut compositor, OWNER_SEAT, 3);
    assert!(after.state.listed.done);
    assert!(
        began.elapsed() < Duration::from_millis(1_500),
        "{:?}",
        began.elapsed()
    );
}
