//! Part 4: how a `wl_shm` buffer reaches the GPU. A client's ARGB8888 pixels in a `memfd`,
//! mapped, uploaded into a sampled image either (a) copied by the CPU into a staging buffer, then
//! by the GPU into the image, or (b) its pages imported as a buffer with
//! `VK_EXT_external_memory_host` and copied by the GPU alone. Each upload is timed on the CPU and
//! on the GPU, and the image is checked once against the client's pixels.

use std::time::Instant;

use ash::vk;

use crate::dmabuf::image_info;
use crate::gpu::{Failure, Gpu, Recorder, Scope, buffer_to_host, image_barrier, texel_copy};
use crate::json::Json;
use crate::load::{LoadMonitor, LoadWindow};
use crate::outcome::Check;
use crate::pattern::{Channels, codes};
use crate::stats::{Summary, summary_cell, summary_json};
use crate::sys::Mapping;

pub const SIZES: [(u32, u32); 2] = [(1920, 1080), (3840, 2160)];
pub const WARM_UP: usize = 10;
pub const UPLOADS: usize = 200;

/// The figures of one way of uploading, in microseconds.
pub struct Figures {
    /// Importing the client's pages, done once a pool rather than once an upload.
    pub import_us: Option<f64>,
    /// The CPU's time an upload: the copy into the staging buffer, if any, recording and
    /// submitting.
    pub cpu: Option<Summary>,
    /// The GPU's time an upload, between timestamps around the copy.
    pub gpu: Option<Summary>,
    /// From the upload's start until the fence says the image is ready.
    pub wall: Option<Summary>,
    pub pixels: Check,
}

pub enum PathResult {
    Measured(Figures),
    Failed(Failure),
    Skipped(String),
}

pub struct ShmResult {
    pub width: u32,
    pub height: u32,
    pub host_pointer_alignment: Option<u64>,
    pub staging: PathResult,
    pub host_pointer: PathResult,
}

pub fn run(gpu: &Gpu<'_>, load: &LoadMonitor, windows: &mut Vec<LoadWindow>) -> Vec<ShmResult> {
    let alignment = host_pointer_alignment(gpu);
    SIZES
        .iter()
        .map(|&(width, height)| {
            let bytes = width as usize * height as usize * 4;
            let length = bytes.next_multiple_of(alignment.unwrap_or(4096) as usize);
            let mut client = match Mapping::memfd(c"wl_shm", length) {
                Ok(memory) => memory,
                Err(error) => {
                    let failure = Failure::other("make the client's memfd", error.to_string());
                    return ShmResult {
                        width,
                        height,
                        host_pointer_alignment: alignment,
                        staging: PathResult::Failed(failure.clone()),
                        host_pointer: PathResult::Failed(failure),
                    };
                }
            };
            fill_argb(client.as_mut_slice(), width, height);

            eprintln!("vulkan-prototype: wl_shm: {width}x{height} through a staging buffer");
            let start = load.mark();
            let staging = match upload_through_staging(gpu, &client, width, height) {
                Ok(figures) => {
                    windows.push(load.window(format!("wl_shm {width}x{height}, staging"), start));
                    PathResult::Measured(figures)
                }
                Err(failure) => PathResult::Failed(failure),
            };

            let host_pointer = if gpu.host_memory.is_none() {
                PathResult::Skipped("VK_EXT_external_memory_host is not offered".to_owned())
            } else {
                eprintln!("vulkan-prototype: wl_shm: {width}x{height} through its pages imported");
                let start = load.mark();
                match upload_from_host_pointer(gpu, &client, width, height) {
                    Ok(figures) => {
                        windows.push(
                            load.window(format!("wl_shm {width}x{height}, host pointer"), start),
                        );
                        PathResult::Measured(figures)
                    }
                    Err(failure) => PathResult::Failed(failure),
                }
            };
            ShmResult {
                width,
                height,
                host_pointer_alignment: alignment,
                staging,
                host_pointer,
            }
        })
        .collect()
}

fn host_pointer_alignment(gpu: &Gpu<'_>) -> Option<u64> {
    gpu.host_memory.as_ref()?;
    let mut host = vk::PhysicalDeviceExternalMemoryHostPropertiesEXT::default();
    let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut host);
    // SAFETY: the device offers external_memory_host (its function table exists only then), so
    // its properties structure may be chained; the chain lives for the call.
    unsafe {
        gpu.vulkan
            .instance
            .get_physical_device_properties2(gpu.physical, &mut properties)
    };
    Some(host.min_imported_host_pointer_alignment)
}

