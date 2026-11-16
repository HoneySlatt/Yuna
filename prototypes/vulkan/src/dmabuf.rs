//! A client's buffer shared as a dma-buf, as a Wayland client sends it: an image made and drawn
//! on one device, released to the foreign queue family, its memory exported as a file
//! descriptor, then imported into another device with its modifier and its planes' layouts.

use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};

use ash::vk;

use crate::gpu::{Failure, Gpu, Scope, image_barrier};
use crate::pattern::TestFormat;

/// How far a buffer went on its way from the client to the compositor.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// The driver lists the modifier for the format.
    Offered,
    /// An image was made with it and its memory bound.
    Created,
    /// Drawn, and its memory exported as a dma-buf.
    Exported,
    /// Imported into the second device with the modifier and its planes.
    Imported,
    /// Sampled on the second device, every texel as drawn.
    SampledCorrectly,
}

impl Stage {
    pub fn name(self) -> &'static str {
        match self {
            Self::Offered => "offered",
            Self::Created => "created",
            Self::Exported => "exported",
            Self::Imported => "imported",
            Self::SampledCorrectly => "sampled-correctly",
        }
    }
}

/// A dma-buf a client exported: what the compositor receives over `zwp_linux_dmabuf_v1`.
pub struct Exported {
    pub fd: OwnedFd,
    pub format: vk::Format,
    pub width: u32,
    pub height: u32,
    pub modifier: u64,
    /// Each memory plane's offset and row pitch, in bytes.
    pub planes: Vec<(u64, u64)>,
}

/// The pipeline that draws the test pattern into a client's image of one format.
pub struct PatternWriter {
    pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    format: TestFormat,
}

impl PatternWriter {
    pub fn new(scope: &mut Scope<'_>, format: TestFormat) -> Result<Self, Failure> {
        let vertex = scope.shader(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/pattern.vert.spv"
        )))?;
        let fragment = scope.shader(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/pattern.frag.spv"
        )))?;
        let layout = scope.pipeline_layout(
            &[],
            &[vk::PushConstantRange {
                stage_flags: vk::ShaderStageFlags::FRAGMENT,
                offset: 0,
                size: 8,
            }],
        )?;
        let pipeline = scope.graphics_pipeline(
            vertex,
            fragment,
            layout,
            format.format,
            vk::PrimitiveTopology::TRIANGLE_LIST,
            false,
        )?;
        Ok(Self {
            pipeline,
            layout,
            format,
        })
    }

    pub fn format(&self) -> TestFormat {
        self.format
    }
}

/// What the driver allows for a dma-buf image of `format` with `modifier` and `usage`.
pub fn dma_buf_features(
    gpu: &Gpu<'_>,
    format: vk::Format,
    modifier: u64,
    usage: vk::ImageUsageFlags,
) -> Result<(vk::ExternalMemoryFeatureFlags, vk::Extent3D), vk::Result> {
    let mut modifier_info = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default()
        .drm_format_modifier(modifier)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let mut external_info = vk::PhysicalDeviceExternalImageFormatInfo::default()
        .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let info = vk::PhysicalDeviceImageFormatInfo2::default()
        .format(format)
        .ty(vk::ImageType::TYPE_2D)
        .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
        .usage(usage)
        .push_next(&mut modifier_info)
        .push_next(&mut external_info);
    let mut external = vk::ExternalImageFormatProperties::default();
    let mut properties = vk::ImageFormatProperties2::default().push_next(&mut external);
    // SAFETY: the physical device comes from this instance, which has the modifier extension's
    // structures (the device offers it, checked by the caller); the chains live for the call.
    unsafe {
        gpu.vulkan
            .instance
            .get_physical_device_image_format_properties2(gpu.physical, &info, &mut properties)
    }?;
    let max_extent = properties.image_format_properties.max_extent;
    Ok((
        external.external_memory_properties.external_memory_features,
        max_extent,
    ))
}

