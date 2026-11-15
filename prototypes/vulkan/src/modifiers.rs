//! Part 2: for each format and each modifier the driver lists, an image of 1920x1080 drawn on a
//! client's device, exported as a dma-buf, imported into the compositor's device with its
//! modifier and planes, and sampled once into a buffer the CPU checks texel by texel.

use ash::vk;

use crate::dmabuf::{self, PatternWriter, Stage};
use crate::gpu::{Failure, Gpu, Scope, buffer_to_host, image_barrier};
use crate::json::Json;
use crate::pattern::{self, TEST_FORMATS, TestFormat};

pub const WIDTH: u32 = 1920;
pub const HEIGHT: u32 = 1080;

pub struct ModifierResult {
    pub modifier: u64,
    pub plane_count: u32,
    pub features: vk::FormatFeatureFlags2,
    /// The furthest stage the buffer reached.
    pub reached: Stage,
    /// The largest image that makes the whole trip, as the driver reports it.
    pub largest: Option<(u32, u32)>,
    /// Why the next stage failed, when one did.
    pub failure: Option<Failure>,
    /// Why the modifier was not tried, when it was not.
    pub not_tried: Option<&'static str>,
    pub planes: Vec<(u64, u64)>,
}

pub struct FormatResult {
    pub format: TestFormat,
    pub modifiers: Vec<ModifierResult>,
}

/// The objects every import test on the compositor shares: the sampler and the pipeline that
/// samples an image into a buffer.
struct Sampler {
    sampler: vk::Sampler,
    set_layout: vk::DescriptorSetLayout,
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
}

impl Sampler {
    fn new(scope: &mut Scope<'_>) -> Result<Self, Failure> {
        let sampler = scope.sampler(
            &vk::SamplerCreateInfo::default()
                .mag_filter(vk::Filter::NEAREST)
                .min_filter(vk::Filter::NEAREST)
                .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
        )?;
        let set_layout = scope.descriptor_set_layout(&[
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
        ])?;
        let layout = scope.pipeline_layout(
            &[set_layout],
            &[vk::PushConstantRange {
                stage_flags: vk::ShaderStageFlags::COMPUTE,
                offset: 0,
                size: 8,
            }],
        )?;
        let module = scope.shader(include_bytes!(concat!(env!("OUT_DIR"), "/sample.comp.spv")))?;
        let pipeline = scope.compute_pipeline(module, layout)?;
        Ok(Self {
            sampler,
            set_layout,
            layout,
            pipeline,
        })
    }
}

/// Runs part 2 on every format. The devices must have the dma-buf and modifier extensions.
pub fn run(client: &Gpu<'_>, compositor: &Gpu<'_>) -> Result<Vec<FormatResult>, Failure> {
    let mut shared = Scope::new(compositor);
    let sampler = Sampler::new(&mut shared)?;
    let mut results = Vec::new();
    for format in TEST_FORMATS {
        let mut writer_scope = Scope::new(client);
        let writer = PatternWriter::new(&mut writer_scope, format);
        let listed = dmabuf::list_modifiers(client, format.format);
        let mut modifiers = Vec::new();
        for (index, entry) in listed.iter().enumerate() {
            eprintln!(
                "vulkan-prototype: modifiers: {} {} of {}: {}",
                format.name,
                index + 1,
                listed.len(),
                dmabuf::modifier_name(entry.drm_format_modifier)
            );
            let mut result = ModifierResult {
                modifier: entry.drm_format_modifier,
                plane_count: entry.drm_format_modifier_plane_count,
                features: entry.drm_format_modifier_tiling_features,
                reached: Stage::Offered,
                largest: None,
                failure: None,
                not_tried: None,
                planes: Vec::new(),
            };
            let features = entry.drm_format_modifier_tiling_features;
            if !features.contains(vk::FormatFeatureFlags2::SAMPLED_IMAGE) {
                result.not_tried = Some("cannot be sampled");
            } else if !features.contains(vk::FormatFeatureFlags2::COLOR_ATTACHMENT) {
                result.not_tried =
                    Some("cannot be drawn into, and the prototype writes by drawing");
            } else {
                result.largest =
                    dmabuf::largest_shared_image(client, format.format, entry.drm_format_modifier);
                match &writer {
                    Ok(writer) => try_modifier(client, compositor, &sampler, writer, &mut result),
                    Err(failure) => result.failure = Some(failure.clone()),
                }
            }
            modifiers.push(result);
        }
        results.push(FormatResult { format, modifiers });
    }
    Ok(results)
}

