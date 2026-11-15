//! The Vulkan objects every part shares: the instance, a device with its queue, the scope that
//! destroys what a test created whether it passed or failed, and the recorder of commands.
//!
//! Every Vulkan call is `unsafe`: its soundness rests on handles that are alive and belong to the
//! device called, on pointers that live for the call (ash's builders borrow what they point at),
//! and on the spec's valid usage, which `--validate` checks with Khronos' layer. Each block's
//! `SAFETY:` comment says which of these it relies on and how it holds.

use std::ffi::{CStr, c_void};
use std::sync::Mutex;
use std::time::Duration;

use ash::{ext, khr, vk};

/// How long the prototype waits for the GPU before calling a step failed.
pub const GPU_TIMEOUT: Duration = Duration::from_secs(20);

/// A step that failed: what the prototype was doing, and Vulkan's or the system's answer.
#[derive(Clone, Debug)]
pub struct Failure {
    pub step: String,
    pub error: String,
}

impl Failure {
    pub fn vulkan(step: impl Into<String>, result: vk::Result) -> Self {
        Self {
            step: step.into(),
            error: format!("VK_{result:?}"),
        }
    }

    pub fn other(step: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            step: step.into(),
            error: error.into(),
        }
    }

    pub fn describe(&self) -> String {
        format!("{}: {}", self.step, self.error)
    }
}

/// A message the validation layer or the loader sent, counted by its id.
#[derive(Clone)]
pub struct ValidationMessage {
    pub severity: &'static str,
    /// "validation" or "performance" from the layer's checks; "general" from the loader and the
    /// layer's own news, such as a driver the loader skipped.
    pub kind: &'static str,
    pub id: String,
    pub first_text: String,
    pub count: u32,
}

/// The loader and the instance, with the validation layer's messenger when asked.
pub struct Vulkan {
    /// Held, never read: it keeps libvulkan.so.1 loaded, where the instance's functions live.
    _entry: ash::Entry,
    pub instance: ash::Instance,
    validation: Option<Validation>,
}

struct Validation {
    utils: ext::debug_utils::Instance,
    messenger: vk::DebugUtilsMessengerEXT,
    /// Boxed so that its address, given to the layer, stays put; dropped after the messenger.
    messages: Box<Mutex<Vec<ValidationMessage>>>,
}

const VALIDATION_LAYER: &CStr = c"VK_LAYER_KHRONOS_validation";

impl Vulkan {
    pub fn new(validate: bool) -> Result<Self, Failure> {
        // SAFETY: this opens the system's Vulkan loader, libvulkan.so.1, and runs its
        // initialisers; the loader is a library made to be loaded so, and nothing else in this
        // process has loaded another copy that could conflict.
        let entry = unsafe { ash::Entry::load() }
            .map_err(|error| Failure::other("load libvulkan.so.1", error.to_string()))?;

        let mut layers = Vec::new();
        let mut extensions = Vec::new();
        if validate {
            // SAFETY: the entry's functions are loaded; the call writes into a vector it sizes.
            let available = unsafe { entry.enumerate_instance_layer_properties() }
                .map_err(|result| Failure::vulkan("list the instance's layers", result))?;
            let present = available
                .iter()
                .any(|layer| layer.layer_name_as_c_str() == Ok(VALIDATION_LAYER));
            if !present {
                return Err(Failure::other(
                    "turn on validation",
                    "VK_LAYER_KHRONOS_validation is not installed; run inside `nix develop`",
                ));
            }
            layers.push(VALIDATION_LAYER.as_ptr());
            extensions.push(ext::debug_utils::NAME.as_ptr());
        }

        let messages: Box<Mutex<Vec<ValidationMessage>>> = Box::new(Mutex::new(Vec::new()));
        let mut messenger_info = vk::DebugUtilsMessengerCreateInfoEXT::default()
            .message_severity(
                vk::DebugUtilsMessageSeverityFlagsEXT::WARNING
                    | vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
            )
            .message_type(
                vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                    | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                    | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
            )
            .pfn_user_callback(Some(on_validation_message))
            .user_data(std::ptr::from_ref(messages.as_ref()).cast_mut().cast());

        let application = vk::ApplicationInfo::default()
            .application_name(c"vulkan-prototype")
            .api_version(vk::API_VERSION_1_3);
        let mut instance_info = vk::InstanceCreateInfo::default()
            .application_info(&application)
            .enabled_layer_names(&layers)
            .enabled_extension_names(&extensions);
        if validate {
            // Chained, the messenger also hears the instance's creation and destruction.
            instance_info = instance_info.push_next(&mut messenger_info);
        }
        // SAFETY: the create info and what it points at (names, application info, the chained
        // messenger info whose user data outlives the instance, see `Validation`) live for the
        // call; the layer and extension named were checked present.
        let instance = unsafe { entry.create_instance(&instance_info, None) }
            .map_err(|result| Failure::vulkan("create the instance", result))?;

        let validation = if validate {
            let utils = ext::debug_utils::Instance::new(&entry, &instance);
            // SAFETY: debug_utils is enabled on this instance; the info's user data is `messages`,
            // which `Validation` keeps alive until after the messenger is destroyed.
            match unsafe { utils.create_debug_utils_messenger(&messenger_info, None) } {
                Ok(messenger) => Some(Validation {
                    utils,
                    messenger,
                    messages,
                }),
                Err(result) => {
                    // SAFETY: the instance was just created and no object was made from it.
                    unsafe { instance.destroy_instance(None) };
                    return Err(Failure::vulkan("create the validation messenger", result));
                }
            }
        } else {
            None
        };

        Ok(Self {
            _entry: entry,
            instance,
            validation,
        })
    }

