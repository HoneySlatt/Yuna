//! Part 5: the composition's floor. An output image at the output's size, in 16-bit floats and
//! in 8-bit; scenes of 1, 6 and 15 windows in a grid, each a quad sampling its own client's
//! dma-buf with rounded corners and a soft shadow, blended over a cleared background. One command
//! buffer a frame, recorded again each frame as a compositor does, timed on the CPU (recording
//! and submitting) and on the GPU (timestamps).
//!
//! Each frame acquires every window's buffer from the foreign queue family and releases it back,
//! as a compositor must for buffers a client may draw into again: that is part of the floor.

use std::time::Instant;

use ash::vk;

use crate::dmabuf::{self, PatternWriter, image_info, view_info};
use crate::gpu::{Failure, Gpu, Recorder, Scope, buffer_to_host, image_barrier, texel_copy};
use crate::json::Json;
use crate::load::{LoadMonitor, LoadWindow};
use crate::modifiers::{acquire, release};
use crate::outcome::Check;
use crate::pattern::{Channels, TEST_FORMATS, value};
use crate::stats::{Summary, summary_cell, summary_json};

pub const SCENES: [u32; 3] = [1, 6, 15];
pub const WARM_UP: usize = 100;
pub const FRAMES: usize = 1000;

const OUTPUT_FORMATS: [(vk::Format, &str); 2] = [
    (vk::Format::R16G16B16A16_SFLOAT, "R16G16B16A16_SFLOAT"),
    (vk::Format::B8G8R8A8_UNORM, "B8G8R8A8_UNORM"),
];

/// The scene's look, in pixels: the gap between windows, their corners and their shadow.
const GAP: u32 = 24;
const CORNER_RADIUS: f32 = 12.0;
const SHADOW_RADIUS: f32 = 24.0;
const SHADOW_OFFSET: [f32; 2] = [0.0, 8.0];
const SHADOW_OPACITY: f32 = 0.5;
const BACKGROUND: [f32; 4] = [0.10, 0.11, 0.13, 1.0];

#[derive(Clone, Copy)]
struct Rect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

/// `windows` windows in a grid that fills the output, `GAP` apart and from its edges.
fn grid(windows: u32, width: u32, height: u32) -> Result<Vec<Rect>, Failure> {
    let (columns, rows) = match windows {
        1 => (1, 1),
        6 => (3, 2),
        15 => (5, 3),
        other => (other, 1),
    };
    let cell = |length: u32, count: u32| {
        length
            .checked_sub((count + 1) * GAP)
            .map(|free| free / count)
    };
    let (Some(window_width), Some(window_height)) = (cell(width, columns), cell(height, rows))
    else {
        return Err(Failure::other(
            "lay out the windows",
            "the output is too small",
        ));
    };
    if window_width < 64 || window_height < 64 {
        return Err(Failure::other(
            "lay out the windows",
            "the output is too small",
        ));
    }
    Ok((0..rows)
        .flat_map(|row| {
            (0..columns).map(move |column| Rect {
                x: GAP + column * (window_width + GAP),
                y: GAP + row * (window_height + GAP),
                width: window_width,
                height: window_height,
            })
        })
        .collect())
}

/// The push constants window.vert and window.frag read, field by field in their order.
fn window_constants(rect: Rect, output: (u32, u32)) -> [u8; 44] {
    let fields: [f32; 11] = [
        output.0 as f32,
        output.1 as f32,
        rect.x as f32,
        rect.y as f32,
        rect.width as f32,
        rect.height as f32,
        SHADOW_OFFSET[0],
        SHADOW_OFFSET[1],
        CORNER_RADIUS,
        SHADOW_RADIUS,
        SHADOW_OPACITY,
    ];
    let mut bytes = [0u8; 44];
    for (chunk, field) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(fields) {
        *chunk = field.to_ne_bytes();
    }
    bytes
}

pub struct SceneFigures {
    pub cpu: Option<Summary>,
    pub gpu: Option<Summary>,
    pub check: Check,
}

