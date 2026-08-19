//! Symphonia codec registry: Fraunhofer FDK-AAC (HE-AAC) + other enabled decoders.

use std::sync::OnceLock;

use symphonia::core::codecs::CodecRegistry;
use symphonia_adapter_fdk_aac::AacDecoder as FdkAacDecoder;

/// Codecs for live decode. Uses official libfdk-aac for AAC/HE-AAC instead of symphonia's pure-Rust AAC-LC.
pub fn get_codecs() -> &'static CodecRegistry {
    static REGISTRY: OnceLock<CodecRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = CodecRegistry::new();
        registry.register_all::<FdkAacDecoder>();
        registry.register_all::<symphonia::default::codecs::FlacDecoder>();
        registry.register_all::<symphonia::default::codecs::MpaDecoder>();
        registry.register_all::<symphonia::default::codecs::PcmDecoder>();
        registry.register_all::<symphonia::default::codecs::VorbisDecoder>();
        registry
    })
}