fn try_modifier(
    client: &Gpu<'_>,
    compositor: &Gpu<'_>,
    sampler: &Sampler,
    writer: &PatternWriter,
    result: &mut ModifierResult,
) {
    let mut client_scope = Scope::new(client);
    let exported = match dmabuf::export_pattern(
        client,
        &mut client_scope,
        writer,
        &[result.modifier],
        (WIDTH, HEIGHT),
        false,
    ) {
        Ok(exported) => exported,
        Err((reached, failure)) => {
            result.reached = reached;
            result.failure = Some(failure);
            return;
        }
    };
    result.reached = Stage::Exported;
    result.planes.clone_from(&exported.planes);

    let mut scope = Scope::new(compositor);
    let image = match dmabuf::import(
        compositor,
        &mut scope,
        exported,
        vk::ImageUsageFlags::SAMPLED,
    ) {
        Ok(image) => image,
        Err(failure) => {
            result.failure = Some(failure);
            return;
        }
    };
    result.reached = Stage::Imported;
    match sample_and_check(compositor, &mut scope, sampler, writer.format(), image) {
        Ok(()) => result.reached = Stage::SampledCorrectly,
        Err(failure) => result.failure = Some(failure),
    }
}

/// Acquires the imported image from the foreign family, samples every texel into a buffer and
/// compares them with the pattern.
fn sample_and_check(
    compositor: &Gpu<'_>,
    scope: &mut Scope<'_>,
    sampler: &Sampler,
    format: TestFormat,
    image: vk::Image,
) -> Result<(), Failure> {
    let view = scope.image_view(&dmabuf::view_info(image, format.format))?;
    let texel_bytes = u64::from(WIDTH) * u64::from(HEIGHT) * 16;
    let (buffer, mapped) = scope.bound_buffer(
        compositor,
        texel_bytes,
        vk::BufferUsageFlags::STORAGE_BUFFER,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        vk::MemoryPropertyFlags::HOST_CACHED,
    )?;
    let mapped = mapped.ok_or_else(|| Failure::other("map the texels", "not mapped"))?;
    let set = scope.descriptor_set(
        sampler.set_layout,
        &[
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                descriptor_count: 1,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_BUFFER,
                descriptor_count: 1,
            },
        ],
    )?;
    let image_info = [vk::DescriptorImageInfo {
        sampler: sampler.sampler,
        image_view: view,
        image_layout: vk::ImageLayout::GENERAL,
    }];
    let buffer_info = [vk::DescriptorBufferInfo {
        buffer,
        offset: 0,
        range: vk::WHOLE_SIZE,
    }];
    let writes = [
        vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&image_info),
        vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(1)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(&buffer_info),
    ];
    // SAFETY: the set, view, sampler and buffer come from this device and are not in use by
    // the GPU: the set is new.
    unsafe { compositor.device.update_descriptor_sets(&writes, &[]) };

    let mut size = [0u8; 8];
    size[..4].copy_from_slice(&WIDTH.to_ne_bytes());
    size[4..].copy_from_slice(&HEIGHT.to_ne_bytes());
    compositor.run_once(scope, "sample the imported image", |commands| {
        commands.barriers(
            &[acquire(
                compositor,
                image,
                vk::PipelineStageFlags2::COMPUTE_SHADER,
            )],
            &[],
        );
        commands.bind_pipeline(vk::PipelineBindPoint::COMPUTE, sampler.pipeline);
        commands.bind_set(vk::PipelineBindPoint::COMPUTE, sampler.layout, set);
        commands.push_constants(sampler.layout, vk::ShaderStageFlags::COMPUTE, &size);
        commands.dispatch(WIDTH.div_ceil(16), HEIGHT.div_ceil(16));
        commands.barriers(
            &[],
            &[buffer_to_host(
                buffer,
                vk::PipelineStageFlags2::COMPUTE_SHADER,
                vk::AccessFlags2::SHADER_STORAGE_WRITE,
            )],
        );
    })?;

    let texel_count = (WIDTH * HEIGHT) as usize;
    // SAFETY: the mapping is host-coherent, holds `texel_count` vec4s of f32 written by the
    // compute shader that the fence just waited for, and is aligned to at least
    // minMemoryMapAlignment (64 bytes); the scope keeps it mapped while the slice lives.
    let texels = unsafe { std::slice::from_raw_parts(mapped.cast::<[f32; 4]>(), texel_count) };
    match pattern::check(format.channels, WIDTH, texels) {
        None => Ok(()),
        Some(mismatch) => Err(Failure::other(
            "check the sampled texels",
            format!(
                "{} of {} texels differ; the first at ({}, {}): expected {:?}, sampled {:?}",
                mismatch.count,
                texel_count,
                mismatch.x,
                mismatch.y,
                mismatch.expected,
                mismatch.found
            ),
        )),
    }
}

