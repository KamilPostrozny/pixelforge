//! `Encoder::readback_thread_id` names the encoder's own readback thread, so a
//! caller can set that one thread's scheduling. Needs a video-encode capable GPU,
//! so the test is ignored by default.

#[allow(dead_code)]
mod common;

use pixelforge::{Codec, EncodeConfig, Encoder, VideoContextBuilder};

#[test]
#[ignore = "requires a Vulkan Video device"]
fn readback_thread_id_is_the_readback_thread() {
    common::init_logging();
    let context = VideoContextBuilder::new()
        .app_name("pixelforge-test")
        .build()
        .expect("no Vulkan video device");
    let codec = [Codec::H265, Codec::H264, Codec::AV1]
        .into_iter()
        .find(|&codec| context.supports_encode(codec))
        .expect("no encodable codec");
    let config = match codec {
        Codec::H264 => EncodeConfig::h264(320, 240),
        Codec::H265 => EncodeConfig::h265(320, 240),
        Codec::AV1 => EncodeConfig::av1(320, 240),
    };
    let encoder = Encoder::new(context, config).expect("encoder");

    let tid = encoder.readback_thread_id().expect("readback thread id");
    let comm = std::fs::read_to_string(format!("/proc/self/task/{tid}/comm")).expect("comm");
    // Linux keeps 15 bytes of a thread's name.
    assert_eq!(comm.trim_end(), "pixelforge-enco");

    // The thread ends with the encoder.
    drop(encoder);
    assert!(!std::path::Path::new(&format!("/proc/self/task/{tid}")).exists());
}
