//! Global queue priority (`VK_KHR_global_priority`) for a context's own device.
//!
//! A queue's global priority decides how the driver and kernel schedule its
//! work against every other process's queues on the GPU, which is what keeps a
//! latency-sensitive encode from queueing behind a GPU-bound game. Above the
//! default it is a privilege: on Linux, amdgpu grants it only to a process
//! with `CAP_SYS_NICE` (or the DRM master). RADV (Mesa 25.3 and later) lists
//! only the priorities the process may use in its queue family properties;
//! older drivers, and those without the query, refuse at `vkCreateDevice`
//! with `VK_ERROR_NOT_PERMITTED`. So a context that asks for the highest
//! priority tries every one the families list, from the top down, and keeps
//! the first the driver accepts, ending at the default, which needs no
//! privilege.

use super::has_extension;
use ash::vk;
use ash::vk::TaggedStructure;
use std::ffi::CStr;
use std::fmt;

/// A queue's global priority, lowest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QueueGlobalPriority {
    /// Below the default.
    Low,
    /// The default for a queue created without a global priority.
    Medium,
    /// Above the default.
    High,
    /// The highest.
    Realtime,
}

impl QueueGlobalPriority {
    fn to_vk(self) -> vk::QueueGlobalPriority {
        match self {
            Self::Low => vk::QueueGlobalPriority::LOW,
            Self::Medium => vk::QueueGlobalPriority::MEDIUM,
            Self::High => vk::QueueGlobalPriority::HIGH,
            Self::Realtime => vk::QueueGlobalPriority::REALTIME,
        }
    }

    fn from_vk(priority: vk::QueueGlobalPriority) -> Option<Self> {
        match priority {
            vk::QueueGlobalPriority::LOW => Some(Self::Low),
            vk::QueueGlobalPriority::MEDIUM => Some(Self::Medium),
            vk::QueueGlobalPriority::HIGH => Some(Self::High),
            vk::QueueGlobalPriority::REALTIME => Some(Self::Realtime),
            _ => None,
        }
    }
}

impl fmt::Display for QueueGlobalPriority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Realtime => "realtime",
        })
    }
}

/// The extension to enable for global priorities on a device with
/// `extensions`, and whether its queue families can be asked which priorities
/// they support. `None` when the device has neither the KHR nor the EXT
/// extension.
pub(super) fn extension(extensions: &[vk::ExtensionProperties]) -> Option<(&'static CStr, bool)> {
    let has = |name: &CStr| has_extension(extensions, name);
    if has(ash::khr::global_priority::NAME) {
        Some((ash::khr::global_priority::NAME, true))
    } else if has(ash::ext::global_priority::NAME) {
        Some((
            ash::ext::global_priority::NAME,
            has(ash::ext::global_priority_query::NAME),
        ))
    } else {
        None
    }
}

/// The priorities each of `families` supports, as the device reports them.
pub(super) fn supported_by_families(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    families: &[u32],
) -> Vec<Vec<QueueGlobalPriority>> {
    let count =
        unsafe { instance.get_physical_device_queue_family_properties2_len(physical_device) };
    let mut priorities = vec![vk::QueueFamilyGlobalPriorityProperties::default(); count];
    {
        let mut properties: Vec<vk::QueueFamilyProperties2> = priorities
            .iter_mut()
            .map(|p| vk::QueueFamilyProperties2::default().push(p))
            .collect();
        unsafe {
            instance.get_physical_device_queue_family_properties2(physical_device, &mut properties)
        };
    }
    families
        .iter()
        .map(|&family| {
            priorities.get(family as usize).map_or_else(Vec::new, |p| {
                p.priorities[..(p.priority_count as usize).min(p.priorities.len())]
                    .iter()
                    .filter_map(|&priority| QueueGlobalPriority::from_vk(priority))
                    .collect()
            })
        })
        .collect()
}

/// The priorities to try, highest first, ending at the default.
///
/// With `supported` (one list per queue family the device will have), only
/// priorities above the default that every family supports; without it, all
/// of them.
pub(super) fn candidates(
    supported: Option<&[Vec<QueueGlobalPriority>]>,
) -> Vec<QueueGlobalPriority> {
    let mut out: Vec<QueueGlobalPriority> =
        [QueueGlobalPriority::Realtime, QueueGlobalPriority::High]
            .into_iter()
            .filter(|priority| {
                supported.is_none_or(|families| families.iter().all(|f| f.contains(priority)))
            })
            .collect();
    out.push(QueueGlobalPriority::Medium);
    out
}

/// Create a device with `create` at each of `candidates` in turn, moving on
/// only when the driver refuses one with `VK_ERROR_NOT_PERMITTED`. Returns the
/// device and the priority it got.
pub(super) fn create_at_highest<T>(
    candidates: &[QueueGlobalPriority],
    mut create: impl FnMut(vk::QueueGlobalPriority) -> Result<T, vk::Result>,
) -> Result<(T, QueueGlobalPriority), vk::Result> {
    for &priority in candidates {
        match create(priority.to_vk()) {
            Ok(created) => return Ok((created, priority)),
            Err(vk::Result::ERROR_NOT_PERMITTED) => {
                tracing::debug!("Global queue priority {priority} not permitted");
            }
            Err(e) => return Err(e),
        }
    }
    Err(vk::Result::ERROR_NOT_PERMITTED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use QueueGlobalPriority::*;

    #[test]
    fn candidates_are_highest_first_and_end_at_the_default() {
        assert_eq!(candidates(None), vec![Realtime, High, Medium]);
        let families = [vec![Low, Medium, High, Realtime], vec![Low, Medium, High]];
        assert_eq!(candidates(Some(&families)), vec![High, Medium]);
        // What RADV lists for a process without CAP_SYS_NICE.
        assert_eq!(
            candidates(Some(&[vec![Low, Medium], vec![Low, Medium]])),
            vec![Medium]
        );
    }

    #[test]
    fn a_refused_priority_falls_back_to_the_next() {
        let mut tried = Vec::new();
        let (device, granted) = create_at_highest(&[Realtime, High, Medium], |p| {
            tried.push(p);
            if p == vk::QueueGlobalPriority::MEDIUM {
                Ok("device")
            } else {
                Err(vk::Result::ERROR_NOT_PERMITTED)
            }
        })
        .unwrap();
        assert_eq!((device, granted), ("device", Medium));
        assert_eq!(
            tried,
            [
                vk::QueueGlobalPriority::REALTIME,
                vk::QueueGlobalPriority::HIGH,
                vk::QueueGlobalPriority::MEDIUM
            ]
        );
    }

    #[test]
    fn the_first_accepted_priority_is_kept() {
        let (_, granted) = create_at_highest(&[Realtime, High, Medium], |p| {
            if p == vk::QueueGlobalPriority::REALTIME {
                Err(vk::Result::ERROR_NOT_PERMITTED)
            } else {
                Ok(())
            }
        })
        .unwrap();
        assert_eq!(granted, High);
        let (_, granted) = create_at_highest(&[Realtime, High, Medium], |_| Ok(())).unwrap();
        assert_eq!(granted, Realtime);
    }

    #[test]
    fn other_errors_are_not_retried() {
        let mut calls = 0;
        let result = create_at_highest(&[Realtime, High, Medium], |_| -> Result<(), _> {
            calls += 1;
            Err(vk::Result::ERROR_DEVICE_LOST)
        });
        assert_eq!(result.unwrap_err(), vk::Result::ERROR_DEVICE_LOST);
        assert_eq!(calls, 1);
    }
}