/// The client's pixels: the 8-bit pattern, opaque, as ARGB8888 lays it out in memory (B, G, R,
/// A), which `B8G8R8A8_UNORM` reads.
fn fill_argb(pixels: &mut [u8], width: u32, height: u32) {
    for y in 0..height {
        for x in 0..width {
            let [red, green, blue, alpha] = codes(Channels::Eight, x, y, true);
            let at = (y as usize * width as usize + x as usize) * 4;
            pixels[at..at + 4].copy_from_slice(&[blue as u8, green as u8, red as u8, alpha as u8]);
        }
    }
}

/// What both ways share: the sampled image, the commands, the fence and the timestamps.
struct Upload {
    image: vk::Image,
    commands: vk::CommandBuffer,
    fence: vk::Fence,
    queries: Option<vk::QueryPool>,
    width: u32,
    height: u32,
}

impl Upload {
    fn new(gpu: &Gpu<'_>, scope: &mut Scope<'_>, width: u32, height: u32) -> Result<Self, Failure> {
        if width > gpu.limits.max_image_dimension2_d || height > gpu.limits.max_image_dimension2_d {
            return Err(Failure::other(
                "make the image",
                "larger than the device's largest image",
            ));
        }
        let image = scope.image(&image_info(
            vk::Format::B8G8R8A8_UNORM,
            width,
            height,
            vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_DST
                | vk::ImageUsageFlags::TRANSFER_SRC,
        ))?;
        // SAFETY: the image was just made on this device and has no memory yet.
        let requirements = unsafe { gpu.device.get_image_memory_requirements(image) };
        let memory_type = gpu
            .memory_type(
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
                vk::MemoryPropertyFlags::empty(),
            )
            .or_else(|| {
                gpu.memory_type(
                    requirements.memory_type_bits,
                    vk::MemoryPropertyFlags::empty(),
                    vk::MemoryPropertyFlags::empty(),
                )
            })
            .ok_or_else(|| Failure::other("allocate the image", "no memory type fits"))?;
        let memory = scope.memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(requirements.size)
                .memory_type_index(memory_type),
            "allocate the image",
        )?;
        // SAFETY: the memory is new, of a type the image allows, and as large as it needs.
        unsafe { gpu.device.bind_image_memory(image, memory, 0) }
            .map_err(|result| Failure::vulkan("bind the image", result))?;
        Ok(Self {
            image,
            commands: scope.command_buffer(gpu)?,
            fence: scope.fence(false)?,
            queries: scope.timestamp_pool(gpu)?,
            width,
            height,
        })
    }

    /// Records the GPU's copy of `source` into the image, between timestamps.
    fn record_copy(&self, commands: &Recorder<'_>, source: vk::Buffer) {
        commands.start_timestamps(self.queries);
        commands.barriers(
            &[image_barrier(
                self.image,
                (
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                ),
                (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
                (
                    vk::PipelineStageFlags2::COPY,
                    vk::AccessFlags2::TRANSFER_WRITE,
                ),
            )],
            &[],
        );
        commands.copy_buffer_to_image(
            source,
            self.image,
            &[texel_copy(0, (0, 0), (self.width, self.height))],
        );
        commands.barriers(
            &[image_barrier(
                self.image,
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
            )],
            &[],
        );
        commands.end_timestamps(self.queries);
    }

    /// Times `WARM_UP` then `UPLOADS` uploads: `before_record` runs first in each, on the CPU.
    fn measure(
        &self,
        gpu: &Gpu<'_>,
        scope: &Scope<'_>,
        source: vk::Buffer,
        mut before_record: impl FnMut(),
    ) -> Result<Times, Failure> {
        let mut times = Times {
            cpu: Vec::with_capacity(UPLOADS),
            gpu: Vec::with_capacity(UPLOADS),
            wall: Vec::with_capacity(UPLOADS),
        };
        for upload in 0..WARM_UP + UPLOADS {
            let start = Instant::now();
            before_record();
            gpu.record(self.commands, "upload", |commands| {
                self.record_copy(commands, source)
            })?;
            gpu.submit("upload", self.commands, &[], &[], self.fence)?;
            let submitted = Instant::now();
            scope.wait_fence(self.fence, "upload")?;
            let done = Instant::now();
            let gpu_us = gpu.elapsed_us(self.queries)?;
            if upload >= WARM_UP {
                times
                    .cpu
                    .push(submitted.duration_since(start).as_secs_f64() * 1e6);
                times
                    .wall
                    .push(done.duration_since(start).as_secs_f64() * 1e6);
                times.gpu.extend(gpu_us);
            }
        }
        Ok(times)
    }

    /// Copies the image back and compares it with the client's bytes.
    fn check(
        &self,
        gpu: &Gpu<'_>,
        scope: &mut Scope<'_>,
        client: &[u8],
    ) -> Result<String, Failure> {
        let bytes = self.width as usize * self.height as usize * 4;
        let (buffer, mapped) = scope.bound_buffer(
            gpu,
            bytes as u64,
            vk::BufferUsageFlags::TRANSFER_DST,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            vk::MemoryPropertyFlags::HOST_CACHED,
        )?;
        let mapped = mapped.ok_or_else(|| Failure::other("map the copy", "not mapped"))?;
        gpu.run_once(scope, "copy the image back", |commands| {
            commands.barriers(
                &[image_barrier(
                    self.image,
                    (
                        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    ),
                    // The uploads' fence was waited for: their writes are done and available.
                    (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
                    (
                        vk::PipelineStageFlags2::COPY,
                        vk::AccessFlags2::TRANSFER_READ,
                    ),
                )],
                &[],
            );
            commands.copy_image_to_buffer(
                self.image,
                buffer,
                &[texel_copy(0, (0, 0), (self.width, self.height))],
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
        // fence waited for; the scope keeps it mapped while the slice lives.
        let copied = unsafe { std::slice::from_raw_parts(mapped, bytes) };
        let wrong = copied
            .as_chunks::<4>()
            .0
            .iter()
            .zip(client[..bytes].as_chunks::<4>().0)
            .filter(|(copied, client)| copied != client)
            .count();
        if wrong == 0 {
            Ok("every pixel as the client wrote it".to_owned())
        } else {
            Err(Failure::other(
                "check the image",
                format!("{wrong} pixels differ"),
            ))
        }
    }
}

/// Each measured upload's times, in microseconds.
struct Times {
    cpu: Vec<f64>,
    gpu: Vec<f64>,
    wall: Vec<f64>,
}

fn figures(import_us: Option<f64>, times: Times, pixels: Check) -> Figures {
    Figures {
        import_us,
        cpu: Summary::of(&times.cpu),
        gpu: Summary::of(&times.gpu),
        wall: Summary::of(&times.wall),
        pixels,
    }
}

fn upload_through_staging(
    gpu: &Gpu<'_>,
    client: &Mapping,
    width: u32,
    height: u32,
) -> Result<Figures, Failure> {
    let mut scope = Scope::new(gpu);
    let upload = Upload::new(gpu, &mut scope, width, height)?;
    let bytes = width as usize * height as usize * 4;
    let (staging, mapped) = scope.bound_buffer(
        gpu,
        bytes as u64,
        vk::BufferUsageFlags::TRANSFER_SRC,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        vk::MemoryPropertyFlags::empty(),
    )?;
    let mapped = mapped.ok_or_else(|| Failure::other("map the staging buffer", "not mapped"))?;
    let pixels = &client.as_slice()[..bytes];
    let times = upload.measure(gpu, &scope, staging, || {
        // SAFETY: the staging buffer's mapping holds `bytes` bytes and the GPU is not reading it:
        // the previous upload's fence was waited for. It cannot overlap the client's memfd.
        let staging_bytes = unsafe { std::slice::from_raw_parts_mut(mapped, bytes) };
        staging_bytes.copy_from_slice(pixels);
    })?;
    let check = Check::from_result(upload.check(gpu, &mut scope, client.as_slice()));
    Ok(figures(None, times, check))
}

fn upload_from_host_pointer(
    gpu: &Gpu<'_>,
    client: &Mapping,
    width: u32,
    height: u32,
) -> Result<Figures, Failure> {
    let handle_type = vk::ExternalMemoryHandleTypeFlags::HOST_ALLOCATION_EXT;
    let external_info = vk::PhysicalDeviceExternalBufferInfo::default()
        .usage(vk::BufferUsageFlags::TRANSFER_SRC)
        .handle_type(handle_type);
    let mut external = vk::ExternalBufferProperties::default();
    // SAFETY: the physical device comes from this instance; the structures live for the call.
    unsafe {
        gpu.vulkan
            .instance
            .get_physical_device_external_buffer_properties(
                gpu.physical,
                &external_info,
                &mut external,
            )
    };
    let features = external.external_memory_properties.external_memory_features;
    if !features.contains(vk::ExternalMemoryFeatureFlags::IMPORTABLE) {
        return Err(Failure::other(
            "check host memory can be imported",
            format!("the driver reports {features:?}"),
        ));
    }

    // The imported memory lives in this local scope, freed before the function returns, while
    // the caller still holds the mapping it imports.
    let mut scope = Scope::new(gpu);
    let upload = Upload::new(gpu, &mut scope, width, height)?;
    let started = Instant::now();
    let buffer = match import_pages(gpu, &mut scope, client) {
        Ok(buffer) => buffer,
        Err(failure) => return Err(compare_with_anonymous_memory(gpu, client.len(), failure)),
    };
    let import_us = started.elapsed().as_secs_f64() * 1e6;

    let times = upload.measure(gpu, &scope, buffer, || {})?;
    let check = Check::from_result(upload.check(gpu, &mut scope, client.as_slice()));
    drop(scope);
    Ok(figures(Some(import_us), times, check))
}

/// Imports `pages` as a buffer the GPU copies from, with `VK_EXT_external_memory_host`. The
/// buffer's memory lives in `scope`, which must end before the mapping does.
fn import_pages(
    gpu: &Gpu<'_>,
    scope: &mut Scope<'_>,
    pages: &Mapping,
) -> Result<vk::Buffer, Failure> {
    let Some(host_memory) = &gpu.host_memory else {
        return Err(Failure::other(
            "import",
            "VK_EXT_external_memory_host is not enabled",
        ));
    };
    let handle_type = vk::ExternalMemoryHandleTypeFlags::HOST_ALLOCATION_EXT;
    let pointer = pages.as_ptr().cast::<std::ffi::c_void>();
    let mut pointer_properties = vk::MemoryHostPointerPropertiesEXT::default();
    // SAFETY: the device has external_memory_host enabled; the pointer is the start of a live
    // mapping, page-aligned by mmap, which the call only inspects.
    unsafe {
        (host_memory.fp().get_memory_host_pointer_properties_ext)(
            gpu.device.handle(),
            handle_type,
            pointer,
            &mut pointer_properties,
        )
    }
    .result()
    .map_err(|result| Failure::vulkan("read the host pointer's memory types", result))?;

    let mut external_buffer =
        vk::ExternalMemoryBufferCreateInfo::default().handle_types(handle_type);
    let buffer = scope.buffer(
        &vk::BufferCreateInfo::default()
            .size(pages.len() as u64)
            .usage(vk::BufferUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .push_next(&mut external_buffer),
    )?;
    // SAFETY: the buffer was just made on this device and has no memory yet.
    let requirements = unsafe { gpu.device.get_buffer_memory_requirements(buffer) };
    if requirements.size > pages.len() as u64 {
        return Err(Failure::other(
            "import the pages",
            format!(
                "the buffer needs {} bytes, the mapping has {}",
                requirements.size,
                pages.len()
            ),
        ));
    }
    let memory_type = gpu
        .memory_type(
            requirements.memory_type_bits & pointer_properties.memory_type_bits,
            vk::MemoryPropertyFlags::empty(),
            vk::MemoryPropertyFlags::empty(),
        )
        .ok_or_else(|| Failure::other("import the pages", "no memory type fits"))?;
    let mut import = vk::ImportMemoryHostPointerInfoEXT::default()
        .handle_type(handle_type)
        .host_pointer(pointer.cast_mut());
    let memory = scope.memory(
        &vk::MemoryAllocateInfo::default()
            .allocation_size(pages.len() as u64)
            .memory_type_index(memory_type)
            .push_next(&mut import),
        "import the pages",
    )?;
    // SAFETY: the memory imports the mapping, as large as the buffer needs, of a type it allows;
    // the caller keeps the mapping alive longer than the scope that frees the memory.
    unsafe { gpu.device.bind_buffer_memory(buffer, memory, 0) }
        .map_err(|result| Failure::vulkan("bind the pages", result))?;
    Ok(buffer)
}

/// When a client's memfd cannot be imported, whether anonymous memory of the same size can: it
/// tells a driver that refuses file-backed pages from one that refuses host memory at all.
fn compare_with_anonymous_memory(gpu: &Gpu<'_>, length: usize, failure: Failure) -> Failure {
    let comparison = match Mapping::anonymous(length) {
        Err(error) => format!("anonymous memory could not be mapped to compare: {error}"),
        Ok(anonymous) => {
            // Declared after `anonymous`, so dropped before it: the memory goes before the pages.
            let mut scope = Scope::new(gpu);
            match import_pages(gpu, &mut scope, &anonymous) {
                Ok(_) => "anonymous memory of the same size imports".to_owned(),
                Err(other) => format!("anonymous memory fails too, {}", other.describe()),
            }
        }
    };
    Failure::other(
        failure.step,
        format!("{} for the client's memfd; {comparison}", failure.error),
    )
}

pub fn to_json(results: &[ShmResult]) -> Json {
    Json::Array(
        results
            .iter()
            .map(|result| {
                Json::Object(vec![
                    ("width", Json::Integer(i64::from(result.width))),
                    ("height", Json::Integer(i64::from(result.height))),
                    ("uploads", Json::Integer(UPLOADS as i64)),
                    ("warm_up", Json::Integer(WARM_UP as i64)),
                    (
                        "min_imported_host_pointer_alignment",
                        Json::optional(result.host_pointer_alignment, |bytes| {
                            Json::Integer(bytes as i64)
                        }),
                    ),
                    ("staging", path_json(&result.staging)),
                    ("host_pointer", path_json(&result.host_pointer)),
                ])
            })
            .collect(),
    )
}

fn path_json(result: &PathResult) -> Json {
    match result {
        PathResult::Measured(figures) => Json::Object(vec![
            ("result", Json::text("measured")),
            ("import_us", Json::optional(figures.import_us, Json::Number)),
            ("cpu_us", summary_json(figures.cpu)),
            ("gpu_us", summary_json(figures.gpu)),
            ("wall_us", summary_json(figures.wall)),
            ("pixels", figures.pixels.to_json()),
        ]),
        PathResult::Failed(failure) => Check::Failed(failure.clone()).to_json(),
        PathResult::Skipped(reason) => Check::Skipped(reason.clone()).to_json(),
    }
}

pub fn to_markdown(results: &[ShmResult], out: &mut String) {
    out.push_str(&format!(
        "A client's ARGB8888 `memfd`, mapped, uploaded into a sampled `B8G8R8A8_UNORM` image, \
         {UPLOADS} times after {WARM_UP} of warm-up, waiting for each. Microseconds, median / 99th \
         percentile / longest. CPU: the copy into the staging buffer (staging only), recording and \
         submitting. GPU: timestamps around the copy. Wall: until the fence signals.\n\n"
    ));
    out.push_str("| Size | Path | Import, once | CPU | GPU | Wall | Pixels |\n|---|---|---|---|---|---|---|\n");
    for result in results {
        for (name, path) in [
            ("staging", &result.staging),
            ("host pointer", &result.host_pointer),
        ] {
            let size = format!("{}x{}", result.width, result.height);
            match path {
                PathResult::Measured(figures) => out.push_str(&format!(
                    "| {size} | {name} | {} | {} | {} | {} | {} |\n",
                    figures
                        .import_us
                        .map_or_else(|| "-".to_owned(), |us| format!("{us:.1}")),
                    summary_cell(figures.cpu),
                    summary_cell(figures.gpu),
                    summary_cell(figures.wall),
                    figures.pixels.to_cell(),
                )),
                PathResult::Failed(failure) => out.push_str(&format!(
                    "| {size} | {name} | {} | | | | |\n",
                    Check::Failed(failure.clone()).to_cell()
                )),
                PathResult::Skipped(reason) => out.push_str(&format!(
                    "| {size} | {name} | skipped: {reason} | | | | |\n"
                )),
            }
        }
    }
    if let Some(alignment) = results
        .first()
        .and_then(|result| result.host_pointer_alignment)
    {
        out.push_str(&format!(
            "\n`minImportedHostPointerAlignment`: {alignment} bytes.\n"
        ));
    }
    out.push('\n');
}