    pub fn validating(&self) -> bool {
        self.validation.is_some()
    }

    pub fn validation_messages(&self) -> Vec<ValidationMessage> {
        self.validation
            .as_ref()
            .and_then(|validation| validation.messages.lock().ok().map(|list| list.clone()))
            .unwrap_or_default()
    }

    pub fn physical_devices(&self) -> Result<Vec<vk::PhysicalDevice>, Failure> {
        // SAFETY: the instance is alive; the call writes into a vector it sizes.
        unsafe { self.instance.enumerate_physical_devices() }
            .map_err(|result| Failure::vulkan("list the physical devices", result))
    }

    /// The names of the device extensions the driver offers.
    pub fn device_extensions(&self, physical: vk::PhysicalDevice) -> Result<Vec<String>, Failure> {
        // SAFETY: `physical` comes from this instance; the call writes into a vector it sizes.
        let properties = unsafe {
            self.instance
                .enumerate_device_extension_properties(physical)
        }
        .map_err(|result| Failure::vulkan("list the device's extensions", result))?;
        Ok(properties
            .iter()
            .filter_map(|extension| extension.extension_name_as_c_str().ok())
            .map(|name| name.to_string_lossy().into_owned())
            .collect())
    }

    pub fn properties(&self, physical: vk::PhysicalDevice) -> vk::PhysicalDeviceProperties {
        // SAFETY: `physical` comes from this instance; the call fills a plain structure.
        unsafe { self.instance.get_physical_device_properties(physical) }
    }

    /// The first queue family that draws and computes: a compositor's own queue.
    pub fn graphics_family(
        &self,
        physical: vk::PhysicalDevice,
    ) -> Option<(u32, vk::QueueFamilyProperties)> {
        // SAFETY: `physical` comes from this instance; the call writes into a vector it sizes.
        let families = unsafe {
            self.instance
                .get_physical_device_queue_family_properties(physical)
        };
        families
            .into_iter()
            .enumerate()
            .find(|(_, family)| {
                family
                    .queue_flags
                    .contains(vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE)
            })
            .map(|(index, family)| (index as u32, family))
    }
}

impl Drop for Vulkan {
    fn drop(&mut self) {
        // SAFETY: every device was dropped before (a `Gpu` borrows its `Vulkan`), so nothing made
        // from the instance is alive; the messenger goes first, then the instance.
        unsafe {
            if let Some(validation) = &self.validation {
                validation
                    .utils
                    .destroy_debug_utils_messenger(validation.messenger, None);
            }
            self.instance.destroy_instance(None);
        }
    }
}

/// Counts each validation message by its id, keeping the first text.
///
/// # Safety
///
/// Only the layer calls it, through the messenger `Vulkan::new` made: `data` points at callback
/// data valid for the call, and `user_data` at the messages' mutex, alive until the messenger is
/// destroyed.
unsafe extern "system" fn on_validation_message(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    types: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    user_data: *mut c_void,
) -> vk::Bool32 {
    if data.is_null() || user_data.is_null() {
        return vk::FALSE;
    }
    // SAFETY: the layer passes callback data valid for the call, and `user_data` is the boxed
    // mutex `Vulkan::new` gave it, which lives until the messenger is destroyed.
    let (data, messages) = unsafe { (&*data, &*user_data.cast::<Mutex<Vec<ValidationMessage>>>()) };
    let read = |pointer: *const std::ffi::c_char| {
        if pointer.is_null() {
            String::new()
        } else {
            // SAFETY: the layer's strings are NUL-terminated and valid for the call.
            unsafe { CStr::from_ptr(pointer) }
                .to_string_lossy()
                .into_owned()
        }
    };
    let id = read(data.p_message_id_name);
    let text = read(data.p_message);
    let severity = if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
        "error"
    } else {
        "warning"
    };
    let kind = if types.contains(vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION) {
        "validation"
    } else if types.contains(vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE) {
        "performance"
    } else {
        "general"
    };
    if let Ok(mut messages) = messages.lock() {
        match messages.iter_mut().find(|message| message.id == id) {
            Some(message) => message.count += 1,
            None => messages.push(ValidationMessage {
                severity,
                kind,
                id,
                first_text: text,
                count: 1,
            }),
        }
    }
    vk::FALSE
}

/// The device extensions a `Gpu` turns on when the driver offers them.
const WANTED_EXTENSIONS: [&CStr; 7] = [
    khr::external_memory_fd::NAME,
    ext::external_memory_dma_buf::NAME,
    ext::image_drm_format_modifier::NAME,
    ext::queue_family_foreign::NAME,
    khr::external_semaphore_fd::NAME,
    khr::external_fence_fd::NAME,
    ext::external_memory_host::NAME,
];

/// The GPU's clock, as timestamp queries read it.
#[derive(Clone, Copy)]
pub struct Timestamps {
    pub period_ns: f64,
    pub mask: u64,
}

impl Timestamps {
    pub fn elapsed_us(&self, start: u64, end: u64) -> f64 {
        (end.wrapping_sub(start) & self.mask) as f64 * self.period_ns / 1000.0
    }
}

/// One logical device on the chosen physical device, with its universal queue. The prototype
/// makes two: a client's, which exports, and a compositor's, which imports.
pub struct Gpu<'v> {
    pub vulkan: &'v Vulkan,
    pub physical: vk::PhysicalDevice,
    pub device: ash::Device,
    pub queue: vk::Queue,
    pub queue_family: u32,
    pub memory: vk::PhysicalDeviceMemoryProperties,
    pub limits: vk::PhysicalDeviceLimits,
    pub timestamps: Option<Timestamps>,
    /// The family a buffer shared with another device or process is released to and acquired
    /// from: `FOREIGN` when the driver offers it, `EXTERNAL` otherwise.
    pub foreign_family: u32,
    pub dma_buf: bool,
    pub memory_fd: Option<khr::external_memory_fd::Device>,
    pub modifiers: Option<ext::image_drm_format_modifier::Device>,
    pub semaphore_fd: Option<khr::external_semaphore_fd::Device>,
    pub fence_fd: Option<khr::external_fence_fd::Device>,
    pub host_memory: Option<ext::external_memory_host::Device>,
    command_pool: vk::CommandPool,
}