/// The largest image of `format` with `modifier` that a client can draw and export and a
/// compositor can import and sample, or `None` when the driver refuses either.
pub fn largest_shared_image(
    gpu: &Gpu<'_>,
    format: vk::Format,
    modifier: u64,
) -> Option<(u32, u32)> {
    let (_, export) =
        dma_buf_features(gpu, format, modifier, vk::ImageUsageFlags::COLOR_ATTACHMENT).ok()?;
    let (_, import) = dma_buf_features(gpu, format, modifier, vk::ImageUsageFlags::SAMPLED).ok()?;
    Some((
        export.width.min(import.width),
        export.height.min(import.height),
    ))
}

fn check_dma_buf_support(
    gpu: &Gpu<'_>,
    format: vk::Format,
    modifier: u64,
    usage: vk::ImageUsageFlags,
    needed: vk::ExternalMemoryFeatureFlags,
    (width, height): (u32, u32),
    step: &str,
) -> Result<(), Failure> {
    let (features, max_extent) = dma_buf_features(gpu, format, modifier, usage)
        .map_err(|result| Failure::vulkan(step, result))?;
    if !features.contains(needed) {
        return Err(Failure::other(
            step,
            format!("the driver reports {features:?}, not {needed:?}"),
        ));
    }
    if max_extent.width < width || max_extent.height < height {
        return Err(Failure::other(
            step,
            format!("at most {}x{}", max_extent.width, max_extent.height),
        ));
    }
    Ok(())
}

/// The client's side: an image with one of `modifiers`, the driver's choice among them, drawn
/// with the pattern, released to the foreign family, its memory exported. On failure, the stage
/// it reached.
pub fn export_pattern(
    client: &Gpu<'_>,
    scope: &mut Scope<'_>,
    writer: &PatternWriter,
    modifiers: &[u64],
    (width, height): (u32, u32),
    opaque: bool,
) -> Result<Exported, (Stage, Failure)> {
    let at_offered = |failure| (Stage::Offered, failure);
    let at_created = |failure| (Stage::Created, failure);
    let (Some(modifier_fns), Some(memory_fd)) = (&client.modifiers, &client.memory_fd) else {
        return Err(at_offered(Failure::other(
            "export",
            "the device has no image_drm_format_modifier or external_memory_fd",
        )));
    };
    let format = writer.format.format;
    let usage = vk::ImageUsageFlags::COLOR_ATTACHMENT;
    // The driver chooses among the modifiers that can be exported at this size: some allow
    // smaller images than others.
    let mut usable = Vec::new();
    let mut first_refusal = None;
    for &modifier in modifiers {
        match check_dma_buf_support(
            client,
            format,
            modifier,
            usage,
            vk::ExternalMemoryFeatureFlags::EXPORTABLE,
            (width, height),
            "check the image can be exported",
        ) {
            Ok(()) => usable.push(modifier),
            Err(failure) => {
                first_refusal.get_or_insert(failure);
            }
        }
    }
    if usable.is_empty() {
        return Err(at_offered(first_refusal.unwrap_or_else(|| {
            Failure::other("check the image can be exported", "no modifier was given")
        })));
    }

    let mut modifier_list =
        vk::ImageDrmFormatModifierListCreateInfoEXT::default().drm_format_modifiers(&usable);
    let mut external = vk::ExternalMemoryImageCreateInfo::default()
        .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let image = scope
        .image(
            &image_info(format, width, height, usage)
                .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
                .push_next(&mut modifier_list)
                .push_next(&mut external),
        )
        .map_err(at_offered)?;
    let mut export = vk::ExportMemoryAllocateInfo::default()
        .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let memory =
        allocate_dedicated(client, scope, image, u32::MAX, &mut export).map_err(at_offered)?;
    bind_image(client, image, memory).map_err(at_offered)?;

    let mut chosen = vk::ImageDrmFormatModifierPropertiesEXT::default();
    // SAFETY: the image was made with DRM_FORMAT_MODIFIER_EXT tiling on this device, which has
    // the extension enabled.
    unsafe { modifier_fns.get_image_drm_format_modifier_properties(image, &mut chosen) }
        .map_err(|result| at_created(Failure::vulkan("read the image's modifier", result)))?;
    let modifier = chosen.drm_format_modifier;
    let plane_count = plane_count(client, format, modifier).map_err(at_created)?;
    let planes = (0..plane_count)
        .map(|plane| {
            let subresource = vk::ImageSubresource {
                aspect_mask: memory_plane(plane),
                mip_level: 0,
                array_layer: 0,
            };
            // SAFETY: the image has a modifier with `plane_count` memory planes, and the aspect
            // names one of them.
            let layout = unsafe {
                client
                    .device
                    .get_image_subresource_layout(image, subresource)
            };
            (layout.offset, layout.row_pitch)
        })
        .collect();

    let view = scope
        .image_view(&view_info(image, format))
        .map_err(at_created)?;
    let mut constants = [0u8; 8];
    constants[..4].copy_from_slice(&writer.format.channels.shader_kind().to_ne_bytes());
    constants[4..].copy_from_slice(&u32::from(opaque).to_ne_bytes());
    client
        .run_once(scope, "draw the pattern", |commands| {
            commands.barriers(
                &[image_barrier(
                    image,
                    (
                        vk::ImageLayout::UNDEFINED,
                        vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                    ),
                    (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
                    (
                        vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                        vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                    ),
                )],
                &[],
            );
            commands.begin_rendering(view, width, height, None);
            commands.bind_pipeline(vk::PipelineBindPoint::GRAPHICS, writer.pipeline);
            commands.push_constants(writer.layout, vk::ShaderStageFlags::FRAGMENT, &constants);
            commands.draw(3);
            commands.end_rendering();
            // The release to the foreign family, as a client gives its buffer away.
            commands.barriers(
                &[image_barrier(
                    image,
                    (
                        vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                        vk::ImageLayout::GENERAL,
                    ),
                    (
                        vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                        vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                    ),
                    (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
                )
                .src_queue_family_index(client.queue_family)
                .dst_queue_family_index(client.foreign_family)],
                &[],
            );
        })
        .map_err(at_created)?;

    let fd_info = vk::MemoryGetFdInfoKHR::default()
        .memory(memory)
        .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    // SAFETY: the memory was allocated exportable as a dma-buf on this device, which has
    // external_memory_fd; the new descriptor is ours.
    let fd = unsafe { memory_fd.get_memory_fd(&fd_info) }
        .map_err(|result| at_created(Failure::vulkan("export the memory as a dma-buf", result)))?;
    // SAFETY: vkGetMemoryFdKHR returned a new descriptor that nothing else owns.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    Ok(Exported {
        fd,
        format,
        width,
        height,
        modifier,
        planes,
    })
}

