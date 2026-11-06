//! A context asked for the highest global queue priority gets the highest one
//! the driver grants, and never fails over a priority it is refused.
//!
//! On Linux, amdgpu grants a context above the normal priority only to a
//! process with `CAP_SYS_NICE` (or the DRM master). So the expected priority on
//! RADV follows from the test process's capabilities: realtime with
//! `CAP_SYS_NICE`, medium (the default) without it. Other drivers have their
//! own rules, so on them the test checks only that the context works and that
//! it reports a priority no lower than the default.
//!
//! Ignored by default: both tests encode on a Vulkan Video device. Run with
//! `cargo test --test queue_priority -- --ignored`. Like `tests/context.rs`,
//! they skip where no Vulkan driver is present.

#[allow(dead_code)]
mod common;

use ash::vk;
use ash::vk::TaggedStructure;
use pixelforge::encoder::{Codec, EncodeConfig, Encoder, RateControlMode};
use pixelforge::error::PixelForgeError;
use pixelforge::vulkan::{QueueGlobalPriority, VideoContext, VideoContextBuilder};

/// Whether this process holds `CAP_SYS_NICE` (bit 23) in its effective set.
fn has_cap_sys_nice() -> bool {
    const CAP_SYS_NICE: u32 = 23;
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    status
        .lines()
        .find_map(|line| line.strip_prefix("CapEff:"))
        .and_then(|hex| u64::from_str_radix(hex.trim(), 16).ok())
        .is_some_and(|caps| caps & (1 << CAP_SYS_NICE) != 0)
}

fn is_radv(context: &VideoContext) -> bool {
    let mut driver = vk::PhysicalDeviceDriverProperties::default();
    let mut properties = vk::PhysicalDeviceProperties2::default().push(&mut driver);
    unsafe {
        context
            .instance()
            .get_physical_device_properties2(context.physical_device(), &mut properties);
    }
    driver.driver_id == vk::DriverId::MESA_RADV
}

/// Build an H.264 encode context, or `None` (after saying why) where this
/// machine has no driver or no encoder to test with.
fn encode_context(builder: VideoContextBuilder, test: &str) -> Option<VideoContext> {
    match builder
        .app_name("pixelforge-test")
        .enable_validation(std::env::var("PIXELFORGE_VALIDATION").is_ok())
        .require_encode(Codec::H264)
        .build()
    {
        Ok(context) => Some(context),
        Err(
            error @ (PixelForgeError::InstanceCreation(_) | PixelForgeError::NoSuitableDevice(_)),
        ) => {
            assert!(
                std::env::var("PIXELFORGE_REQUIRE_VULKAN").is_err(),
                "{test}: PIXELFORGE_REQUIRE_VULKAN is set but no H.264 encoder is usable: {error}"
            );
            eprintln!("skipping {test}: no H.264 encoder available ({error})");
            None
        }
        Err(other) => panic!("{test}: building the context failed: {other}"),
    }
}

/// Encode a few frames, which submits to the encode queue and waits on it.
fn encode_frames(context: &VideoContext) -> Result<(), Box<dyn std::error::Error>> {
    let mut encoder = Encoder::new(
        context.clone(),
        EncodeConfig::h264(320, 240)
            .with_rate_control(RateControlMode::Cqp)
            .with_b_frames(0),
    )?;
    for _ in 0..3 {
        let target = encoder.input_image();
        let packet = pollster::block_on(encoder.encode(target)?)?;
        assert!(!packet.data.is_empty(), "an encoded frame has bytes");
    }
    Ok(())
}

#[test]
#[ignore = "requires a Vulkan Video device"]
fn highest_queue_priority_gets_what_the_driver_grants() -> Result<(), Box<dyn std::error::Error>> {
    common::init_logging();
    let test = "highest_queue_priority_gets_what_the_driver_grants";
    let Some(context) = encode_context(VideoContextBuilder::new().highest_queue_priority(), test)
    else {
        return Ok(());
    };

    let priority = context.queue_priority();
    if is_radv(&context) {
        let expected = if has_cap_sys_nice() {
            QueueGlobalPriority::Realtime
        } else {
            QueueGlobalPriority::Medium
        };
        assert_eq!(priority, Some(expected), "RADV has VK_KHR_global_priority");
    } else if let Some(priority) = priority {
        assert!(priority >= QueueGlobalPriority::Medium, "{priority}");
    }

    // The queues work at whatever priority they got.
    encode_frames(&context)
}

#[test]
#[ignore = "requires a Vulkan Video device"]
fn a_context_not_asked_for_a_priority_reports_none() -> Result<(), Box<dyn std::error::Error>> {
    common::init_logging();
    let test = "a_context_not_asked_for_a_priority_reports_none";
    let Some(context) = encode_context(VideoContextBuilder::new(), test) else {
        return Ok(());
    };
    assert_eq!(context.queue_priority(), None);
    encode_frames(&context)
}