impl<'v> Gpu<'v> {
    /// A device with every wanted extension the driver offers, Vulkan 1.3's dynamic rendering
    /// and synchronization2, and 1.2's timeline semaphores.
    pub fn new(vulkan: &'v Vulkan, physical: vk::PhysicalDevice) -> Result<Self, Failure> {
        let properties = vulkan.properties(physical);
        if properties.api_version < vk::API_VERSION_1_3 {
            return Err(Failure::other(
                "create a device",
                format!(
                    "Vulkan {} offered; the prototype needs 1.3 (dynamic rendering, synchronization2)",
                    version_text(properties.api_version)
                ),
            ));
        }
        let offered = vulkan.device_extensions(physical)?;
        let enabled: Vec<&CStr> = WANTED_EXTENSIONS
            .into_iter()
            .filter(|name| {
                offered
                    .iter()
                    .any(|offered| offered.as_bytes() == name.to_bytes())
            })
            .collect();
        let is_enabled = |name: &CStr| enabled.contains(&name);

        let mut supported_12 = vk::PhysicalDeviceVulkan12Features::default();
        let mut supported_13 = vk::PhysicalDeviceVulkan13Features::default();
        let mut supported = vk::PhysicalDeviceFeatures2::default()
            .push_next(&mut supported_12)
            .push_next(&mut supported_13);
        // SAFETY: `physical` comes from this instance, which is 1.3; the chain's structures live
        // for the call.
        unsafe {
            vulkan
                .instance
                .get_physical_device_features2(physical, &mut supported)
        };
        if supported_12.timeline_semaphore == vk::FALSE
            || supported_13.synchronization2 == vk::FALSE
            || supported_13.dynamic_rendering == vk::FALSE
        {
            return Err(Failure::other(
                "create a device",
                "the device lacks timeline semaphores, synchronization2 or dynamic rendering",
            ));
        }

        let (queue_family, family) = vulkan.graphics_family(physical).ok_or_else(|| {
            Failure::other("create a device", "no queue family draws and computes")
        })?;
        let priorities = [1.0];
        let queues = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family)
            .queue_priorities(&priorities)];
        let extension_pointers: Vec<_> = enabled.iter().map(|name| name.as_ptr()).collect();
        let mut features_12 =
            vk::PhysicalDeviceVulkan12Features::default().timeline_semaphore(true);
        let mut features_13 = vk::PhysicalDeviceVulkan13Features::default()
            .synchronization2(true)
            .dynamic_rendering(true);
        let device_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queues)
            .enabled_extension_names(&extension_pointers)
            .push_next(&mut features_12)
            .push_next(&mut features_13);
        // SAFETY: the extensions are among those offered, the features were checked supported,
        // and the info's pointers live for the call.
        let device = unsafe { vulkan.instance.create_device(physical, &device_info, None) }
            .map_err(|result| Failure::vulkan("create a device", result))?;
        // SAFETY: the device was made with one queue in this family.
        let queue = unsafe { device.get_device_queue(queue_family, 0) };
        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(queue_family)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        // SAFETY: the device is alive and the family is the queue's.
        let command_pool = match unsafe { device.create_command_pool(&pool_info, None) } {
            Ok(pool) => pool,
            Err(result) => {
                // SAFETY: nothing was made from the new device yet.
                unsafe { device.destroy_device(None) };
                return Err(Failure::vulkan("create a command pool", result));
            }
        };
        // SAFETY: `physical` comes from this instance; the call fills a plain structure.
        let memory = unsafe {
            vulkan
                .instance
                .get_physical_device_memory_properties(physical)
        };
        let timestamps = (family.timestamp_valid_bits > 0).then(|| Timestamps {
            period_ns: f64::from(properties.limits.timestamp_period),
            mask: if family.timestamp_valid_bits >= 64 {
                u64::MAX
            } else {
                (1u64 << family.timestamp_valid_bits) - 1
            },
        });

        let instance = &vulkan.instance;
        Ok(Self {
            vulkan,
            physical,
            queue,
            queue_family,
            memory,
            limits: properties.limits,
            timestamps,
            foreign_family: if is_enabled(ext::queue_family_foreign::NAME) {
                vk::QUEUE_FAMILY_FOREIGN_EXT
            } else {
                vk::QUEUE_FAMILY_EXTERNAL
            },
            dma_buf: is_enabled(ext::external_memory_dma_buf::NAME),
            memory_fd: is_enabled(khr::external_memory_fd::NAME)
                .then(|| khr::external_memory_fd::Device::new(instance, &device)),
            modifiers: is_enabled(ext::image_drm_format_modifier::NAME)
                .then(|| ext::image_drm_format_modifier::Device::new(instance, &device)),
            semaphore_fd: is_enabled(khr::external_semaphore_fd::NAME)
                .then(|| khr::external_semaphore_fd::Device::new(instance, &device)),
            fence_fd: is_enabled(khr::external_fence_fd::NAME)
                .then(|| khr::external_fence_fd::Device::new(instance, &device)),
            host_memory: is_enabled(ext::external_memory_host::NAME)
                .then(|| ext::external_memory_host::Device::new(instance, &device)),
            command_pool,
            device,
        })
    }

    /// The best memory type among `allowed`: it has `required`, and `preferred` when one does.
    pub fn memory_type(
        &self,
        allowed: u32,
        required: vk::MemoryPropertyFlags,
        preferred: vk::MemoryPropertyFlags,
    ) -> Option<u32> {
        let candidates = || {
            (0..self.memory.memory_type_count).filter(move |&index| {
                allowed & (1 << index) != 0
                    && self.memory.memory_types[index as usize]
                        .property_flags
                        .contains(required)
            })
        };
        candidates()
            .find(|&index| {
                self.memory.memory_types[index as usize]
                    .property_flags
                    .contains(preferred)
            })
            .or_else(|| candidates().next())
    }

    /// Records a command buffer with `record`, submits it and waits for it.
    pub fn run_once(
        &self,
        scope: &mut Scope<'_>,
        step: &str,
        record: impl FnOnce(&Recorder<'_>),
    ) -> Result<(), Failure> {
        let commands = scope.command_buffer(self)?;
        let fence = scope.fence(false)?;
        self.record(commands, step, record)?;
        self.submit(step, commands, &[], &[], fence)?;
        scope.wait_fence(fence, step)
    }

    /// Records `commands` from the start: it must not be pending on the GPU.
    pub fn record(
        &self,
        commands: vk::CommandBuffer,
        step: &str,
        record: impl FnOnce(&Recorder<'_>),
    ) -> Result<(), Failure> {
        let begin = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: the command buffer comes from this device's resettable pool, and the callers
        // wait for its last submission before recording it again.
        unsafe {
            self.device
                .reset_command_buffer(commands, vk::CommandBufferResetFlags::empty())
                .and_then(|()| self.device.begin_command_buffer(commands, &begin))
        }
        .map_err(|result| Failure::vulkan(format!("{step}: begin the commands"), result))?;
        record(&Recorder {
            device: &self.device,
            commands,
        });
        // SAFETY: the command buffer is recording, begun above.
        unsafe { self.device.end_command_buffer(commands) }
            .map_err(|result| Failure::vulkan(format!("{step}: record the commands"), result))
    }

    /// Submits `commands` to the queue, waiting on and signalling the semaphores given.
    pub fn submit(
        &self,
        step: &str,
        commands: vk::CommandBuffer,
        waits: &[vk::SemaphoreSubmitInfo<'_>],
        signals: &[vk::SemaphoreSubmitInfo<'_>],
        fence: vk::Fence,
    ) -> Result<(), Failure> {
        let buffers = [vk::CommandBufferSubmitInfo::default().command_buffer(commands)];
        let submits = [vk::SubmitInfo2::default()
            .command_buffer_infos(&buffers)
            .wait_semaphore_infos(waits)
            .signal_semaphore_infos(signals)];
        // SAFETY: the command buffer is recorded and not pending, the semaphores and fence come
        // from this device, and the queue is used from this thread alone.
        unsafe { self.device.queue_submit2(self.queue, &submits, fence) }
            .map_err(|result| Failure::vulkan(format!("{step}: submit"), result))
    }

    /// The microseconds between the two timestamps in `pool`, once the GPU wrote them; none
    /// when the queue keeps no timestamps.
    pub fn elapsed_us(&self, pool: Option<vk::QueryPool>) -> Result<Option<f64>, Failure> {
        let (Some(pool), Some(timestamps)) = (pool, self.timestamps) else {
            return Ok(None);
        };
        let mut values = [0u64; 2];
        // SAFETY: the pool holds two timestamp queries, written by a submission the caller waited
        // for; the slice holds two 64-bit values.
        unsafe {
            self.device.get_query_pool_results(
                pool,
                0,
                &mut values,
                vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT,
            )
        }
        .map_err(|result| Failure::vulkan("read the timestamps", result))?;
        Ok(Some(timestamps.elapsed_us(values[0], values[1])))
    }
}

impl Drop for Gpu<'_> {
    fn drop(&mut self) {
        // SAFETY: every scope that made objects on this device borrowed it and was dropped first;
        // waiting for the device leaves no work pending on the pool or the device.
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_device(None);
        }
    }
}

pub fn version_text(version: u32) -> String {
    format!(
        "{}.{}.{}",
        vk::api_version_major(version),
        vk::api_version_minor(version),
        vk::api_version_patch(version)
    )
}

/// An object a scope destroys.
enum Object {
    Buffer(vk::Buffer),
    Image(vk::Image),
    ImageView(vk::ImageView),
    Sampler(vk::Sampler),
    Memory(vk::DeviceMemory),
    Fence(vk::Fence),
    Semaphore(vk::Semaphore),
    QueryPool(vk::QueryPool),
    DescriptorPool(vk::DescriptorPool),
    DescriptorSetLayout(vk::DescriptorSetLayout),
    PipelineLayout(vk::PipelineLayout),
    Pipeline(vk::Pipeline),
    ShaderModule(vk::ShaderModule),
    CommandBuffer(vk::CommandPool, vk::CommandBuffer),
}

/// What one test made on one device, destroyed when the scope ends, whether the test passed or
/// failed, once the device is idle.
pub struct Scope<'d> {
    device: &'d ash::Device,
    objects: Vec<Object>,
}

