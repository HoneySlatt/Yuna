//! Part 3: explicit synchronisation, as `wp_linux_drm_syncobj_v1` and implicit-sync fallbacks
//! need it. A semaphore exported as a sync file by the client's device and waited on by the
//! compositor's; a fence exported as a sync file and imported likewise; a timeline semaphore
//! signalled and waited, its wait submitted before its signal.

use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};

use ash::vk;

use crate::gpu::{Failure, GPU_TIMEOUT, Gpu, Scope};
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

/// What polling a sync file found, for the report.
fn poll_sync_file(fd: &Option<OwnedFd>, step: &str) -> Result<&'static str, Failure> {
    let Some(fd) = fd else {
        return Ok("exported as -1, already signalled");
    };
    match sys::sync_file_signals(fd, GPU_TIMEOUT.as_millis() as i32) {
        Ok(true) => Ok("the sync file polled signalled"),
        Ok(false) => Err(Failure::other(step, "the sync file did not signal in time")),
        Err(error) => Err(Failure::other(step, error.to_string())),
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
    let mut export = vk::ExportSemaphoreCreateInfo::default()
        .handle_types(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
    let signalled =
        client_scope.semaphore(&vk::SemaphoreCreateInfo::default().push_next(&mut export))?;
    let signals = [vk::SemaphoreSubmitInfo::default()
        .semaphore(signalled)
        .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
    submit_empty(
        client,
        &mut client_scope,
        "signal on the client",
        &[],
        &signals,
        vk::Fence::null(),
    )?;
    let get_info = vk::SemaphoreGetFdInfoKHR::default()
        .semaphore(signalled)
        .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
    // SAFETY: the semaphore was made exportable as a sync file on this device, and a signal of
    // it is pending, as a sync file export requires.
    let raw = unsafe { client_fd.get_semaphore_fd(&get_info) }
        .map_err(|result| Failure::vulkan("export the semaphore as a sync file", result))?;
    let fd = sync_file(raw);
    let polled = poll_sync_file(&fd, "poll the semaphore's sync file")?;

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
    Ok(format!(
        "{polled}; imported and waited on the second device"
    ))
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
    let exported_fence = client_scope.fence(true)?;
    submit_empty(
        client,
        &mut client_scope,
        "signal the fence on the client",
        &[],
        &[],
        exported_fence,
    )?;
    let get_info = vk::FenceGetFdInfoKHR::default()
        .fence(exported_fence)
        .handle_type(vk::ExternalFenceHandleTypeFlags::SYNC_FD);
    // SAFETY: the fence was made exportable as a sync file on this device and has a pending
    // signal, as a sync file export requires. The export resets it: nothing waits on it after.
    let raw = unsafe { client_fd.get_fence_fd(&get_info) }
        .map_err(|result| Failure::vulkan("export the fence as a sync file", result))?;
    let fd = sync_file(raw);
    let polled = poll_sync_file(&fd, "poll the fence's sync file")?;

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
    Ok(format!(
        "{polled}; imported as a fence on the second device and waited"
    ))
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
    // signals 3.
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
        "| Semaphore exported as a sync file, imported into the second device, waited | {} |\n",
        result.semaphore_sync_file.to_cell()
    ));
    out.push_str(&format!(
        "| Fence exported as a sync file, polled, imported into the second device, waited | {} |\n",
        result.fence_sync_file.to_cell()
    ));
    out.push_str(&format!(
        "| Timeline semaphore signalled and waited | {} |\n\n",
        result.timeline.to_cell()
    ));
}
