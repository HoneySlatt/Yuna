//! Part 3: explicit synchronisation between a client's device and a compositor's, through sync
//! files. A semaphore and a fence, each signalled behind work that keeps the client's GPU busy,
//! exported as sync files while that work still runs, imported into the compositor's device and
//! waited on there; a timeline semaphore signalled and waited, its wait submitted before its
//! signal.

use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};

use ash::vk;

use crate::gpu::{BoundBuffer, Failure, GPU_TIMEOUT, Gpu, Recorder, Scope};
use crate::json::Json;
use crate::outcome::Check;
use crate::sys;

pub struct SyncResult {
    pub semaphore_sync_file: Check,
    pub fence_sync_file: Check,
    pub timeline: Check,
}

pub fn run(client: &Gpu<'_>, compositor: &Gpu<'_>) -> SyncResult {
    let semaphore_sync_file = match (&client.semaphore_fd, &compositor.semaphore_fd) {
        (Some(_), Some(_)) => Check::from_result(semaphore_through_sync_file(client, compositor)),
        _ => Check::Skipped("VK_KHR_external_semaphore_fd is not offered".to_owned()),
    };
    let fence_sync_file = match (&client.fence_fd, &compositor.fence_fd) {
        (Some(_), Some(_)) => Check::from_result(fence_through_sync_file(client, compositor)),
        _ => Check::Skipped("VK_KHR_external_fence_fd is not offered".to_owned()),
    };
    let timeline = Check::from_result(timeline_semaphore(compositor));
    SyncResult {
        semaphore_sync_file,
        fence_sync_file,
        timeline,
    }
}

/// A descriptor Vulkan returned, or `None` for -1, which a sync file export may return for a
/// payload already signalled.
fn sync_file(fd: i32) -> Option<OwnedFd> {
    // SAFETY: a descriptor other than -1 that Vulkan exported is new and ours alone.
    (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Whether a sync file has signalled, without waiting; -1 stands for one already signalled.
fn signalled_now(fd: &Option<OwnedFd>, step: &str) -> Result<bool, Failure> {
    let Some(fd) = fd else {
        return Ok(true);
    };
    sys::sync_file_signals(fd, 0).map_err(|error| Failure::other(step, error.to_string()))
}

/// A second descriptor of the sync file, to poll it after the import took the first.
fn watch(fd: &Option<OwnedFd>, step: &str) -> Result<Option<OwnedFd>, Failure> {
    fd.as_ref()
        .map(OwnedFd::try_clone)
        .transpose()
        .map_err(|error| Failure::other(step, error.to_string()))
}

/// Checks that the second device's wait really waited: once it ended, the client's work behind
/// the sync file must be done. Says whether the payload was still pending when imported, and
/// whether the driver exported -1, its sign for a payload already signalled.
fn waited_for_the_client(
    pending_at_import: bool,
    watched: &Option<OwnedFd>,
    step: &str,
) -> Result<&'static str, Failure> {
    if !signalled_now(watched, step)? {
        return Err(Failure::other(
            step,
            "the second device's wait ended before the client's work did",
        ));
    }
    Ok(match (pending_at_import, watched) {
        (true, _) => {
            "imported while the client's work still ran; the second device waited until it ended"
        }
        (false, Some(_)) => "the client's work had ended before the import; imported and waited",
        (false, None) => {
            "the driver exported -1, already signalled, its work done; imported and waited"
        }
    })
}

/// The number of iterations busy.comp runs: tens of milliseconds on the GPUs measured, well
/// under the kernel's hang timeout on the slowest.
const BUSY_ITERATIONS: u32 = 1 << 22;

/// The client's work that a sync file is exported behind: busy.comp, on one invocation.
struct Busy {
    pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    set: vk::DescriptorSet,
}

impl Busy {
    fn new(client: &Gpu<'_>, scope: &mut Scope<'_>) -> Result<Self, Failure> {
        let set_layout =
            scope.descriptor_set_layout(&[vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE)])?;
        let layout = scope.pipeline_layout(
            &[set_layout],
            &[vk::PushConstantRange {
                stage_flags: vk::ShaderStageFlags::COMPUTE,
                offset: 0,
                size: 4,
            }],
        )?;
        let module = scope.shader(include_bytes!(concat!(env!("OUT_DIR"), "/busy.comp.spv")))?;
        let pipeline = scope.compute_pipeline(module, layout)?;
        let BoundBuffer { buffer, .. } = scope.bound_buffer(
            client,
            4,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            vk::MemoryPropertyFlags::empty(),
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )?;
        let set = scope.descriptor_set(
            set_layout,
            &[vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_BUFFER,
                descriptor_count: 1,
            }],
        )?;
        let buffer_info = [vk::DescriptorBufferInfo {
            buffer,
            offset: 0,
            range: vk::WHOLE_SIZE,
        }];
        let write = [vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(&buffer_info)];
        // SAFETY: the set and the buffer come from this device, and the set is new, so no
        // submitted work uses it.
        unsafe { client.device.update_descriptor_sets(&write, &[]) };
        Ok(Self {
            pipeline,
            layout,
            set,
        })
    }

    fn record(&self, commands: &Recorder<'_>) {
        commands.bind_pipeline(vk::PipelineBindPoint::COMPUTE, self.pipeline);
        commands.bind_set(vk::PipelineBindPoint::COMPUTE, self.layout, self.set);
        commands.push_constants(
            self.layout,
            vk::ShaderStageFlags::COMPUTE,
            &BUSY_ITERATIONS.to_ne_bytes(),
        );
        commands.dispatch(1, 1);
    }

    /// Submits the busy work on the client's device, signalling the semaphores and fence given
    /// once it ends.
    fn submit(
        &self,
        client: &Gpu<'_>,
        scope: &mut Scope<'_>,
        signals: &[vk::SemaphoreSubmitInfo<'_>],
        fence: vk::Fence,
    ) -> Result<(), Failure> {
        let commands = scope.command_buffer(client)?;
        client.record(commands, "keep the client busy", |commands| {
            self.record(commands)
        })?;
        client.submit("keep the client busy", commands, &[], signals, fence)
    }
}