/// The compositor's side: imports a client's dma-buf as an image with its modifier and planes,
/// for `usage`. It stays in the foreign family until acquired; its contents are the client's.
pub fn import(
    compositor: &Gpu<'_>,
    scope: &mut Scope<'_>,
    exported: Exported,
    usage: vk::ImageUsageFlags,
) -> Result<vk::Image, Failure> {
    let Some(memory_fd) = &compositor.memory_fd else {
        return Err(Failure::other(
            "import",
            "the device has no external_memory_fd",
        ));
    };
    check_dma_buf_support(
        compositor,
        exported.format,
        exported.modifier,
        usage,
        vk::ExternalMemoryFeatureFlags::IMPORTABLE,
        (exported.width, exported.height),
        "check the image can be imported",
    )?;
    let mut fd_properties = vk::MemoryFdPropertiesKHR::default();
    // SAFETY: the descriptor is an open dma-buf; the call only reads it.
    unsafe {
        memory_fd.get_memory_fd_properties(
            vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
            exported.fd.as_raw_fd(),
            &mut fd_properties,
        )
    }
    .map_err(|result| Failure::vulkan("read the dma-buf's memory types", result))?;

    let plane_layouts: Vec<vk::SubresourceLayout> = exported
        .planes
        .iter()
        .map(|&(offset, row_pitch)| vk::SubresourceLayout {
            offset,
            size: 0,
            row_pitch,
            array_pitch: 0,
            depth_pitch: 0,
        })
        .collect();
    let mut explicit = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
        .drm_format_modifier(exported.modifier)
        .plane_layouts(&plane_layouts);
    let mut external = vk::ExternalMemoryImageCreateInfo::default()
        .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let image = scope.image(
        &image_info(exported.format, exported.width, exported.height, usage)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .push_next(&mut explicit)
            .push_next(&mut external),
    )?;
    let mut import = vk::ImportMemoryFdInfoKHR::default()
        .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
        .fd(exported.fd.as_raw_fd());
    let memory = allocate_dedicated(
        compositor,
        scope,
        image,
        fd_properties.memory_type_bits,
        &mut import,
    )?;
    // The allocation succeeded, so it owns the descriptor and closes it with the memory: given
    // up here, before anything else can fail, no path closes it a second time. Had it failed,
    // the descriptor would still be ours, closed when `exported` drops.
    let _ = exported.fd.into_raw_fd();
    bind_image(compositor, image, memory)?;
    Ok(image)
}

