//! Embedded UI icon textures.

use eframe::egui::{ColorImage, Context, TextureHandle, TextureOptions};

/// Embedded static UI icon handles.
#[derive(Clone)]
pub(crate) struct AppIcons {
    pub logo: TextureHandle,
    pub search: TextureHandle,
    pub mic: TextureHandle,
    pub speaker: TextureHandle,
    pub speaker_mute: TextureHandle,
    pub clear: TextureHandle,
    pub pc: TextureHandle,
    pub phone: TextureHandle,
    pub arrow_up: TextureHandle,
    pub locate: TextureHandle,
}

impl AppIcons {
    /// Loads all embedded application icon textures into the egui context.
    pub fn new(ctx: &Context) -> Self {
        Self {
            logo: load_png(
                ctx,
                "icon_logo",
                include_bytes!("../../assets/icon_logo.png"),
            ),
            search: load_png(
                ctx,
                "icon_search",
                include_bytes!("../../assets/icon_search.png"),
            ),
            mic: load_png(ctx, "icon_mic", include_bytes!("../../assets/icon_mic.png")),
            speaker: load_png(
                ctx,
                "icon_speaker",
                include_bytes!("../../assets/icon_speaker.png"),
            ),
            speaker_mute: load_png(
                ctx,
                "icon_speaker_mute",
                include_bytes!("../../assets/icon_speaker_mute.png"),
            ),
            clear: load_png(
                ctx,
                "icon_clear",
                include_bytes!("../../assets/icon_clear.png"),
            ),
            pc: load_png(ctx, "icon_pc", include_bytes!("../../assets/icon_pc.png")),
            phone: load_png(
                ctx,
                "icon_phone",
                include_bytes!("../../assets/icon_phone.png"),
            ),
            arrow_up: load_png(
                ctx,
                "icon_arrow_up",
                include_bytes!("../../assets/icon_arrow_up.png"),
            ),
            locate: load_png(
                ctx,
                "icon_locate",
                include_bytes!("../../assets/icon_locate.png"),
            ),
        }
    }
}

fn load_png(ctx: &Context, name: &str, bytes: &[u8]) -> TextureHandle {
    let img = image::load_from_memory(bytes)
        .unwrap_or_else(|e| panic!("failed to decode embedded png icon {name}: {e}"))
        .to_rgba8();
    let (w, h) = img.dimensions();
    let color_image = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw());
    ctx.load_texture(name, color_image, TextureOptions::LINEAR)
}