impl<'d> Scope<'d> {
    pub fn new(gpu: &'d Gpu<'_>) -> Self {
        Self {
            device: &gpu.device,
            objects: Vec::new(),
        }
    }

    pub fn buffer(&mut self, info: &vk::BufferCreateInfo<'_>) -> Result<vk::Buffer, Failure> {
        // SAFETY: the info's pointers live for the call; the scope destroys the buffer before the
        // device, which it borrows.
        let buffer = unsafe { self.device.create_buffer(info, None) }
            .map_err(|result| Failure::vulkan("create a buffer", result))?;
        self.objects.push(Object::Buffer(buffer));
        Ok(buffer)
    }

    pub fn image(&mut self, info: &vk::ImageCreateInfo<'_>) -> Result<vk::Image, Failure> {
        // SAFETY: as for `buffer`; the callers check the image's format, tiling and usage are
        // supported before creating it.
        let image = unsafe { self.device.create_image(info, None) }
            .map_err(|result| Failure::vulkan("create an image", result))?;
        self.objects.push(Object::Image(image));
        Ok(image)
    }

    pub fn image_view(
        &mut self,
        info: &vk::ImageViewCreateInfo<'_>,
    ) -> Result<vk::ImageView, Failure> {
        // SAFETY: as for `buffer`; the view's image belongs to this device.
        let view = unsafe { self.device.create_image_view(info, None) }
            .map_err(|result| Failure::vulkan("create an image view", result))?;
        self.objects.push(Object::ImageView(view));
        Ok(view)
    }