/// The barrier that acquires an imported image from the foreign family, in GENERAL, for
/// sampling at `stage`.
pub fn acquire(
    compositor: &Gpu<'_>,
    image: vk::Image,
    stage: vk::PipelineStageFlags2,
) -> vk::ImageMemoryBarrier2<'static> {
    image_barrier(
        image,
        (vk::ImageLayout::GENERAL, vk::ImageLayout::GENERAL),
        (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
        (stage, vk::AccessFlags2::SHADER_SAMPLED_READ),
    )
    .src_queue_family_index(compositor.foreign_family)
    .dst_queue_family_index(compositor.queue_family)
}

/// The barrier that releases an imported image back to the foreign family once sampled.
pub fn release(
    compositor: &Gpu<'_>,
    image: vk::Image,
    stage: vk::PipelineStageFlags2,
) -> vk::ImageMemoryBarrier2<'static> {
    image_barrier(
        image,
        (vk::ImageLayout::GENERAL, vk::ImageLayout::GENERAL),
        (stage, vk::AccessFlags2::SHADER_SAMPLED_READ),
        (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
    )
    .src_queue_family_index(compositor.queue_family)
    .dst_queue_family_index(compositor.foreign_family)
}

/// The modifiers of `format` that reached the end: those a compositor can take.
pub fn working_modifiers(results: &[FormatResult], format: vk::Format) -> Vec<u64> {
    results
        .iter()
        .filter(|result| result.format.format == format)
        .flat_map(|result| &result.modifiers)
        .filter(|modifier| modifier.reached == Stage::SampledCorrectly)
        .map(|modifier| modifier.modifier)
        .collect()
}

pub fn features_names(features: vk::FormatFeatureFlags2) -> Vec<String> {
    format!("{features:?}")
        .split(" | ")
        .map(str::to_owned)
        .filter(|name| !name.is_empty())
        .collect()
}

/// The features a compositor's import reads, abbreviated for a table: Sampled, Linear filter,
/// Colour attachment, Blend, sTorage, transfer Src and Dst, disJoint.
fn features_cell(features: vk::FormatFeatureFlags2) -> String {
    use vk::FormatFeatureFlags2 as F;
    [
        (F::SAMPLED_IMAGE, 'S'),
        (F::SAMPLED_IMAGE_FILTER_LINEAR, 'L'),
        (F::COLOR_ATTACHMENT, 'C'),
        (F::COLOR_ATTACHMENT_BLEND, 'B'),
        (F::STORAGE_IMAGE, 'T'),
        (F::TRANSFER_SRC, 's'),
        (F::TRANSFER_DST, 'd'),
        (F::DISJOINT, 'J'),
    ]
    .iter()
    .map(|&(flag, letter)| if features.contains(flag) { letter } else { '-' })
    .collect()
}

