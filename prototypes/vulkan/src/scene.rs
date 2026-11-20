//! `scene`: the scene the processor tier is judged on (`scene_definition.rs`, ADR-0139), drawn on
//! one device frame after frame as today's Yuna draws it on llvmpipe, so that its figures and
//! those `prototypes/llvmpipe-scene` reads from Yuna compare like for like.
//!
//! A frame does what Yuna's renderer, Smithay's GLES, does for the same commits:
//!
//! - It uploads what the clients damaged into each window's image, from their `wl_shm` pages,
//!   imported once with `VK_EXT_external_memory_host`: one copy, as `glTexSubImage2D` makes. A
//!   window's two buffers alternate, as its client's do.
//! - It draws what the frame damaged, over what the output holds: the background where no window
//!   covers the damage, then each window's damaged part, one quad a rectangle, without blending.
//!   The windows are opaque `XRGB8888` buffers, which Smithay draws without blending and clears
//!   nothing beneath: only the first frame draws any background.
//! - It waits for its fence.
//!
//! A frame's wall time runs from the start of its recording, uploads included, to its fence
//! signalled. Each frame starts at the first boundary of `--rate` (240 Hz by default) after the
//! frame before ended (`scene_definition::wait_for_boundary`); `--rate 0` draws them back to
//! back. That is not Yuna's own cadence: with a client that commits at each frame callback,
//! Yuna's frame loop waits for the boundary its last frame was presented at, so a full-damage
//! frame a little over two periods long comes every second or third boundary. For the same
//! cadence on both sides, pace both at one rate that leaves Yuna a period to reach its next
//! refresh: `--rate 60` here and in `llvmpipe-scene`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ash::vk;

use crate::dmabuf::{image_info, view_info};
use crate::driver;
use crate::gpu::{
    BoundBuffer, Failure, Gpu, Recorder, Scope, Vulkan, buffer_to_host, image_barrier, texel_copy,
};
use crate::load::{GpuFiles, LoadMonitor, Machine};
use crate::scene_definition::{
    BACKGROUND, OUTPUT_HEIGHT, OUTPUT_RATE_HZ, OUTPUT_SCALE_120THS, OUTPUT_WIDTH, REGION, Rect,
    WINDOWS, check_image, texel, wait_for_boundary, write_pam,
};
use crate::scene_report::{Damage, SceneRun, rasterizer_threads, step_counts};
use crate::shm::{host_pointer_alignment, import_pages};
use crate::sys::Mapping;
use crate::threads::{Threads, Usage};

/// Each window's buffers, which its client attaches in turn.
const BUFFERS_A_WINDOW: usize = 2;
const FORMAT: vk::Format = vk::Format::B8G8R8A8_UNORM;
/// The push constants scene.vert and its fragment shaders read: colour, the output's size, the
/// rectangle's origin and size, and the texel at its origin and the texture's size.
const CONSTANTS_BYTES: u32 = 56;

pub struct SceneOptions {
    pub device: usize,
    pub damage: Damage,
    pub warm_up: usize,
    pub frames: usize,
    /// Frames a second, at the output's boundaries; 0 draws them back to back.
    pub rate_hz: f64,
    pub json: PathBuf,
    pub capture: Option<PathBuf>,
}

impl SceneOptions {
    pub fn new() -> Self {
        Self {
            device: 0,
            damage: Damage::Full,
            warm_up: 300,
            frames: 1200,
            rate_hz: OUTPUT_RATE_HZ,
            json: PathBuf::from("scene.json"),
            capture: None,
        }
    }
}