/// Allocates dedicated memory for `image`, of a type both it and `allowed` take, with `extra`
/// (an export or an import) chained. The caller binds it.
fn allocate_dedicated<T: vk::ExtendsMemoryAllocateInfo>(
    gpu: &Gpu<'_>,
    scope: &mut Scope<'_>,
    image: vk::Image,
    allowed: u32,
    extra: &mut T,
) -> Result<vk::DeviceMemory, Failure> {
    // SAFETY: the image was just made on this device and has no memory yet.
    let requirements = unsafe { gpu.device.get_image_memory_requirements(image) };
    let memory_type = gpu
        .memory_type(
            requirements.memory_type_bits & allowed,
            vk::MemoryPropertyFlags::empty(),
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
        .ok_or_else(|| Failure::other("allocate the image's memory", "no memory type fits"))?;
    let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
    scope.memory(
        &vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type)
            .push_next(&mut dedicated)
            .push_next(extra),
        "allocate the image's memory",
    )
}

/// Binds `memory`, which `allocate_dedicated` made for `image`, to it.
fn bind_image(gpu: &Gpu<'_>, image: vk::Image, memory: vk::DeviceMemory) -> Result<(), Failure> {
    // SAFETY: the memory is dedicated to this image, of a type it allows, as large as it needs,
    // and bound to nothing yet.
    unsafe { gpu.device.bind_image_memory(image, memory, 0) }
        .map_err(|result| Failure::vulkan("bind the image's memory", result))
}

/// The number of memory planes `modifier` has for `format`, as the driver lists it.
fn plane_count(gpu: &Gpu<'_>, format: vk::Format, modifier: u64) -> Result<u32, Failure> {
    list_modifiers(gpu, format)
        .iter()
        .find(|listed| listed.drm_format_modifier == modifier)
        .map(|listed| listed.drm_format_modifier_plane_count)
        .ok_or_else(|| {
            Failure::other(
                "read the modifier's planes",
                "the driver chose an unlisted modifier",
            )
        })
}

/// Every modifier the driver lists for `format`, with its planes and features.
pub fn list_modifiers(
    gpu: &Gpu<'_>,
    format: vk::Format,
) -> Vec<vk::DrmFormatModifierProperties2EXT> {
    let mut list = vk::DrmFormatModifierPropertiesList2EXT::default();
    let mut properties = vk::FormatProperties2::default().push_next(&mut list);
    // SAFETY: the physical device comes from this instance and offers the modifier extension
    // (the callers check); the first call only counts.
    unsafe {
        gpu.vulkan.instance.get_physical_device_format_properties2(
            gpu.physical,
            format,
            &mut properties,
        )
    };
    let mut modifiers = vec![
        vk::DrmFormatModifierProperties2EXT::default();
        list.drm_format_modifier_count as usize
    ];
    let mut list = vk::DrmFormatModifierPropertiesList2EXT::default()
        .drm_format_modifier_properties(&mut modifiers);
    let mut properties = vk::FormatProperties2::default().push_next(&mut list);
    // SAFETY: as above; the list points at room for as many modifiers as the first call counted.
    unsafe {
        gpu.vulkan.instance.get_physical_device_format_properties2(
            gpu.physical,
            format,
            &mut properties,
        )
    };
    let written = list.drm_format_modifier_count as usize;
    modifiers.truncate(written);
    modifiers
}