pub struct SceneResult {
    pub windows: u32,
    pub window_size: Option<(u32, u32)>,
    pub output_format: &'static str,
    pub client_modifiers: Vec<u64>,
    pub outcome: Result<SceneFigures, Failure>,
}

pub struct ComposeResult {
    pub width: u32,
    pub height: u32,
    pub scenes: Vec<SceneResult>,
}

/// The compositor's objects shared by every scene and frame.
struct Compositor {
    set_layout: vk::DescriptorSetLayout,
    layout: vk::PipelineLayout,
    sampler: vk::Sampler,
    pipelines: Vec<vk::Pipeline>,
    outputs: Vec<(vk::Image, vk::ImageView)>,
    commands: vk::CommandBuffer,
    fence: vk::Fence,
    queries: Option<vk::QueryPool>,
}

impl Compositor {
    fn new(
        gpu: &Gpu<'_>,
        scope: &mut Scope<'_>,
        (width, height): (u32, u32),
    ) -> Result<Self, Failure> {
        let vertex = scope.shader(include_bytes!(concat!(env!("OUT_DIR"), "/window.vert.spv")))?;
        let fragment =
            scope.shader(include_bytes!(concat!(env!("OUT_DIR"), "/window.frag.spv")))?;
        let set_layout =
            scope.descriptor_set_layout(&[vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)])?;
        let layout = scope.pipeline_layout(
            &[set_layout],
            &[vk::PushConstantRange {
                stage_flags: vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                offset: 0,
                size: 44,
            }],
        )?;
        let sampler = scope.sampler(
            &vk::SamplerCreateInfo::default()
                .mag_filter(vk::Filter::LINEAR)
                .min_filter(vk::Filter::LINEAR)
                .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
        )?;
        let mut pipelines = Vec::new();
        let mut outputs = Vec::new();
        for (format, name) in OUTPUT_FORMATS {
            // SAFETY: the physical device comes from this instance; the call fills a plain
            // structure.
            let features = unsafe {
                gpu.vulkan
                    .instance
                    .get_physical_device_format_properties(gpu.physical, format)
            }
            .optimal_tiling_features;
            let needed = vk::FormatFeatureFlags::COLOR_ATTACHMENT_BLEND
                | vk::FormatFeatureFlags::TRANSFER_SRC;
            if !features.contains(needed) {
                return Err(Failure::other(
                    "make the output",
                    format!("{name} cannot be blended into or copied from"),
                ));
            }
            pipelines.push(scope.graphics_pipeline(
                vertex,
                fragment,
                layout,
                format,
                vk::PrimitiveTopology::TRIANGLE_STRIP,
                true,
            )?);
            let image = scope.image(&image_info(
                format,
                width,
                height,
                vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
            ))?;
            // SAFETY: the image was just made on this device and has no memory yet.
            let requirements = unsafe { gpu.device.get_image_memory_requirements(image) };
            let memory_type = gpu
                .memory_type(
                    requirements.memory_type_bits,
                    vk::MemoryPropertyFlags::empty(),
                    vk::MemoryPropertyFlags::DEVICE_LOCAL,
                )
                .ok_or_else(|| Failure::other("allocate the output", "no memory type fits"))?;
            let memory = scope.memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(requirements.size)
                    .memory_type_index(memory_type),
                "allocate the output",
            )?;
            // SAFETY: the memory is new, of a type the image allows, and as large as it needs.
            unsafe { gpu.device.bind_image_memory(image, memory, 0) }
                .map_err(|result| Failure::vulkan("bind the output", result))?;
            outputs.push((image, scope.image_view(&view_info(image, format))?));
        }
        Ok(Self {
            set_layout,
            layout,
            sampler,
            pipelines,
            outputs,
            commands: scope.command_buffer(gpu)?,
            fence: scope.fence(false)?,
            queries: scope.timestamp_pool(gpu)?,
        })
    }
}