    pub fn sampler(&mut self, info: &vk::SamplerCreateInfo<'_>) -> Result<vk::Sampler, Failure> {
        // SAFETY: as for `buffer`.
        let sampler = unsafe { self.device.create_sampler(info, None) }
            .map_err(|result| Failure::vulkan("create a sampler", result))?;
        self.objects.push(Object::Sampler(sampler));
        Ok(sampler)
    }

    pub fn memory(
        &mut self,
        info: &vk::MemoryAllocateInfo<'_>,
        step: &str,
    ) -> Result<vk::DeviceMemory, Failure> {
        // SAFETY: as for `buffer`; an imported handle in the chain is given to Vulkan only when
        // the allocation succeeds, so a failure leaves it with the caller.
        let memory = unsafe { self.device.allocate_memory(info, None) }
            .map_err(|result| Failure::vulkan(step, result))?;
        self.objects.push(Object::Memory(memory));
        Ok(memory)
    }

    pub fn fence(&mut self, export_sync_file: bool) -> Result<vk::Fence, Failure> {
        let mut export = vk::ExportFenceCreateInfo::default()
            .handle_types(vk::ExternalFenceHandleTypeFlags::SYNC_FD);
        let mut info = vk::FenceCreateInfo::default();
        if export_sync_file {
            info = info.push_next(&mut export);
        }
        // SAFETY: as for `buffer`; the callers ask for an exportable fence only when the device
        // has external_fence_fd and reports sync files exportable.
        let fence = unsafe { self.device.create_fence(&info, None) }
            .map_err(|result| Failure::vulkan("create a fence", result))?;
        self.objects.push(Object::Fence(fence));
        Ok(fence)
    }

    pub fn semaphore(
        &mut self,
        info: &vk::SemaphoreCreateInfo<'_>,
    ) -> Result<vk::Semaphore, Failure> {
        // SAFETY: as for `buffer`.
        let semaphore = unsafe { self.device.create_semaphore(info, None) }
            .map_err(|result| Failure::vulkan("create a semaphore", result))?;
        self.objects.push(Object::Semaphore(semaphore));
        Ok(semaphore)
    }

    /// A pool of two timestamp queries, a start and an end; none when the queue keeps no
    /// timestamps (its `timestampValidBits` is 0).
    pub fn timestamp_pool(&mut self, gpu: &Gpu<'_>) -> Result<Option<vk::QueryPool>, Failure> {
        if gpu.timestamps.is_none() {
            return Ok(None);
        }
        let info = vk::QueryPoolCreateInfo::default()
            .query_type(vk::QueryType::TIMESTAMP)
            .query_count(2);
        // SAFETY: as for `buffer`.
        let pool = unsafe { self.device.create_query_pool(&info, None) }
            .map_err(|result| Failure::vulkan("create a query pool", result))?;
        self.objects.push(Object::QueryPool(pool));
        Ok(Some(pool))
    }