pub fn image_info(
    format: vk::Format,
    width: u32,
    height: u32,
    usage: vk::ImageUsageFlags,
) -> vk::ImageCreateInfo<'static> {
    vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
}

pub fn view_info(image: vk::Image, format: vk::Format) -> vk::ImageViewCreateInfo<'static> {
    vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(format)
        .subresource_range(crate::gpu::COLOUR)
}

fn memory_plane(plane: u32) -> vk::ImageAspectFlags {
    match plane {
        0 => vk::ImageAspectFlags::MEMORY_PLANE_0_EXT,
        1 => vk::ImageAspectFlags::MEMORY_PLANE_1_EXT,
        2 => vk::ImageAspectFlags::MEMORY_PLANE_2_EXT,
        _ => vk::ImageAspectFlags::MEMORY_PLANE_3_EXT,
    }
}

/// The modifier's name: its vendor, and for AMD its tiling and compression, decoded from the
/// fields of drm_fourcc.h.
pub fn modifier_name(modifier: u64) -> String {
    match modifier {
        0 => return "LINEAR".to_owned(),
        0x00ff_ffff_ffff_ffff => return "INVALID".to_owned(),
        _ => {}
    }
    let vendor = modifier >> 56;
    if vendor != 0x02 {
        let name = match vendor {
            0x01 => "Intel",
            0x03 => "NVIDIA",
            0x08 => "ARM",
            _ => "vendor",
        };
        return format!("{name} {modifier:#x}");
    }
    let field = |shift: u32, bits: u32| (modifier >> shift) & ((1 << bits) - 1);
    let version = match field(0, 8) {
        1 => "GFX9",
        2 => "GFX10",
        3 => "GFX10_RBPLUS",
        4 => "GFX11",
        5 => "GFX12",
        _ => "GFX?",
    };
    // The tile field holds AMD's swizzle mode; GFX12 numbers its own.
    let tile = match (field(0, 8), field(8, 5)) {
        (5, 1) => "256B_2D",
        (5, 2) => "4K_2D",
        (5, 3) => "64K_2D",
        (5, 4) => "256K_2D",
        (_, 9) => "64K_S",
        (_, 10) => "64K_D",
        (_, 22) => "4K_D_X",
        (_, 25) => "64K_S_X",
        (_, 26) => "64K_D_X",
        (_, 27) => "64K_R_X",
        (_, 31) => "256K_R_X",
        _ => "tile?",
    };
    let mut name = format!("AMD {version} {tile}");
    if field(13, 1) == 1 {
        name.push_str(" DCC");
        if field(14, 1) == 1 {
            name.push_str(" RETILE");
        }
        if field(16, 1) == 1 {
            name.push_str(" I64B");
        }
        if field(17, 1) == 1 {
            name.push_str(" I128B");
        }
        name.push_str(match field(18, 2) {
            0 => " MAX64B",
            1 => " MAX128B",
            2 => " MAX256B",
            _ => "",
        });
    }
    name
}

#[cfg(test)]
mod tests {
    use super::modifier_name;

    #[test]
    fn names_amd_modifiers_and_others() {
        assert_eq!(modifier_name(0), "LINEAR");
        // AMD_FMT_MOD | TILE_VERSION(GFX10_RBPLUS) | TILE(GFX9_64K_R_X), no compression.
        assert_eq!(
            modifier_name((2 << 56) | (27 << 8) | 3),
            "AMD GFX10_RBPLUS 64K_R_X"
        );
        // The same with DCC, independent 64- and 128-byte blocks, at most 128-byte blocks.
        let dcc = (2u64 << 56) | (27 << 8) | 3 | (1 << 13) | (1 << 16) | (1 << 17) | (1 << 18);
        assert_eq!(
            modifier_name(dcc),
            "AMD GFX10_RBPLUS 64K_R_X DCC I64B I128B MAX128B"
        );
        // One RADV lists on an RX 6900 XT: 4K_D_X, with pipe XOR bits.
        assert_eq!(
            modifier_name(0x0200_0000_0080_1603),
            "AMD GFX10_RBPLUS 4K_D_X"
        );
        assert_eq!(
            modifier_name(0x0100_0000_0000_0001),
            "Intel 0x100000000000001"
        );
    }
}
