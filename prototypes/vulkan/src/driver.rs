//! Part 1: the driver. The device and its driver, Mesa's version among them, Vulkan's version,
//! the PCI ids, the render node, and whether each extension Stage 2's renderer would lean on is
//! offered.

use ash::vk;

use crate::gpu::{Vulkan, version_text};
use crate::json::Json;

/// The extensions ADR-0073's questions read, in the report's order.
const EXTENSIONS: [&str; 13] = [
    "VK_EXT_image_drm_format_modifier",
    "VK_KHR_external_memory_fd",
    "VK_EXT_external_memory_dma_buf",
    "VK_EXT_queue_family_foreign",
    "VK_KHR_external_semaphore_fd",
    "VK_KHR_external_fence_fd",
    "VK_KHR_timeline_semaphore",
    "VK_EXT_external_memory_host",
    "VK_EXT_physical_device_drm",
    "VK_KHR_global_priority",
    "VK_EXT_global_priority",
    "VK_EXT_global_priority_query",
    "VK_EXT_host_image_copy",
];

/// A render or primary node's device numbers.
#[derive(Clone, Copy)]
pub struct Node {
    pub major: i64,
    pub minor: i64,
}

pub struct DriverInfo {
    pub device_name: String,
    pub device_type: &'static str,
    pub vendor_id: u32,
    pub device_id: u32,
    pub api_version: u32,
    /// Mesa packs its own version the way Vulkan packs its: major, minor, patch.
    pub driver_version: u32,
    pub driver_name: Option<String>,
    pub driver_info: Option<String>,
    pub driver_id: Option<i32>,
    pub render_node: Option<Node>,
    pub primary_node: Option<Node>,
    pub extensions: Vec<(&'static str, bool)>,
    pub timestamp_period_ns: f32,
    pub extension_list_error: Option<String>,
}

pub fn device_type_name(device_type: vk::PhysicalDeviceType) -> &'static str {
    match device_type {
        vk::PhysicalDeviceType::INTEGRATED_GPU => "integrated GPU",
        vk::PhysicalDeviceType::DISCRETE_GPU => "discrete GPU",
        vk::PhysicalDeviceType::VIRTUAL_GPU => "virtual GPU",
        vk::PhysicalDeviceType::CPU => "CPU",
        _ => "other",
    }
}

pub fn read(vulkan: &Vulkan, physical: vk::PhysicalDevice) -> DriverInfo {
    let (offered, extension_list_error) = match vulkan.device_extensions(physical) {
        Ok(offered) => (offered, None),
        Err(failure) => (Vec::new(), Some(failure.describe())),
    };
    let has = |name: &str| offered.iter().any(|offered| offered == name);
    let base = vulkan.properties(physical);
    let mut driver = vk::PhysicalDeviceDriverProperties::default();
    let mut drm = vk::PhysicalDeviceDrmPropertiesEXT::default();
    let has_drm = has("VK_EXT_physical_device_drm");
    let has_driver_properties =
        base.api_version >= vk::API_VERSION_1_2 || has("VK_KHR_driver_properties");
    {
        let mut properties = vk::PhysicalDeviceProperties2::default();
        if has_driver_properties {
            properties = properties.push_next(&mut driver);
        }
        if has_drm {
            properties = properties.push_next(&mut drm);
        }
        // SAFETY: `physical` comes from this instance (1.3, so properties2 is core); each chained
        // structure is one the device supports, checked above; the chain lives for the call.
        unsafe {
            vulkan
                .instance
                .get_physical_device_properties2(physical, &mut properties)
        };
    }
    let text = |result: Result<&std::ffi::CStr, _>| -> Option<String> {
        result
            .ok()
            .map(|name: &std::ffi::CStr| name.to_string_lossy().into_owned())
    };
    let extensions = EXTENSIONS
        .iter()
        .map(|&name| {
            // Timeline semaphores are core from Vulkan 1.2 on.
            let core =
                name == "VK_KHR_timeline_semaphore" && base.api_version >= vk::API_VERSION_1_2;
            (name, has(name) || core)
        })
        .collect();
    DriverInfo {
        device_name: text(base.device_name_as_c_str()).unwrap_or_default(),
        device_type: device_type_name(base.device_type),
        vendor_id: base.vendor_id,
        device_id: base.device_id,
        api_version: base.api_version,
        driver_version: base.driver_version,
        driver_name: has_driver_properties
            .then(|| text(driver.driver_name_as_c_str()))
            .flatten(),
        driver_info: has_driver_properties
            .then(|| text(driver.driver_info_as_c_str()))
            .flatten(),
        driver_id: has_driver_properties.then_some(driver.driver_id.as_raw()),
        render_node: (has_drm && drm.has_render == vk::TRUE).then_some(Node {
            major: drm.render_major,
            minor: drm.render_minor,
        }),
        primary_node: (has_drm && drm.has_primary == vk::TRUE).then_some(Node {
            major: drm.primary_major,
            minor: drm.primary_minor,
        }),
        extensions,
        timestamp_period_ns: base.limits.timestamp_period,
        extension_list_error,
    }
}