/// Runs the scene on device `options.device`.
pub fn run(options: &SceneOptions, command: &str) -> Result<SceneRun, Failure> {
    let vulkan = Vulkan::new(false)?;
    let physicals = vulkan.physical_devices()?;
    let &physical = physicals.get(options.device).ok_or_else(|| {
        Failure::other(
            "choose the device",
            format!(
                "there is no device {} ({} found)",
                options.device,
                physicals.len()
            ),
        )
    })?;
    let driver = driver::read(&vulkan, physical);
    let gpu = Gpu::new(&vulkan, physical)?;
    if gpu.host_memory.is_none() {
        return Err(Failure::other(
            "import the clients' pages",
            "VK_EXT_external_memory_host is not offered",
        ));
    }
    let machine = Machine::read();
    // A driver without a render node, lavapipe, still has others' GPU load read on the
    // machine's first one.
    let gpu_files = driver
        .render_node
        .and_then(|node| GpuFiles::of_render_node(node.major, node.minor))
        .or_else(GpuFiles::first_render_node);
    let load = LoadMonitor::start(gpu_files, Vec::new());

    // Declared before the scope, so that the pages outlive the memory imported from them.
    let mut pages = ClientPages::new(host_pointer_alignment(&gpu).unwrap_or(4096) as usize)?;
    let mut scope = Scope::new(&gpu);
    let clients = Clients::import(&gpu, &mut scope, &pages)?;
    let drawing = Drawing::new(&gpu, &mut scope)?;

    progress!("scene: the first frame, the whole output");
    drawing.draw(&gpu, &scope, &clients, Frame::First)?;
    progress!(
        "scene: {} damage, {} frames of warm-up then {} measured",
        options.damage.name(),
        options.warm_up,
        options.frames
    );
    let period = (options.rate_hz > 0.0).then(|| Duration::from_secs_f64(1.0 / options.rate_hz));
    let origin = Instant::now();
    let mut last_end = origin;
    let mut wall_us = Vec::with_capacity(options.frames);
    let mut starts = Vec::with_capacity(options.frames);
    let mut window_start = None;
    let total = options.warm_up + options.frames;
    for number in 1..=total as u64 {
        if number == options.warm_up as u64 + 1 {
            window_start = Some((Threads::read(std::process::id()), load.mark()));
        }
        if options.damage == Damage::Partial {
            // The client's own work, before it commits: not the frame's.
            pages.write_region(number);
        }
        if let Some(period) = period {
            wait_for_boundary(origin, last_end, period);
        }
        let start = Instant::now();
        if number > options.warm_up as u64 {
            starts.push(start);
        }
        drawing.draw(
            &gpu,
            &scope,
            &clients,
            Frame::Next {
                number,
                damage: options.damage,
            },
        )?;
        last_end = Instant::now();
        if number > options.warm_up as u64 {
            wall_us.push(last_end.duration_since(start).as_secs_f64() * 1e6);
        }
    }
    let (threads_before, load_start) =
        window_start.ok_or_else(|| Failure::other("measure", "no frame was measured"))?;
    let cpu = Usage::between(&threads_before, &Threads::read(std::process::id()));
    let load_window = load.window("the measured frames", load_start);
    drop(load);

    let pixels = drawing.read_output(&gpu, &mut scope)?;
    // Full damage never changes the region; partial damage leaves the last frame's in it.
    let content_frame = match options.damage {
        Damage::Full => 0,
        Damage::Partial => total as u64,
    };
    let image = check_image(&pixels, content_frame).describe();
    if let Some(path) = &options.capture {
        write_pam(path, &pixels).map_err(|error| {
            Failure::other(format!("write {}", path.display()), error.to_string())
        })?;
        progress!("scene: the output is in {}", path.display());
    }

    let mut facts = vec![
        ("Command", format!("`vulkan-prototype {command}`")),
        (
            "Prototype commit",
            format!("`{}`", env!("PROTOTYPE_COMMIT")),
        ),
        (
            "Device",
            format!(
                "{} ({}), {} {}",
                driver.device_name,
                driver.device_type,
                driver.driver_name.as_deref().unwrap_or("unknown driver"),
                driver.driver_info.as_deref().unwrap_or("")
            ),
        ),
        (
            "Uploads",
            "from the clients' memfds, imported once with VK_EXT_external_memory_host, \
             copied by the device into each window's image"
                .to_owned(),
        ),
        (
            "Frames",
            format!(
                "{} measured after {} of warm-up and a first frame of the whole output; {}",
                options.frames,
                options.warm_up,
                match period {
                    Some(_) => format!(
                        "each started at the first {} Hz boundary after the frame before ended",
                        options.rate_hz
                    ),
                    None => "back to back".to_owned(),
                }
            ),
        ),
        (
            "Wall time",
            "from the start of a frame's recording, its uploads included, to its fence \
             signalled"
                .to_owned(),
        ),
        (
            "Each frame",
            match options.damage {
                Damage::Full => {
                    "uploads both windows' buffers whole and draws both windows' rectangles"
                        .to_owned()
                }
                Damage::Partial => format!(
                    "uploads window 0's {:?} and draws the output's {:?}",
                    REGION,
                    region_drawn()
                ),
            },
        ),
    ];
    for name in ["VK_ICD_FILENAMES", "VK_DRIVER_FILES"] {
        if let Ok(value) = std::env::var(name) {
            facts.push((name, format!("`{value}`")));
        }
    }
    // The output's periods between two frame starts, to the nearest.
    let steps = step_counts(starts.windows(2).map(|pair| {
        (pair[1].duration_since(pair[0]).as_secs_f64() * OUTPUT_RATE_HZ).round() as u64
    }));
    Ok(SceneRun {
        // The device names itself in the facts; the table needs only which tier drew.
        side: if driver.device_type == "CPU" {
            "prototype, lavapipe"
        } else {
            "prototype, a GPU"
        },
        damage: options.damage,
        rasterizer_threads: rasterizer_threads(),
        wall_us,
        cpu,
        load: load_window,
        machine,
        image,
        pacing: match period {
            Some(_) => format!("{} Hz", options.rate_hz),
            None => "back to back".to_owned(),
        },
        steps,
        facts,
    })
}