/// Submits an empty batch of commands on `gpu`, waiting on and signalling the semaphores given.
fn submit_empty(
    gpu: &Gpu<'_>,
    scope: &mut Scope<'_>,
    step: &str,
    waits: &[vk::SemaphoreSubmitInfo<'_>],
    signals: &[vk::SemaphoreSubmitInfo<'_>],
    fence: vk::Fence,
) -> Result<(), Failure> {
    let commands = scope.command_buffer(gpu)?;
    gpu.record(commands, step, |_| {})?;
    gpu.submit(step, commands, waits, signals, fence)
}

fn semaphore_through_sync_file(client: &Gpu<'_>, compositor: &Gpu<'_>) -> Result<String, Failure> {
    let (Some(client_fd), Some(compositor_fd)) = (&client.semaphore_fd, &compositor.semaphore_fd)
    else {
        return Err(Failure::other(
            "check",
            "external_semaphore_fd is not enabled",
        ));
    };
    let info = vk::PhysicalDeviceExternalSemaphoreInfo::default()
        .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
    let mut properties = vk::ExternalSemaphoreProperties::default();
    // SAFETY: the physical device comes from this instance; the structures live for the call.
    unsafe {
        client
            .vulkan
            .instance
            .get_physical_device_external_semaphore_properties(
                client.physical,
                &info,
                &mut properties,
            )
    };
    let needed = vk::ExternalSemaphoreFeatureFlags::EXPORTABLE
        | vk::ExternalSemaphoreFeatureFlags::IMPORTABLE;
    if !properties.external_semaphore_features.contains(needed) {
        return Err(Failure::other(
            "check sync files",
            format!(
                "binary semaphores offer {:?}",
                properties.external_semaphore_features
            ),
        ));
    }

    let mut client_scope = Scope::new(client);
    let busy = Busy::new(client, &mut client_scope)?;
    let mut export = vk::ExportSemaphoreCreateInfo::default()
        .handle_types(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
    let signalled =
        client_scope.semaphore(&vk::SemaphoreCreateInfo::default().push_next(&mut export))?;
    let signals = [vk::SemaphoreSubmitInfo::default()
        .semaphore(signalled)
        .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
    busy.submit(client, &mut client_scope, &signals, vk::Fence::null())?;
    let get_info = vk::SemaphoreGetFdInfoKHR::default()
        .semaphore(signalled)
        .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
    // SAFETY: the semaphore was made exportable as a sync file on this device, and a signal of
    // it is pending, as a sync file export requires.
    let raw = unsafe { client_fd.get_semaphore_fd(&get_info) }
        .map_err(|result| Failure::vulkan("export the semaphore as a sync file", result))?;
    let fd = sync_file(raw);
    let watched = watch(&fd, "keep a second descriptor of the sync file")?;
    let pending_at_import = !signalled_now(&fd, "poll the sync file before the import")?;

    let mut scope = Scope::new(compositor);
    let waited = scope.semaphore(&vk::SemaphoreCreateInfo::default())?;
    let import_info = vk::ImportSemaphoreFdInfoKHR::default()
        .semaphore(waited)
        .flags(vk::SemaphoreImportFlags::TEMPORARY)
        .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD)
        .fd(fd.as_ref().map_or(-1, AsRawFd::as_raw_fd));
    // SAFETY: the semaphore is new, binary and unused; a sync file import must be temporary,
    // and -1 stands for a payload already signalled.
    unsafe { compositor_fd.import_semaphore_fd(&import_info) }
        .map_err(|result| Failure::vulkan("import the sync file into the second device", result))?;
    // A successful import owns the descriptor.
    if let Some(fd) = fd {
        let _ = fd.into_raw_fd();
    }
    let fence = scope.fence(false)?;
    let waits = [vk::SemaphoreSubmitInfo::default()
        .semaphore(waited)
        .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
    submit_empty(
        compositor,
        &mut scope,
        "wait on the second device",
        &waits,
        &[],
        fence,
    )?;
    scope.wait_fence(fence, "wait on the second device")?;
    let outcome = waited_for_the_client(
        pending_at_import,
        &watched,
        "poll the sync file after the wait",
    )?;
    Ok(outcome.to_owned())
}

fn fence_through_sync_file(client: &Gpu<'_>, compositor: &Gpu<'_>) -> Result<String, Failure> {
    let (Some(client_fd), Some(compositor_fd)) = (&client.fence_fd, &compositor.fence_fd) else {
        return Err(Failure::other("check", "external_fence_fd is not enabled"));
    };
    let info = vk::PhysicalDeviceExternalFenceInfo::default()
        .handle_type(vk::ExternalFenceHandleTypeFlags::SYNC_FD);
    let mut properties = vk::ExternalFenceProperties::default();
    // SAFETY: the physical device comes from this instance; the structures live for the call.
    unsafe {
        client
            .vulkan
            .instance
            .get_physical_device_external_fence_properties(client.physical, &info, &mut properties)
    };
    let needed =
        vk::ExternalFenceFeatureFlags::EXPORTABLE | vk::ExternalFenceFeatureFlags::IMPORTABLE;
    if !properties.external_fence_features.contains(needed) {
        return Err(Failure::other(
            "check sync files",
            format!("fences offer {:?}", properties.external_fence_features),
        ));
    }

    let mut client_scope = Scope::new(client);
    let busy = Busy::new(client, &mut client_scope)?;
    let exported_fence = client_scope.fence(true)?;
    busy.submit(client, &mut client_scope, &[], exported_fence)?;
    let get_info = vk::FenceGetFdInfoKHR::default()
        .fence(exported_fence)
        .handle_type(vk::ExternalFenceHandleTypeFlags::SYNC_FD);
    // SAFETY: the fence was made exportable as a sync file on this device and has a pending
    // signal, as a sync file export requires. The export resets it: nothing waits on it after.
    let raw = unsafe { client_fd.get_fence_fd(&get_info) }
        .map_err(|result| Failure::vulkan("export the fence as a sync file", result))?;
    let fd = sync_file(raw);
    let watched = watch(&fd, "keep a second descriptor of the sync file")?;
    let pending_at_import = !signalled_now(&fd, "poll the sync file before the import")?;

    let mut scope = Scope::new(compositor);
    let imported_fence = scope.fence(false)?;
    let import_info = vk::ImportFenceFdInfoKHR::default()
        .fence(imported_fence)
        .flags(vk::FenceImportFlags::TEMPORARY)
        .handle_type(vk::ExternalFenceHandleTypeFlags::SYNC_FD)
        .fd(fd.as_ref().map_or(-1, AsRawFd::as_raw_fd));
    // SAFETY: the fence is new and no queue command uses it; a sync file import must be
    // temporary, and -1 stands for a payload already signalled.
    unsafe { compositor_fd.import_fence_fd(&import_info) }
        .map_err(|result| Failure::vulkan("import the sync file as a fence", result))?;
    if let Some(fd) = fd {
        let _ = fd.into_raw_fd();
    }
    scope.wait_fence(imported_fence, "wait on the imported fence")?;
    let outcome = waited_for_the_client(
        pending_at_import,
        &watched,
        "poll the sync file after the wait",
    )?;
    Ok(outcome.to_owned())
}

fn timeline_semaphore(gpu: &Gpu<'_>) -> Result<String, Failure> {
    let mut scope = Scope::new(gpu);
    let mut timeline_type = vk::SemaphoreTypeCreateInfo::default()
        .semaphore_type(vk::SemaphoreType::TIMELINE)
        .initial_value(0);
    let timeline =
        scope.semaphore(&vk::SemaphoreCreateInfo::default().push_next(&mut timeline_type))?;
    let wait_for = |value: u64, step: &str| {
        let semaphores = [timeline];
        let values = [value];
        let wait = vk::SemaphoreWaitInfo::default()
            .semaphores(&semaphores)
            .values(&values);
        // SAFETY: the semaphore is a timeline semaphore of this device; the arrays live for the
        // call.
        unsafe {
            gpu.device
                .wait_semaphores(&wait, GPU_TIMEOUT.as_nanos() as u64)
        }
        .map_err(|result| Failure::vulkan(step, result))
    };

    // The GPU signals 1; the CPU waits for it.
    let signal_one = [vk::SemaphoreSubmitInfo::default()
        .semaphore(timeline)
        .value(1)
        .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
    submit_empty(
        gpu,
        &mut scope,
        "signal 1 on the GPU",
        &[],
        &signal_one,
        vk::Fence::null(),
    )?;
    wait_for(1, "wait for 1 on the CPU")?;

    // A wait submitted before its signal: the GPU waits for 2, which the CPU signals after, then
    // signals 3. Should the CPU's signal fail, this batch never ends: every wait below is bounded
    // by GPU_TIMEOUT, and the scope's drop waits for the queue that long at most, then leaves its
    // objects alive rather than hang (gpu.rs, `queue_finished`).
    let wait_two = [vk::SemaphoreSubmitInfo::default()
        .semaphore(timeline)
        .value(2)
        .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
    let signal_three = [vk::SemaphoreSubmitInfo::default()
        .semaphore(timeline)
        .value(3)
        .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
    submit_empty(
        gpu,
        &mut scope,
        "wait for 2, then signal 3, on the GPU",
        &wait_two,
        &signal_three,
        vk::Fence::null(),
    )?;
    let signal = vk::SemaphoreSignalInfo::default()
        .semaphore(timeline)
        .value(2);
    // SAFETY: the semaphore is a timeline of this device at 1, and 2 is greater, as a host
    // signal requires.
    unsafe { gpu.device.signal_semaphore(&signal) }
        .map_err(|result| Failure::vulkan("signal 2 on the CPU", result))?;
    wait_for(3, "wait for 3 on the CPU")?;
    // SAFETY: the semaphore is a timeline of this device.
    let value = unsafe { gpu.device.get_semaphore_counter_value(timeline) }
        .map_err(|result| Failure::vulkan("read the counter", result))?;
    if value != 3 {
        return Err(Failure::other(
            "read the counter",
            format!("{value}, not 3"),
        ));
    }
    Ok("signalled on the GPU and the CPU, a wait submitted before its signal".to_owned())
}

pub fn to_json(result: &SyncResult) -> Json {
    Json::Object(vec![
        ("semaphore_sync_file", result.semaphore_sync_file.to_json()),
        ("fence_sync_file", result.fence_sync_file.to_json()),
        ("timeline_semaphore", result.timeline.to_json()),
    ])
}

pub fn to_markdown(result: &SyncResult, out: &mut String) {
    out.push_str("| Check | Result |\n|---|---|\n");
    out.push_str(&format!(
        "| Semaphore signalled behind the client's busy work, exported as a sync file, imported into the second device, waited | {} |\n",
        result.semaphore_sync_file.to_cell()
    ));
    out.push_str(&format!(
        "| Fence signalled behind the client's busy work, exported as a sync file, imported into the second device as a fence, waited | {} |\n",
        result.fence_sync_file.to_cell()
    ));
    out.push_str(&format!(
        "| Timeline semaphore signalled and waited | {} |\n\n",
        result.timeline.to_cell()
    ));
}