impl DriverInfo {
    pub fn to_json(&self) -> Json {
        let node = |node: Option<Node>| {
            Json::optional(node, |node| {
                Json::Object(vec![
                    ("major", Json::Integer(node.major)),
                    ("minor", Json::Integer(node.minor)),
                ])
            })
        };
        Json::Object(vec![
            ("device_name", Json::text(&self.device_name)),
            ("device_type", Json::text(self.device_type)),
            ("vendor_id", Json::text(format!("{:#06x}", self.vendor_id))),
            ("device_id", Json::text(format!("{:#06x}", self.device_id))),
            ("api_version", Json::text(version_text(self.api_version))),
            (
                "driver_version",
                Json::text(version_text(self.driver_version)),
            ),
            (
                "driver_version_raw",
                Json::Integer(i64::from(self.driver_version)),
            ),
            (
                "driver_name",
                Json::optional(self.driver_name.as_ref(), Json::text),
            ),
            (
                "driver_info",
                Json::optional(self.driver_info.as_ref(), Json::text),
            ),
            (
                "driver_id",
                Json::optional(self.driver_id, |id| Json::Integer(i64::from(id))),
            ),
            ("render_node", node(self.render_node)),
            ("primary_node", node(self.primary_node)),
            (
                "timestamp_period_ns",
                Json::Number(f64::from(self.timestamp_period_ns)),
            ),
            (
                "extensions",
                Json::Object(
                    self.extensions
                        .iter()
                        .map(|&(name, offered)| (name, Json::Bool(offered)))
                        .collect(),
                ),
            ),
            (
                "extension_list_error",
                Json::optional(self.extension_list_error.as_ref(), Json::text),
            ),
        ])
    }

    pub fn to_markdown(&self, out: &mut String) {
        out.push_str(&format!(
            "- Device: {} ({})\n",
            self.device_name, self.device_type
        ));
        out.push_str(&format!(
            "- Driver: {}, {}\n",
            self.driver_name.as_deref().unwrap_or("unknown"),
            self.driver_info.as_deref().unwrap_or("no driver info"),
        ));
        out.push_str(&format!(
            "- Vulkan {}; driver version {}\n",
            version_text(self.api_version),
            version_text(self.driver_version)
        ));
        out.push_str(&format!(
            "- PCI ids: vendor {:#06x}, device {:#06x}\n",
            self.vendor_id, self.device_id
        ));
        let node = |node: Option<Node>| {
            node.map_or_else(
                || "none".to_owned(),
                |node| format!("{}:{}", node.major, node.minor),
            )
        };
        out.push_str(&format!(
            "- Render node {}, primary node {}\n",
            node(self.render_node),
            node(self.primary_node)
        ));
        out.push_str(&format!(
            "- Timestamp period: {} ns\n",
            self.timestamp_period_ns
        ));
        if let Some(error) = &self.extension_list_error {
            out.push_str(&format!("- The extensions could not be listed: {error}\n"));
        }
        out.push_str("\n| Extension | Offered |\n|---|---|\n");
        for &(name, offered) in &self.extensions {
            let note = if name == "VK_KHR_timeline_semaphore" && offered {
                " (or core 1.2)"
            } else {
                ""
            };
            out.push_str(&format!(
                "| `{name}` | {}{note} |\n",
                if offered { "yes" } else { "no" }
            ));
        }
        out.push('\n');
    }
}