/// The clients' `wl_shm` memfds: each window's buffers, filled with its texels at frame 0.
struct ClientPages(Vec<Vec<Mapping>>);

impl ClientPages {
    /// Each buffer's length is a multiple of `alignment`, as the device imports it.
    fn new(alignment: usize) -> Result<Self, Failure> {
        let mut windows = Vec::new();
        for (index, window) in WINDOWS.iter().enumerate() {
            let (width, height) = window.buffer;
            let length = (width as usize * height as usize * 4).next_multiple_of(alignment);
            let mut buffers = Vec::new();
            for _ in 0..BUFFERS_A_WINDOW {
                let mut pages = Mapping::memfd(c"wl_shm", length)
                    .map_err(|error| Failure::other("make a client's memfd", error.to_string()))?;
                fill(pages.as_mut_slice(), index, width, height);
                buffers.push(pages);
            }
            windows.push(buffers);
        }
        Ok(Self(windows))
    }

    /// Writes frame `number`'s region of window 0 into the buffer that frame attaches, as its
    /// client draws before it commits. The device reads these pages only while a frame runs,
    /// and each frame waited for its fence.
    fn write_region(&mut self, number: u64) {
        let (width, _) = WINDOWS[0].buffer;
        let pixels = self.0[0][number as usize % BUFFERS_A_WINDOW].as_mut_slice();
        for y in REGION.y..REGION.y + REGION.height {
            for x in REGION.x..REGION.x + REGION.width {
                let at = (y as usize * width as usize + x as usize) * 4;
                pixels[at..at + 4].copy_from_slice(&texel(0, x, y, number));
            }
        }
    }
}

/// Fills a window's buffer with its texels at frame 0.
fn fill(pixels: &mut [u8], window: usize, width: u32, height: u32) {
    for y in 0..height {
        for x in 0..width {
            let at = (y as usize * width as usize + x as usize) * 4;
            pixels[at..at + 4].copy_from_slice(&texel(window, x, y, 0));
        }
    }
}

/// The device's buffers over the clients' pages, imported once, as a compositor imports a
/// `wl_shm` pool.
struct Clients {
    buffers: Vec<Vec<vk::Buffer>>,
}

impl Clients {
    /// Imports every page; `scope` must end before `pages` does.
    fn import(gpu: &Gpu<'_>, scope: &mut Scope<'_>, pages: &ClientPages) -> Result<Self, Failure> {
        let mut buffers = Vec::new();
        for window in &pages.0 {
            let mut own = Vec::new();
            for buffer_pages in window {
                own.push(import_pages(gpu, scope, buffer_pages)?.0);
            }
            buffers.push(own);
        }
        Ok(Self { buffers })
    }