pub fn to_json(results: &[FormatResult]) -> Json {
    Json::Array(
        results
            .iter()
            .map(|result| {
                Json::Object(vec![
                    ("format", Json::text(result.format.name)),
                    ("drm_formats", Json::text(result.format.drm_formats)),
                    (
                        "modifiers",
                        Json::Array(result.modifiers.iter().map(modifier_json).collect()),
                    ),
                ])
            })
            .collect(),
    )
}

fn modifier_json(result: &ModifierResult) -> Json {
    Json::Object(vec![
        ("modifier", Json::text(format!("{:#018x}", result.modifier))),
        ("name", Json::text(dmabuf::modifier_name(result.modifier))),
        ("plane_count", Json::Integer(i64::from(result.plane_count))),
        (
            "features",
            Json::Array(
                features_names(result.features)
                    .into_iter()
                    .map(Json::Text)
                    .collect(),
            ),
        ),
        ("reached", Json::text(result.reached.name())),
        (
            "largest_shared_image",
            Json::optional(result.largest, |(width, height)| {
                Json::text(format!("{width}x{height}"))
            }),
        ),
        ("not_tried", Json::optional(result.not_tried, Json::text)),
        (
            "failure",
            Json::optional(result.failure.as_ref(), |failure| {
                Json::Object(vec![
                    ("step", Json::text(&failure.step)),
                    ("error", Json::text(&failure.error)),
                ])
            }),
        ),
        (
            "planes",
            Json::Array(
                result
                    .planes
                    .iter()
                    .map(|&(offset, row_pitch)| {
                        Json::Object(vec![
                            ("offset", Json::Integer(offset as i64)),
                            ("row_pitch", Json::Integer(row_pitch as i64)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

pub fn to_markdown(results: &[FormatResult], out: &mut String) {
    out.push_str(
        "Each modifier the driver lists, drawn at 1920x1080 on one device, exported as a dma-buf, \
         imported into a second device and sampled there. Features: S sampled, L linear filter, \
         C colour attachment, B blend, T storage, s and d transfer source and destination, \
         J disjoint. Largest: the largest image the driver lets a client draw and export and a \
         compositor import and sample with it.\n\n",
    );
    for result in results {
        let reached = result
            .modifiers
            .iter()
            .filter(|modifier| modifier.reached == Stage::SampledCorrectly)
            .count();
        out.push_str(&format!(
            "#### {} ({}): {} of {} modifiers sampled correctly\n\n",
            result.format.name,
            result.format.drm_formats,
            reached,
            result.modifiers.len()
        ));
        if result.modifiers.is_empty() {
            out.push_str("No modifier listed.\n\n");
            continue;
        }
        out.push_str("| Modifier | Name | Planes | Features | Largest | Result |\n|---|---|---|---|---|---|\n");
        for modifier in &result.modifiers {
            let outcome = match (&modifier.failure, modifier.not_tried) {
                (Some(failure), _) => {
                    format!("{}, then {}", modifier.reached.name(), failure.describe())
                }
                (None, Some(reason)) => format!("offered; not tried: {reason}"),
                (None, None) => modifier.reached.name().to_owned(),
            };
            let largest = modifier.largest.map_or_else(
                || "-".to_owned(),
                |(width, height)| format!("{width}x{height}"),
            );
            out.push_str(&format!(
                "| `{:#018x}` | {} | {} | `{}` | {largest} | {} |\n",
                modifier.modifier,
                dmabuf::modifier_name(modifier.modifier),
                modifier.plane_count,
                features_cell(modifier.features),
                outcome.replace('|', "\\|")
            ));
        }
        out.push('\n');
    }
}