/// One window: its client's imported image, where it is drawn, its descriptor set.
struct Window {
    image: vk::Image,
    rect: Rect,
    set: vk::DescriptorSet,
    constants: [u8; 44],
}

/// The clients' side: their device, what they made on it, how they draw, and the modifiers their
/// driver may choose among.
struct Clients<'a> {
    gpu: &'a Gpu<'a>,
    scope: Scope<'a>,
    writer: PatternWriter,
    modifiers: &'a [u64],
}

pub fn run(
    client: &Gpu<'_>,
    compositor: &Gpu<'_>,
    working_modifiers: &[u64],
    size: (u32, u32),
    load: &LoadMonitor,
    load_windows: &mut Vec<LoadWindow>,
) -> Result<ComposeResult, Failure> {
    if working_modifiers.is_empty() {
        return Err(Failure::other(
            "choose the clients' modifier",
            "no B8G8R8A8_UNORM modifier was imported and sampled correctly in part 2",
        ));
    }
    let largest = compositor.limits.max_image_dimension2_d;
    if size.0 > largest || size.1 > largest {
        return Err(Failure::other(
            "make the output",
            format!("larger than {largest}, the device's largest image"),
        ));
    }
    let mut shared = Scope::new(compositor);
    let parts = Compositor::new(compositor, &mut shared, size)?;
    let mut client_scope = Scope::new(client);
    let writer = PatternWriter::new(&mut client_scope, TEST_FORMATS[0])?;
    let mut clients = Clients {
        gpu: client,
        scope: client_scope,
        writer,
        modifiers: working_modifiers,
    };

    let mut scenes = Vec::new();
    for count in SCENES {
        let mut scene_scope = Scope::new(compositor);
        let scene = make_scene(
            &mut clients,
            compositor,
            &mut scene_scope,
            &parts,
            count,
            size,
        );
        for (index, (_, name)) in OUTPUT_FORMATS.iter().enumerate() {
            let (window_size, client_modifiers, outcome) = match &scene {
                Ok((windows_drawn, modifiers)) => {
                    let scene_name = format!("{count} window{}", if count == 1 { "" } else { "s" });
                    eprintln!("vulkan-prototype: composition: {scene_name} into {name}");
                    let start = load.mark();
                    let outcome = measure(
                        compositor,
                        &mut scene_scope,
                        &parts,
                        index,
                        windows_drawn,
                        size,
                    );
                    if outcome.is_ok() {
                        load_windows
                            .push(load.window(format!("composition, {scene_name}, {name}"), start));
                    }
                    let first = windows_drawn
                        .first()
                        .map(|window| (window.rect.width, window.rect.height));
                    (first, modifiers.clone(), outcome)
                }
                Err(failure) => (None, Vec::new(), Err(failure.clone())),
            };
            scenes.push(SceneResult {
                windows: count,
                window_size,
                output_format: name,
                client_modifiers,
                outcome,
            });
        }
    }
    Ok(ComposeResult {
        width: size.0,
        height: size.1,
        scenes,
    })
}

/// The scene's windows: each client draws its own image, of its window's size, with the
/// modifier its driver chooses among those that worked, and the compositor imports it.
fn make_scene(
    clients: &mut Clients<'_>,
    compositor: &Gpu<'_>,
    scope: &mut Scope<'_>,
    parts: &Compositor,
    count: u32,
    size: (u32, u32),
) -> Result<(Vec<Window>, Vec<u64>), Failure> {
    let mut windows = Vec::new();
    let mut chosen = Vec::new();
    for rect in grid(count, size.0, size.1)? {
        let exported = dmabuf::export_pattern(
            clients.gpu,
            &mut clients.scope,
            &clients.writer,
            clients.modifiers,
            (rect.width, rect.height),
            true,
        )
        .map_err(|(_, failure)| failure)?;
        if !chosen.contains(&exported.modifier) {
            chosen.push(exported.modifier);
        }
        let image = dmabuf::import(compositor, scope, exported, vk::ImageUsageFlags::SAMPLED)?;
        let view = scope.image_view(&view_info(image, vk::Format::B8G8R8A8_UNORM))?;
        let set = scope.descriptor_set(
            parts.set_layout,
            &[vk::DescriptorPoolSize {
                ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                descriptor_count: 1,
            }],
        )?;
        let image_info = [vk::DescriptorImageInfo {
            sampler: parts.sampler,
            image_view: view,
            image_layout: vk::ImageLayout::GENERAL,
        }];
        let write = [vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&image_info)];
        // SAFETY: the set, view and sampler come from this device, and the set is new, so no
        // submitted work uses it.
        unsafe { compositor.device.update_descriptor_sets(&write, &[]) };
        windows.push(Window {
            image,
            rect,
            set,
            constants: window_constants(rect, size),
        });
    }
    Ok((windows, chosen))
}