    /// A descriptor set of `layout`, from a pool of its own.
    pub fn descriptor_set(
        &mut self,
        layout: vk::DescriptorSetLayout,
        sizes: &[vk::DescriptorPoolSize],
    ) -> Result<vk::DescriptorSet, Failure> {
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(1)
            .pool_sizes(sizes);
        // SAFETY: as for `buffer`.
        let pool = unsafe { self.device.create_descriptor_pool(&pool_info, None) }
            .map_err(|result| Failure::vulkan("create a descriptor pool", result))?;
        self.objects.push(Object::DescriptorPool(pool));
        let layouts = [layout];
        let set_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool)
            .set_layouts(&layouts);
        // SAFETY: the pool and layout come from this device, and the pool has room for the set;
        // destroying the pool frees it.
        let sets = unsafe { self.device.allocate_descriptor_sets(&set_info) }
            .map_err(|result| Failure::vulkan("allocate a descriptor set", result))?;
        sets.first()
            .copied()
            .ok_or_else(|| Failure::other("allocate a descriptor set", "no set returned"))
    }

    pub fn descriptor_set_layout(
        &mut self,
        bindings: &[vk::DescriptorSetLayoutBinding<'_>],
    ) -> Result<vk::DescriptorSetLayout, Failure> {
        let info = vk::DescriptorSetLayoutCreateInfo::default().bindings(bindings);
        // SAFETY: as for `buffer`.
        let layout = unsafe { self.device.create_descriptor_set_layout(&info, None) }
            .map_err(|result| Failure::vulkan("create a descriptor set layout", result))?;
        self.objects.push(Object::DescriptorSetLayout(layout));
        Ok(layout)
    }

    pub fn pipeline_layout(
        &mut self,
        set_layouts: &[vk::DescriptorSetLayout],
        push_constants: &[vk::PushConstantRange],
    ) -> Result<vk::PipelineLayout, Failure> {
        let info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(set_layouts)
            .push_constant_ranges(push_constants);
        // SAFETY: as for `buffer`; the set layouts come from this device.
        let layout = unsafe { self.device.create_pipeline_layout(&info, None) }
            .map_err(|result| Failure::vulkan("create a pipeline layout", result))?;
        self.objects.push(Object::PipelineLayout(layout));
        Ok(layout)
    }

    /// A shader module from SPIR-V that build.rs compiled.
    pub fn shader(&mut self, spirv: &[u8]) -> Result<vk::ShaderModule, Failure> {
        let words = ash::util::read_spv(&mut std::io::Cursor::new(spirv))
            .map_err(|error| Failure::other("read a shader", error.to_string()))?;
        let info = vk::ShaderModuleCreateInfo::default().code(&words);
        // SAFETY: as for `buffer`; the words are glslangValidator's SPIR-V for Vulkan 1.3.
        let module = unsafe { self.device.create_shader_module(&info, None) }
            .map_err(|result| Failure::vulkan("create a shader module", result))?;
        self.objects.push(Object::ShaderModule(module));
        Ok(module)
    }

    /// A pipeline that draws `vertex` and `fragment` into one colour attachment of `format`,
    /// without vertex buffers, blending premultiplied colour over the attachment when `blend`.
    pub fn graphics_pipeline(
        &mut self,
        vertex: vk::ShaderModule,
        fragment: vk::ShaderModule,
        layout: vk::PipelineLayout,
        format: vk::Format,
        topology: vk::PrimitiveTopology,
        blend: bool,
    ) -> Result<vk::Pipeline, Failure> {
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vertex)
                .name(c"main"),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(fragment)
                .name(c"main"),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default().topology(topology);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(blend)
            .src_color_blend_factor(vk::BlendFactor::ONE)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(vk::ColorComponentFlags::RGBA)];
        let colour_blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let formats = [format];
        let mut rendering =
            vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&formats);
        let info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport)
            .rasterization_state(&rasterization)
            .multisample_state(&multisample)
            .color_blend_state(&colour_blend)
            .dynamic_state(&dynamic)
            .layout(layout)
            .push_next(&mut rendering);
        // SAFETY: as for `buffer`; the modules and layout come from this device, and dynamic
        // rendering is enabled on it.
        let pipelines = unsafe {
            self.device
                .create_graphics_pipelines(vk::PipelineCache::null(), &[info], None)
        }
        .map_err(|(_, result)| Failure::vulkan("create a graphics pipeline", result))?;
        let pipeline = pipelines[0];
        self.objects.push(Object::Pipeline(pipeline));
        Ok(pipeline)
    }

    pub fn compute_pipeline(
        &mut self,
        module: vk::ShaderModule,
        layout: vk::PipelineLayout,
    ) -> Result<vk::Pipeline, Failure> {
        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(module)
            .name(c"main");
        let info = vk::ComputePipelineCreateInfo::default()
            .stage(stage)
            .layout(layout);
        // SAFETY: as for `buffer`; the module and layout come from this device.
        let pipelines = unsafe {
            self.device
                .create_compute_pipelines(vk::PipelineCache::null(), &[info], None)
        }
        .map_err(|(_, result)| Failure::vulkan("create a compute pipeline", result))?;
        let pipeline = pipelines[0];
        self.objects.push(Object::Pipeline(pipeline));
        Ok(pipeline)
    }

    pub fn command_buffer(&mut self, gpu: &Gpu<'_>) -> Result<vk::CommandBuffer, Failure> {
        let info = vk::CommandBufferAllocateInfo::default()
            .command_pool(gpu.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: the pool belongs to this scope's device and is used from this thread alone.
        let buffers = unsafe { self.device.allocate_command_buffers(&info) }
            .map_err(|result| Failure::vulkan("allocate a command buffer", result))?;
        let commands = buffers[0];
        self.objects
            .push(Object::CommandBuffer(gpu.command_pool, commands));
        Ok(commands)
    }

    /// A buffer bound to new memory of a type with `required` properties, mapped when they are
    /// host-visible.
    pub fn bound_buffer(
        &mut self,
        gpu: &Gpu<'_>,
        size: u64,
        usage: vk::BufferUsageFlags,
        required: vk::MemoryPropertyFlags,
        preferred: vk::MemoryPropertyFlags,
    ) -> Result<(vk::Buffer, Option<*mut u8>), Failure> {
        let buffer = self.buffer(
            &vk::BufferCreateInfo::default()
                .size(size)
                .usage(usage)
                .sharing_mode(vk::SharingMode::EXCLUSIVE),
        )?;
        // SAFETY: the buffer was just made on this device.
        let requirements = unsafe { self.device.get_buffer_memory_requirements(buffer) };
        let memory_type = gpu
            .memory_type(requirements.memory_type_bits, required, preferred)
            .ok_or_else(|| Failure::other("allocate a buffer's memory", "no memory type fits"))?;
        let memory = self.memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(requirements.size)
                .memory_type_index(memory_type),
            "allocate a buffer's memory",
        )?;
        // SAFETY: the memory is new, of a type the buffer allows, and as large as it needs.
        unsafe { self.device.bind_buffer_memory(buffer, memory, 0) }
            .map_err(|result| Failure::vulkan("bind a buffer's memory", result))?;
        if !required.contains(vk::MemoryPropertyFlags::HOST_VISIBLE) {
            return Ok((buffer, None));
        }
        // SAFETY: the memory is host-visible and not mapped yet; it stays mapped until freed.
        let pointer = unsafe {
            self.device
                .map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
        }
        .map_err(|result| Failure::vulkan("map a buffer's memory", result))?;
        Ok((buffer, Some(pointer.cast())))
    }

    /// Waits for `fence` and resets it for the next submission.
    pub fn wait_fence(&self, fence: vk::Fence, step: &str) -> Result<(), Failure> {
        // SAFETY: the fence comes from this device; once it signalled nothing is pending on it.
        unsafe {
            self.device
                .wait_for_fences(&[fence], true, GPU_TIMEOUT.as_nanos() as u64)
                .and_then(|()| self.device.reset_fences(&[fence]))
        }
        .map_err(|result| Failure::vulkan(format!("{step}: wait for the GPU"), result))
    }
}

