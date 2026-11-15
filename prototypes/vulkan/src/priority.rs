//! Part 6: whether the driver grants a queue of high or realtime global priority to a process
//! without privileges, as a compositor would ask so that its frames go before a game's.

use std::ffi::CStr;

use ash::{khr, vk};

use crate::gpu::{Failure, Vulkan};
use crate::json::Json;
use crate::outcome::Check;

const EXT_GLOBAL_PRIORITY: &CStr = c"VK_EXT_global_priority";
const EXT_GLOBAL_PRIORITY_QUERY: &CStr = c"VK_EXT_global_priority_query";

pub struct PriorityResult {
    /// The extension the devices were made with.
    pub extension: Option<String>,
    /// The priorities the queue family lists, when the driver offers the query.
    pub listed: Option<Vec<&'static str>>,
    pub high: Check,
    pub realtime: Check,
}

fn priority_name(priority: vk::QueueGlobalPriorityKHR) -> &'static str {
    match priority {
        vk::QueueGlobalPriorityKHR::LOW => "LOW",
        vk::QueueGlobalPriorityKHR::MEDIUM => "MEDIUM",
        vk::QueueGlobalPriorityKHR::HIGH => "HIGH",
        vk::QueueGlobalPriorityKHR::REALTIME => "REALTIME",
        _ => "unknown",
    }
}

pub fn run(vulkan: &Vulkan, physical: vk::PhysicalDevice) -> PriorityResult {
    let offered = match vulkan.device_extensions(physical) {
        Ok(offered) => offered,
        Err(failure) => {
            return PriorityResult {
                extension: None,
                listed: None,
                high: Check::Failed(failure.clone()),
                realtime: Check::Failed(failure),
            };
        }
    };
    let has = |name: &CStr| {
        offered
            .iter()
            .any(|offered| offered.as_bytes() == name.to_bytes())
    };
    let extension = [khr::global_priority::NAME, EXT_GLOBAL_PRIORITY]
        .into_iter()
        .find(|&name| has(name));
    let Some(extension) = extension else {
        let reason = "neither VK_KHR_global_priority nor VK_EXT_global_priority is offered";
        return PriorityResult {
            extension: None,
            listed: None,
            high: Check::Skipped(reason.to_owned()),
            realtime: Check::Skipped(reason.to_owned()),
        };
    };
    let Some((family, _)) = vulkan.graphics_family(physical) else {
        let failure = Failure::other("find the queue family", "no family draws and computes");
        return PriorityResult {
            extension: Some(extension.to_string_lossy().into_owned()),
            listed: None,
            high: Check::Failed(failure.clone()),
            realtime: Check::Failed(failure),
        };
    };
    let listed = (has(khr::global_priority::NAME) || has(EXT_GLOBAL_PRIORITY_QUERY))
        .then(|| listed_priorities(vulkan, physical, family));
    PriorityResult {
        extension: Some(extension.to_string_lossy().into_owned()),
        listed,
        high: ask(
            vulkan,
            physical,
            family,
            extension,
            vk::QueueGlobalPriorityKHR::HIGH,
        ),
        realtime: ask(
            vulkan,
            physical,
            family,
            extension,
            vk::QueueGlobalPriorityKHR::REALTIME,
        ),
    }
}

fn listed_priorities(
    vulkan: &Vulkan,
    physical: vk::PhysicalDevice,
    family: u32,
) -> Vec<&'static str> {
    // SAFETY: `physical` comes from this instance; the call only counts.
    let count = unsafe {
        vulkan
            .instance
            .get_physical_device_queue_family_properties2_len(physical)
    };
    let mut priorities = vec![vk::QueueFamilyGlobalPriorityPropertiesKHR::default(); count];
    let mut families: Vec<vk::QueueFamilyProperties2> = priorities
        .iter_mut()
        .map(|priorities| vk::QueueFamilyProperties2::default().push_next(priorities))
        .collect();
    // SAFETY: as above; the slice has the count's length, and each element chains a priority
    // structure the device offers (it has the global priority query), alive for the call.
    unsafe {
        vulkan
            .instance
            .get_physical_device_queue_family_properties2(physical, &mut families)
    };
    drop(families);
    let Some(listed) = priorities.get(family as usize) else {
        return Vec::new();
    };
    listed.priorities[..(listed.priority_count as usize).min(listed.priorities.len())]
        .iter()
        .map(|&priority| priority_name(priority))
        .collect()
}

/// Makes a device whose one queue asks `priority`, and destroys it at once if granted.
fn ask(
    vulkan: &Vulkan,
    physical: vk::PhysicalDevice,
    family: u32,
    extension: &CStr,
    priority: vk::QueueGlobalPriorityKHR,
) -> Check {
    let step = format!("make a queue at {}", priority_name(priority));
    let priorities = [1.0];
    let mut global =
        vk::DeviceQueueGlobalPriorityCreateInfoKHR::default().global_priority(priority);
    let queues = [vk::DeviceQueueCreateInfo::default()
        .queue_family_index(family)
        .queue_priorities(&priorities)
        .push_next(&mut global)];
    let extensions = [extension.as_ptr()];
    let info = vk::DeviceCreateInfo::default()
        .queue_create_infos(&queues)
        .enabled_extension_names(&extensions);
    // SAFETY: the extension is offered by the device, the queue family exists, and the info's
    // pointers live for the call.
    match unsafe { vulkan.instance.create_device(physical, &info, None) } {
        Ok(device) => {
            // SAFETY: the device was just made and nothing was made from it.
            unsafe { device.destroy_device(None) };
            Check::Passed("granted".to_owned())
        }
        Err(result) => Check::Failed(Failure::vulkan(step, result)),
    }
}

pub fn to_json(result: &PriorityResult) -> Json {
    Json::Object(vec![
        (
            "extension",
            Json::optional(result.extension.as_ref(), Json::text),
        ),
        (
            "listed_priorities",
            Json::optional(result.listed.as_ref(), |listed| {
                Json::Array(listed.iter().map(|&name| Json::text(name)).collect())
            }),
        ),
        ("high", result.high.to_json()),
        ("realtime", result.realtime.to_json()),
    ])
}

pub fn to_markdown(result: &PriorityResult, out: &mut String) {
    out.push_str(&format!(
        "Asked through {}; the graphics family lists {}.\n\n",
        result.extension.as_deref().unwrap_or("no extension"),
        result.listed.as_ref().map_or_else(
            || "no priorities (no query offered)".to_owned(),
            |listed| listed.join(", ")
        )
    ));
    out.push_str("| Priority | Result |\n|---|---|\n");
    out.push_str(&format!("| HIGH | {} |\n", result.high.to_cell()));
    out.push_str(&format!("| REALTIME | {} |\n\n", result.realtime.to_cell()));
}