fn record_frame(
    commands: &Recorder<'_>,
    parts: &Compositor,
    format_index: usize,
    windows: &[Window],
    (acquires, releases): (
        &[vk::ImageMemoryBarrier2<'_>],
        &[vk::ImageMemoryBarrier2<'_>],
    ),
    size: (u32, u32),
) {
    let (_, view) = parts.outputs[format_index];
    commands.start_timestamps(parts.queries);
    commands.barriers(acquires, &[]);
    commands.begin_rendering(view, size.0, size.1, Some(BACKGROUND));
    commands.bind_pipeline(
        vk::PipelineBindPoint::GRAPHICS,
        parts.pipelines[format_index],
    );
    for window in windows {
        commands.bind_set(vk::PipelineBindPoint::GRAPHICS, parts.layout, window.set);
        commands.push_constants(
            parts.layout,
            vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
            &window.constants,
        );
        commands.draw(4);
    }
    commands.end_rendering();
    commands.barriers(releases, &[]);
    commands.end_timestamps(parts.queries);
}

fn measure(
    gpu: &Gpu<'_>,
    scope: &mut Scope<'_>,
    parts: &Compositor,
    format_index: usize,
    windows: &[Window],
    size: (u32, u32),
) -> Result<SceneFigures, Failure> {
    let (output, _) = parts.outputs[format_index];
    // The output's barrier, then each window's acquire; built once, as a compositor would.
    let mut acquires = vec![image_barrier(
        output,
        (
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        ),
        (
            vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT | vk::PipelineStageFlags2::COPY,
            vk::AccessFlags2::COLOR_ATTACHMENT_WRITE | vk::AccessFlags2::TRANSFER_READ,
        ),
        (
            vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
            vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
        ),
    )];
    acquires.extend(
        windows
            .iter()
            .map(|window| acquire(gpu, window.image, vk::PipelineStageFlags2::FRAGMENT_SHADER)),
    );
    let releases: Vec<_> = windows
        .iter()
        .map(|window| release(gpu, window.image, vk::PipelineStageFlags2::FRAGMENT_SHADER))
        .collect();

    let mut cpu = Vec::with_capacity(FRAMES);
    let mut gpu_times = Vec::with_capacity(FRAMES);
    for frame in 0..WARM_UP + FRAMES {
        let start = Instant::now();
        gpu.record(parts.commands, "draw a frame", |commands| {
            record_frame(
                commands,
                parts,
                format_index,
                windows,
                (&acquires, &releases),
                size,
            )
        })?;
        gpu.submit("draw a frame", parts.commands, &[], &[], parts.fence)?;
        let submitted = Instant::now();
        scope.wait_fence(parts.fence, "draw a frame")?;
        let gpu_us = gpu.elapsed_us(parts.queries)?;
        if frame >= WARM_UP {
            cpu.push(submitted.duration_since(start).as_secs_f64() * 1e6);
            gpu_times.extend(gpu_us);
        }
    }
    let check = Check::from_result(check_window_centres(
        gpu,
        scope,
        parts,
        format_index,
        windows,
    ));
    Ok(SceneFigures {
        cpu: Summary::of(&cpu),
        gpu: Summary::of(&gpu_times),
        check,
    })
}

/// Reads back the last frame's pixel at each window's centre, which must be its client's texel
/// there: the scene was drawn, and drawn where it should be.
fn check_window_centres(
    gpu: &Gpu<'_>,
    scope: &mut Scope<'_>,
    parts: &Compositor,
    format_index: usize,
    windows: &[Window],
) -> Result<String, Failure> {
    let (output, _) = parts.outputs[format_index];
    let (format, _) = OUTPUT_FORMATS[format_index];
    let pixel_bytes: u64 = if format == vk::Format::R16G16B16A16_SFLOAT {
        8
    } else {
        4
    };
    let (buffer, mapped) = scope.bound_buffer(
        gpu,
        pixel_bytes * windows.len() as u64,
        vk::BufferUsageFlags::TRANSFER_DST,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        vk::MemoryPropertyFlags::HOST_CACHED,
    )?;
    let mapped = mapped.ok_or_else(|| Failure::other("map the pixels", "not mapped"))?;
    let centres: Vec<(u32, u32)> = windows
        .iter()
        .map(|window| (window.rect.width / 2, window.rect.height / 2))
        .collect();
    let regions: Vec<vk::BufferImageCopy> = windows
        .iter()
        .zip(&centres)
        .enumerate()
        .map(|(index, (window, &(x, y)))| {
            texel_copy(
                index as u64 * pixel_bytes,
                (window.rect.x + x, window.rect.y + y),
                (1, 1),
            )
        })
        .collect();
    gpu.run_once(scope, "read the windows' centres", |commands| {
        commands.barriers(
            &[image_barrier(
                output,
                (
                    vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                ),
                (
                    vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                    vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                ),
                (
                    vk::PipelineStageFlags2::COPY,
                    vk::AccessFlags2::TRANSFER_READ,
                ),
            )],
            &[],
        );
        commands.copy_image_to_buffer(output, buffer, &regions);
        commands.barriers(
            &[],
            &[buffer_to_host(
                buffer,
                vk::PipelineStageFlags2::COPY,
                vk::AccessFlags2::TRANSFER_WRITE,
            )],
        );
    })?;
    let bytes = (pixel_bytes as usize) * windows.len();
    // SAFETY: the mapping is host-coherent and holds the `bytes` the copy wrote, which the fence
    // waited for; the scope keeps it mapped while the slice lives.
    let read = unsafe { std::slice::from_raw_parts(mapped, bytes) };
    for (index, &(x, y)) in centres.iter().enumerate() {
        let pixel = &read[index * pixel_bytes as usize..(index + 1) * pixel_bytes as usize];
        let found: [f32; 4] = if pixel_bytes == 8 {
            std::array::from_fn(|channel| {
                half_to_f32(u16::from_ne_bytes([
                    pixel[channel * 2],
                    pixel[channel * 2 + 1],
                ]))
            })
        } else {
            // B8G8R8A8 in memory.
            let [blue, green, red, alpha] = [pixel[0], pixel[1], pixel[2], pixel[3]];
            [red, green, blue, alpha].map(|channel| f32::from(channel) / 255.0)
        };
        let expected = value(Channels::Eight, x, y, true);
        let off = found
            .iter()
            .zip(expected)
            .map(|(found, expected)| (found - expected).abs())
            .fold(0.0f32, f32::max);
        if off > 2.5 / 255.0 {
            return Err(Failure::other(
                "check the windows' centres",
                format!("window {index}: expected {expected:?}, found {found:?}"),
            ));
        }
    }
    Ok("each window's centre shows its client's texel".to_owned())
}

/// An IEEE 754 half-precision float's value.
fn half_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let fraction = f32::from(bits & 0x3ff);
    sign * match exponent {
        0 => fraction * 2f32.powi(-24),
        31 if fraction == 0.0 => f32::INFINITY,
        31 => f32::NAN,
        _ => (1.0 + fraction / 1024.0) * 2f32.powi(exponent - 15),
    }
}

pub fn to_json(result: &ComposeResult) -> Json {
    Json::Object(vec![
        ("width", Json::Integer(i64::from(result.width))),
        ("height", Json::Integer(i64::from(result.height))),
        ("frames", Json::Integer(FRAMES as i64)),
        ("warm_up", Json::Integer(WARM_UP as i64)),
        (
            "scenes",
            Json::Array(
                result
                    .scenes
                    .iter()
                    .map(|scene| {
                        let mut fields = vec![
                            ("windows", Json::Integer(i64::from(scene.windows))),
                            (
                                "window_size",
                                Json::optional(scene.window_size, |(width, height)| {
                                    Json::text(format!("{width}x{height}"))
                                }),
                            ),
                            ("output_format", Json::text(scene.output_format)),
                            (
                                "client_modifiers",
                                Json::Array(
                                    scene
                                        .client_modifiers
                                        .iter()
                                        .map(|modifier| Json::text(format!("{modifier:#018x}")))
                                        .collect(),
                                ),
                            ),
                        ];
                        match &scene.outcome {
                            Ok(figures) => {
                                fields.push(("cpu_record_submit_us", summary_json(figures.cpu)));
                                fields.push(("gpu_us", summary_json(figures.gpu)));
                                fields.push(("check", figures.check.to_json()));
                            }
                            Err(failure) => {
                                fields.push(("failure", Check::Failed(failure.clone()).to_json()))
                            }
                        }
                        Json::Object(fields)
                    })
                    .collect(),
            ),
        ),
    ])
}

pub fn to_markdown(result: &ComposeResult, out: &mut String) {
    out.push_str(&format!(
        "An output of {}x{}; windows in a grid {GAP} px apart, corners of {CORNER_RADIUS} px, a \
         shadow of {SHADOW_RADIUS} px; {FRAMES} frames after {WARM_UP} of warm-up, waiting for \
         each. Microseconds, median / 99th percentile / longest. CPU: recording and submitting \
         the frame's command buffer. GPU: timestamps around the frame.\n\n",
        result.width, result.height
    ));
    out.push_str("| Windows | Window size | Output | Clients' modifier | CPU | GPU | Check |\n|---|---|---|---|---|---|---|\n");
    for scene in &result.scenes {
        let size = scene.window_size.map_or_else(
            || "-".to_owned(),
            |(width, height)| format!("{width}x{height}"),
        );
        let modifiers = if scene.client_modifiers.is_empty() {
            "-".to_owned()
        } else {
            scene
                .client_modifiers
                .iter()
                .map(|&modifier| dmabuf::modifier_name(modifier))
                .collect::<Vec<_>>()
                .join(", ")
        };
        match &scene.outcome {
            Ok(figures) => out.push_str(&format!(
                "| {} | {size} | {} | {modifiers} | {} | {} | {} |\n",
                scene.windows,
                scene.output_format,
                summary_cell(figures.cpu),
                summary_cell(figures.gpu),
                figures.check.to_cell()
            )),
            Err(failure) => out.push_str(&format!(
                "| {} | {size} | {} | {modifiers} | | | {} |\n",
                scene.windows,
                scene.output_format,
                Check::Failed(failure.clone()).to_cell()
            )),
        }
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::{grid, half_to_f32};

    #[test]
    fn lays_out_grids_and_reads_halves() {
        let windows = grid(15, 3840, 2160).expect("a 4K output holds 15 windows");
        assert_eq!(windows.len(), 15);
        let last = windows[14];
        assert!(last.x + last.width <= 3840 - 24 && last.y + last.height <= 2160 - 24);
        assert!(grid(15, 200, 200).is_err());
        assert_eq!(half_to_f32(0x3c00), 1.0);
        assert_eq!(half_to_f32(0x3555), 0.333_251_95);
        assert_eq!(half_to_f32(0xc000), -2.0);
    }
}