impl Drop for Scope<'_> {
    fn drop(&mut self) {
        // SAFETY: once the device is idle no submitted work uses these objects; each was made on
        // this device and is destroyed once, in reverse order of creation, memory last, after the
        // buffers and images bound to it.
        unsafe {
            let _ = self.device.device_wait_idle();
            for object in self.objects.iter().rev() {
                match *object {
                    Object::Buffer(buffer) => self.device.destroy_buffer(buffer, None),
                    Object::Image(image) => self.device.destroy_image(image, None),
                    Object::ImageView(view) => self.device.destroy_image_view(view, None),
                    Object::Sampler(sampler) => self.device.destroy_sampler(sampler, None),
                    Object::Memory(_) => {}
                    Object::Fence(fence) => self.device.destroy_fence(fence, None),
                    Object::Semaphore(semaphore) => self.device.destroy_semaphore(semaphore, None),
                    Object::QueryPool(pool) => self.device.destroy_query_pool(pool, None),
                    Object::DescriptorPool(pool) => self.device.destroy_descriptor_pool(pool, None),
                    Object::DescriptorSetLayout(layout) => {
                        self.device.destroy_descriptor_set_layout(layout, None)
                    }
                    Object::PipelineLayout(layout) => {
                        self.device.destroy_pipeline_layout(layout, None)
                    }
                    Object::Pipeline(pipeline) => self.device.destroy_pipeline(pipeline, None),
                    Object::ShaderModule(module) => self.device.destroy_shader_module(module, None),
                    Object::CommandBuffer(pool, commands) => {
                        self.device.free_command_buffers(pool, &[commands])
                    }
                }
            }
            for object in self.objects.iter().rev() {
                if let Object::Memory(memory) = *object {
                    self.device.free_memory(memory, None);
                }
            }
        }
    }
}

/// The whole colour aspect of a one-level, one-layer image.
pub const COLOUR: vk::ImageSubresourceRange = vk::ImageSubresourceRange {
    aspect_mask: vk::ImageAspectFlags::COLOR,
    base_mip_level: 0,
    level_count: 1,
    base_array_layer: 0,
    layer_count: 1,
};

/// Records commands into a command buffer between its begin and end: `Gpu::record` makes the
/// only ones, so every method runs on a buffer in the recording state.
pub struct Recorder<'a> {
    device: &'a ash::Device,
    commands: vk::CommandBuffer,
}