    /// The buffer window `window` attaches at frame `number`.
    fn buffer(&self, window: usize, number: u64) -> vk::Buffer {
        self.buffers[window][number as usize % BUFFERS_A_WINDOW]
    }
}

/// What a frame draws.
#[derive(Clone, Copy)]
enum Frame {
    /// The whole output: every window's buffer 0 uploaded whole, the background drawn.
    First,
    /// Frame `number`, damaged as `damage` says.
    Next { number: u64, damage: Damage },
}

/// The compositor's side: the windows' images, the output, the pipelines, the commands.
struct Drawing {
    windows: Vec<(vk::Image, vk::DescriptorSet)>,
    output: vk::Image,
    output_view: vk::ImageView,
    layout: vk::PipelineLayout,
    texture_pipeline: vk::Pipeline,
    solid_pipeline: vk::Pipeline,
    commands: vk::CommandBuffer,
    fence: vk::Fence,
}

impl Drawing {
    fn new(gpu: &Gpu<'_>, scope: &mut Scope<'_>) -> Result<Self, Failure> {
        let needed = vk::FormatFeatureFlags::COLOR_ATTACHMENT
            | vk::FormatFeatureFlags::TRANSFER_SRC
            | vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR
            | vk::FormatFeatureFlags::TRANSFER_DST;
        // SAFETY: the physical device comes from this instance; the call fills a plain
        // structure.
        let features = unsafe {
            gpu.vulkan
                .instance
                .get_physical_device_format_properties(gpu.physical, FORMAT)
        }
        .optimal_tiling_features;
        if !features.contains(needed) {
            return Err(Failure::other(
                "check the format",
                format!("B8G8R8A8_UNORM offers {features:?}"),
            ));
        }
        let vertex = scope.shader(include_bytes!(concat!(env!("OUT_DIR"), "/scene.vert.spv")))?;
        let texture_fragment = scope.shader(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/scene_texture.frag.spv"
        )))?;
        let solid_fragment = scope.shader(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/scene_solid.frag.spv"
        )))?;
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
                size: CONSTANTS_BYTES,
            }],
        )?;
        let strip = vk::PrimitiveTopology::TRIANGLE_STRIP;
        let texture_pipeline =
            scope.graphics_pipeline(vertex, texture_fragment, layout, FORMAT, strip, false)?;
        let solid_pipeline =
            scope.graphics_pipeline(vertex, solid_fragment, layout, FORMAT, strip, false)?;
        // As Smithay's GLES samples a client's texture: linearly, clamped to its edges.
        let sampler = scope.sampler(
            &vk::SamplerCreateInfo::default()
                .mag_filter(vk::Filter::LINEAR)
                .min_filter(vk::Filter::LINEAR)
                .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
        )?;

        let mut windows = Vec::new();
        for window in &WINDOWS {
            let (width, height) = window.buffer;
            let image = scope.bound_image(
                gpu,
                &image_info(
                    FORMAT,
                    width,
                    height,
                    vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
                ),
            )?;
            let view = scope.image_view(&view_info(image, FORMAT))?;
            let set = scope.descriptor_set(
                set_layout,
                &[vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                    descriptor_count: 1,
                }],
            )?;
            let image_infos = [vk::DescriptorImageInfo {
                sampler,
                image_view: view,
                image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            }];
            let write = [vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&image_infos)];
            // SAFETY: the set, view and sampler come from this device, and the set is new, so
            // no submitted work uses it.
            unsafe { gpu.device.update_descriptor_sets(&write, &[]) };
            windows.push((image, set));
        }
        let output = scope.bound_image(
            gpu,
            &image_info(
                FORMAT,
                OUTPUT_WIDTH,
                OUTPUT_HEIGHT,
                vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
            ),
        )?;
        let output_view = scope.image_view(&view_info(output, FORMAT))?;
        Ok(Self {
            windows,
            output,
            output_view,
            layout,
            texture_pipeline,
            solid_pipeline,
            commands: scope.command_buffer(gpu)?,
            fence: scope.fence(false)?,
        })
    }

    /// Records, submits and waits for one frame.
    fn draw(
        &self,
        gpu: &Gpu<'_>,
        scope: &Scope<'_>,
        clients: &Clients,
        frame: Frame,
    ) -> Result<(), Failure> {
        gpu.record(self.commands, "draw a frame", |commands| {
            self.record(commands, clients, frame)
        })?;
        gpu.submit("draw a frame", self.commands, &[], &[], self.fence)?;
        scope.wait_fence(self.fence, "draw a frame")
    }

    fn record(&self, commands: &Recorder<'_>, clients: &Clients, frame: Frame) {
        let first = matches!(frame, Frame::First);
        // The uploads: each one's window, buffer and region of it.
        let uploads: Vec<(usize, vk::Buffer, Rect)> = match frame {
            Frame::First => (0..WINDOWS.len())
                .map(|window| (window, clients.buffer(window, 0), whole(window)))
                .collect(),
            Frame::Next {
                number,
                damage: Damage::Full,
            } => (0..WINDOWS.len())
                .map(|window| (window, clients.buffer(window, number), whole(window)))
                .collect(),
            Frame::Next {
                number,
                damage: Damage::Partial,
            } => vec![(0, clients.buffer(0, number), REGION)],
        };
        let sampled = if first {
            vk::ImageLayout::UNDEFINED
        } else {
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
        };
        let to_copy: Vec<_> = uploads
            .iter()
            .map(|&(window, _, _)| {
                image_barrier(
                    self.windows[window].0,
                    (sampled, vk::ImageLayout::TRANSFER_DST_OPTIMAL),
                    (
                        vk::PipelineStageFlags2::FRAGMENT_SHADER,
                        vk::AccessFlags2::NONE,
                    ),
                    (
                        vk::PipelineStageFlags2::COPY,
                        vk::AccessFlags2::TRANSFER_WRITE,
                    ),
                )
            })
            .collect();
        commands.barriers(&to_copy, &[]);
        for &(window, buffer, region) in &uploads {
            commands.copy_buffer_to_image(
                buffer,
                self.windows[window].0,
                &[region_copy(window, region)],
            );
        }
        let mut to_draw: Vec<_> = uploads
            .iter()
            .map(|&(window, _, _)| {
                image_barrier(
                    self.windows[window].0,
                    (
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    ),
                    (
                        vk::PipelineStageFlags2::COPY,
                        vk::AccessFlags2::TRANSFER_WRITE,
                    ),
                    (
                        vk::PipelineStageFlags2::FRAGMENT_SHADER,
                        vk::AccessFlags2::SHADER_SAMPLED_READ,
                    ),
                )
            })
            .collect();
        // The output keeps the frame before, which this one draws over.
        to_draw.push(image_barrier(
            self.output,
            (
                if first {
                    vk::ImageLayout::UNDEFINED
                } else {
                    vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
                },
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            ),
            (
                vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            ),
            (
                vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags2::COLOR_ATTACHMENT_READ | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            ),
        ));
        commands.barriers(&to_draw, &[]);

        let damage = frame_damage(frame);
        commands.begin_rendering_over(self.output_view, OUTPUT_WIDTH, OUTPUT_HEIGHT);
        let stages = vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT;
        let background: Vec<Rect> = damage.iter().flat_map(|&rect| uncovered(rect)).collect();
        if !background.is_empty() {
            commands.bind_pipeline(vk::PipelineBindPoint::GRAPHICS, self.solid_pipeline);
            for rect in background {
                commands.push_constants(self.layout, stages, &constants(rect, None));
                commands.draw(4);
            }
        }
        commands.bind_pipeline(vk::PipelineBindPoint::GRAPHICS, self.texture_pipeline);
        for (index, window) in WINDOWS.iter().enumerate() {
            let parts: Vec<Rect> = damage
                .iter()
                .filter_map(|&rect| intersection(rect, window.rect))
                .collect();
            if parts.is_empty() {
                continue;
            }
            commands.bind_set(
                vk::PipelineBindPoint::GRAPHICS,
                self.layout,
                self.windows[index].1,
            );
            for part in parts {
                commands.push_constants(self.layout, stages, &constants(part, Some(index)));
                commands.draw(4);
            }
        }
        commands.end_rendering();
    }

    /// Copies the output back, `B8G8R8A8` rows, once every frame is done.
    fn read_output(&self, gpu: &Gpu<'_>, scope: &mut Scope<'_>) -> Result<Vec<u8>, Failure> {
        let bytes = OUTPUT_WIDTH as usize * OUTPUT_HEIGHT as usize * 4;
        let BoundBuffer { buffer, mapped, .. } = scope.bound_buffer(
            gpu,
            bytes as u64,
            vk::BufferUsageFlags::TRANSFER_DST,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            vk::MemoryPropertyFlags::HOST_CACHED,
        )?;
        let mapped = mapped.ok_or_else(|| Failure::other("map the output's copy", "not mapped"))?;
        gpu.run_once(scope, "copy the output back", |commands| {
            commands.barriers(
                &[image_barrier(
                    self.output,
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
            commands.copy_image_to_buffer(
                self.output,
                buffer,
                &[texel_copy(0, (0, 0), (OUTPUT_WIDTH, OUTPUT_HEIGHT))],
            );
            commands.barriers(
                &[],
                &[buffer_to_host(
                    buffer,
                    vk::PipelineStageFlags2::COPY,
                    vk::AccessFlags2::TRANSFER_WRITE,
                )],
            );
        })?;
        // SAFETY: the mapping is host-coherent and holds the `bytes` the copy wrote, which the
        // fence waited for; the scope keeps it mapped while the slice lives, and it is copied
        // out before the scope can end.
        Ok(unsafe { std::slice::from_raw_parts(mapped, bytes) }.to_vec())
    }
}

/// The whole of window `window`'s buffer, in its texels.
fn whole(window: usize) -> Rect {
    let (width, height) = WINDOWS[window].buffer;
    Rect {
        x: 0,
        y: 0,
        width,
        height,
    }
}

/// The copy of `region` of window `window`'s buffer, rows of its width, into the same texels of
/// its image.
fn region_copy(window: usize, region: Rect) -> vk::BufferImageCopy {
    let (width, height) = WINDOWS[window].buffer;
    let offset = (u64::from(region.y) * u64::from(width) + u64::from(region.x)) * 4;
    vk::BufferImageCopy {
        buffer_row_length: width,
        buffer_image_height: height,
        ..texel_copy(offset, (region.x, region.y), (region.width, region.height))
    }
}

/// What a frame damages of the output, as Smithay's damage tracker finds it: the whole output
/// first, then each window that committed whole, or the region partial damage changed, as
/// `region_drawn` rounds it.
fn frame_damage(frame: Frame) -> Vec<Rect> {
    match frame {
        Frame::First => vec![Rect {
            x: 0,
            y: 0,
            width: OUTPUT_WIDTH,
            height: OUTPUT_HEIGHT,
        }],
        Frame::Next {
            damage: Damage::Full,
            ..
        } => WINDOWS.iter().map(|window| window.rect).collect(),
        Frame::Next {
            damage: Damage::Partial,
            ..
        } => vec![region_drawn()],
    }
}

/// The rectangle of the output that Yuna draws for `REGION`: Smithay takes a surface's damage
/// from its buffer's texels to logical units, rounded outwards, then to the output's pixels,
/// rounded down, so that it grows by up to a pixel on each side. Its upload stays `REGION`.
/// Yuna's trace showed it: 65 × 65 pixels at (922, 1024) for 64 × 64 texels at (899, 1001).
fn region_drawn() -> Rect {
    let scale = f64::from(OUTPUT_SCALE_120THS) / 120.0;
    let logical_start = |texel: u32| (f64::from(texel) / scale).floor();
    let logical_end = |texel: u32| (f64::from(texel) / scale).ceil();
    let (left, top) = (logical_start(REGION.x), logical_start(REGION.y));
    let (right, bottom) = (
        logical_end(REGION.x + REGION.width),
        logical_end(REGION.y + REGION.height),
    );
    let pixel = |logical: f64| (logical * scale).floor() as u32;
    let window = WINDOWS[0].rect;
    Rect {
        x: window.x + pixel(left),
        y: window.y + pixel(top),
        width: pixel(right) - pixel(left),
        height: pixel(bottom) - pixel(top),
    }
}

/// The parts of `rect` that no window covers.
fn uncovered(rect: Rect) -> Vec<Rect> {
    WINDOWS.iter().fold(vec![rect], |parts, window| {
        parts
            .into_iter()
            .flat_map(|part| subtract(part, window.rect))
            .collect()
    })
}

fn intersection(a: Rect, b: Rect) -> Option<Rect> {
    let left = a.x.max(b.x);
    let top = a.y.max(b.y);
    let right = (a.x + a.width).min(b.x + b.width);
    let bottom = (a.y + a.height).min(b.y + b.height);
    (left < right && top < bottom).then_some(Rect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}

/// `from` without `cut`: up to four rectangles, the bands above and below the cut and the parts
/// beside it.
fn subtract(from: Rect, cut: Rect) -> Vec<Rect> {
    let Some(inner) = intersection(from, cut) else {
        return vec![from];
    };
    let (from_right, from_bottom) = (from.x + from.width, from.y + from.height);
    let (inner_right, inner_bottom) = (inner.x + inner.width, inner.y + inner.height);
    [
        Rect {
            height: inner.y - from.y,
            ..from
        },
        Rect {
            y: inner_bottom,
            height: from_bottom - inner_bottom,
            ..from
        },
        Rect {
            y: inner.y,
            width: inner.x - from.x,
            height: inner.height,
            ..from
        },
        Rect {
            x: inner_right,
            y: inner.y,
            width: from_right - inner_right,
            height: inner.height,
        },
    ]
    .into_iter()
    .filter(|part| part.width > 0 && part.height > 0)
    .collect()
}

/// The push constants of `rect`: part of window `window`, or of the background.
fn constants(rect: Rect, window: Option<usize>) -> [u8; CONSTANTS_BYTES as usize] {
    let (texture_origin, texture_size) = match window {
        Some(index) => {
            let window = WINDOWS[index];
            (
                [
                    (rect.x - window.rect.x) as f32,
                    (rect.y - window.rect.y) as f32,
                ],
                [window.buffer.0 as f32, window.buffer.1 as f32],
            )
        }
        None => ([0.0; 2], [1.0; 2]),
    };
    let fields: [f32; 14] = [
        BACKGROUND[0],
        BACKGROUND[1],
        BACKGROUND[2],
        BACKGROUND[3],
        OUTPUT_WIDTH as f32,
        OUTPUT_HEIGHT as f32,
        rect.x as f32,
        rect.y as f32,
        rect.width as f32,
        rect.height as f32,
        texture_origin[0],
        texture_origin[1],
        texture_size[0],
        texture_size[1],
    ];
    let mut bytes = [0u8; CONSTANTS_BYTES as usize];
    for (chunk, field) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(fields) {
        *chunk = field.to_ne_bytes();
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::{Rect, intersection, region_drawn, subtract, uncovered};
    use crate::scene_definition::{OUTPUT_HEIGHT, OUTPUT_WIDTH, WINDOWS};

    #[test]
    fn the_background_is_what_no_window_covers() {
        let output = Rect {
            x: 0,
            y: 0,
            width: OUTPUT_WIDTH,
            height: OUTPUT_HEIGHT,
        };
        let background = uncovered(output);
        let area = |rect: &Rect| u64::from(rect.width) * u64::from(rect.height);
        let windows: u64 = WINDOWS.iter().map(|window| area(&window.rect)).sum();
        assert_eq!(
            background.iter().map(area).sum::<u64>() + windows,
            area(&output)
        );
        for part in &background {
            assert!(
                WINDOWS
                    .iter()
                    .all(|window| intersection(*part, window.rect).is_none())
            );
        }
        assert!(uncovered(WINDOWS[0].rect).is_empty());
        let apart = Rect {
            x: 10,
            y: 10,
            width: 5,
            height: 5,
        };
        assert_eq!(subtract(apart, output), Vec::new());
        assert_eq!(subtract(output, apart).len(), 4);
    }

    #[test]
    fn the_region_is_drawn_where_yunas_trace_showed_it() {
        assert_eq!(
            region_drawn(),
            Rect {
                x: 922,
                y: 1024,
                width: 65,
                height: 65
            }
        );
    }
}