// Each method below records one command. Its SAFETY rests on the recorder's invariant (the
// buffer is recording), on handles that come from the same device, which the scopes owning them
// keep alive until the GPU finished (they wait for the device), and on the command's valid usage,
// which the callers follow and `--validate` checks.
impl Recorder<'_> {
    pub fn barriers(
        &self,
        images: &[vk::ImageMemoryBarrier2<'_>],
        buffers: &[vk::BufferMemoryBarrier2<'_>],
    ) {
        let dependency = vk::DependencyInfo::default()
            .image_memory_barriers(images)
            .buffer_memory_barriers(buffers);
        // SAFETY: see the comment above `impl Recorder`.
        unsafe {
            self.device
                .cmd_pipeline_barrier2(self.commands, &dependency)
        };
    }

    /// Resets the pool's two queries and writes the first, when there is a pool.
    pub fn start_timestamps(&self, pool: Option<vk::QueryPool>) {
        let Some(pool) = pool else { return };
        // SAFETY: see the comment above `impl Recorder`; the pool holds two timestamp queries,
        // made only when the queue keeps timestamps.
        unsafe {
            self.device.cmd_reset_query_pool(self.commands, pool, 0, 2);
            self.device.cmd_write_timestamp2(
                self.commands,
                vk::PipelineStageFlags2::TOP_OF_PIPE,
                pool,
                0,
            );
        };
    }

    /// Writes the pool's second query once every command before it finished.
    pub fn end_timestamps(&self, pool: Option<vk::QueryPool>) {
        let Some(pool) = pool else { return };
        // SAFETY: see the comment above `impl Recorder`; `start_timestamps` reset the query in
        // this buffer.
        unsafe {
            self.device.cmd_write_timestamp2(
                self.commands,
                vk::PipelineStageFlags2::ALL_COMMANDS,
                pool,
                1,
            )
        };
    }

    pub fn copy_buffer_to_image(
        &self,
        buffer: vk::Buffer,
        image: vk::Image,
        regions: &[vk::BufferImageCopy],
    ) {
        // SAFETY: see the comment above `impl Recorder`; the image is in TRANSFER_DST_OPTIMAL and
        // the buffer holds the regions' texels, tightly packed.
        unsafe {
            self.device.cmd_copy_buffer_to_image(
                self.commands,
                buffer,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                regions,
            )
        };
    }

    pub fn copy_image_to_buffer(
        &self,
        image: vk::Image,
        buffer: vk::Buffer,
        regions: &[vk::BufferImageCopy],
    ) {
        // SAFETY: see the comment above `impl Recorder`; the image is in TRANSFER_SRC_OPTIMAL and
        // the buffer has room for the regions' texels, tightly packed.
        unsafe {
            self.device.cmd_copy_image_to_buffer(
                self.commands,
                image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buffer,
                regions,
            )
        };
    }

    /// Begins drawing into `view`, cleared to `clear` when given, its viewport and scissor the
    /// whole image.
    pub fn begin_rendering(
        &self,
        view: vk::ImageView,
        width: u32,
        height: u32,
        clear: Option<[f32; 4]>,
    ) {
        let attachments = [vk::RenderingAttachmentInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(if clear.is_some() {
                vk::AttachmentLoadOp::CLEAR
            } else {
                vk::AttachmentLoadOp::DONT_CARE
            })
            .store_op(vk::AttachmentStoreOp::STORE)
            .clear_value(vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: clear.unwrap_or_default(),
                },
            })];
        let area = vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent: vk::Extent2D { width, height },
        };
        let info = vk::RenderingInfo::default()
            .render_area(area)
            .layer_count(1)
            .color_attachments(&attachments);
        let viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: width as f32,
            height: height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        // SAFETY: see the comment above `impl Recorder`; the view's image is in
        // COLOR_ATTACHMENT_OPTIMAL, and the pipelines bound after take viewport and scissor as
        // dynamic state.
        unsafe {
            self.device.cmd_begin_rendering(self.commands, &info);
            self.device.cmd_set_viewport(self.commands, 0, &[viewport]);
            self.device.cmd_set_scissor(self.commands, 0, &[area]);
        }
    }

    pub fn end_rendering(&self) {
        // SAFETY: see the comment above `impl Recorder`; rendering was begun in this buffer.
        unsafe { self.device.cmd_end_rendering(self.commands) };
    }

    pub fn bind_pipeline(&self, point: vk::PipelineBindPoint, pipeline: vk::Pipeline) {
        // SAFETY: see the comment above `impl Recorder`.
        unsafe {
            self.device
                .cmd_bind_pipeline(self.commands, point, pipeline)
        };
    }

    pub fn bind_set(
        &self,
        point: vk::PipelineBindPoint,
        layout: vk::PipelineLayout,
        set: vk::DescriptorSet,
    ) {
        // SAFETY: see the comment above `impl Recorder`; the set matches the layout's set 0.
        unsafe {
            self.device
                .cmd_bind_descriptor_sets(self.commands, point, layout, 0, &[set], &[])
        };
    }

    pub fn push_constants(
        &self,
        layout: vk::PipelineLayout,
        stages: vk::ShaderStageFlags,
        bytes: &[u8],
    ) {
        // SAFETY: see the comment above `impl Recorder`; the layout's range covers these stages and
        // bytes from offset 0.
        unsafe {
            self.device
                .cmd_push_constants(self.commands, layout, stages, 0, bytes)
        };
    }

    pub fn draw(&self, vertices: u32) {
        // SAFETY: see the comment above `impl Recorder`; a graphics pipeline is bound inside
        // rendering, and its shaders read no vertex buffer.
        unsafe { self.device.cmd_draw(self.commands, vertices, 1, 0, 0) };
    }

    pub fn dispatch(&self, x: u32, y: u32) {
        // SAFETY: see the comment above `impl Recorder`; a compute pipeline and its set are bound.
        unsafe { self.device.cmd_dispatch(self.commands, x, y, 1) };
    }
}

/// A copy of `width` × `height` texels at (x, y) in the image, tightly packed in the buffer from
/// `buffer_offset`.
pub fn texel_copy(
    buffer_offset: u64,
    (x, y): (u32, u32),
    (width, height): (u32, u32),
) -> vk::BufferImageCopy {
    vk::BufferImageCopy {
        buffer_offset,
        buffer_row_length: 0,
        buffer_image_height: 0,
        image_subresource: vk::ImageSubresourceLayers {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            mip_level: 0,
            base_array_layer: 0,
            layer_count: 1,
        },
        image_offset: vk::Offset3D {
            x: x as i32,
            y: y as i32,
            z: 0,
        },
        image_extent: vk::Extent3D {
            width,
            height,
            depth: 1,
        },
    }
}

/// An image barrier on the whole colour aspect.
pub fn image_barrier(
    image: vk::Image,
    (old_layout, new_layout): (vk::ImageLayout, vk::ImageLayout),
    (src_stage, src_access): (vk::PipelineStageFlags2, vk::AccessFlags2),
    (dst_stage, dst_access): (vk::PipelineStageFlags2, vk::AccessFlags2),
) -> vk::ImageMemoryBarrier2<'static> {
    vk::ImageMemoryBarrier2::default()
        .image(image)
        .subresource_range(COLOUR)
        .old_layout(old_layout)
        .new_layout(new_layout)
        .src_stage_mask(src_stage)
        .src_access_mask(src_access)
        .dst_stage_mask(dst_stage)
        .dst_access_mask(dst_access)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
}

/// Makes the GPU's writes to a host-visible buffer visible to the CPU.
pub fn buffer_to_host(
    buffer: vk::Buffer,
    src_stage: vk::PipelineStageFlags2,
    src_access: vk::AccessFlags2,
) -> vk::BufferMemoryBarrier2<'static> {
    vk::BufferMemoryBarrier2::default()
        .buffer(buffer)
        .offset(0)
        .size(vk::WHOLE_SIZE)
        .src_stage_mask(src_stage)
        .src_access_mask(src_access)
        .dst_stage_mask(vk::PipelineStageFlags2::HOST)
        .dst_access_mask(vk::AccessFlags2::HOST_READ)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
}
