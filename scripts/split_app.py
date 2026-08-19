from pathlib import Path
import re

root = Path(__file__).resolve().parents[1] / "src" / "app"
text = (root / "mod.rs").read_text(encoding="utf-8")

# Theme constants block
const_block = re.search(
    r"(const BG: Color32.*?const UI_SLOW_REPAINT_INTERVAL: Duration = Duration::from_millis\(120\);)",
    text,
    re.S,
).group(1)
const_block = const_block.replace("const ", "pub(crate) const ")

panel_match = re.search(r"    fn panel\(ui: &mut Ui.*?\n    \}", text, re.S)
panel_fn = panel_match.group(0).replace("    fn panel", "pub(crate) fn panel")

theme_rs = (
    "//! egui theme tokens and layout helpers.\n\n"
    "use eframe::egui::{self, Color32, CornerRadius, Frame, Ui, Vec2};\n\n"
    + const_block
    + "\n\n"
    + panel_fn
    + """

pub(crate) fn truncate(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
"""
)
(root / "theme.rs").write_text(theme_rs, encoding="utf-8")

# UiMsg enum
ui_msg = re.search(r"enum UiMsg \{.*?\n\}", text, re.S).group(0)
messages_rs = (
    "//! Background → UI channel messages.\n\n"
    "use crate::{output::OutputDevice, stations::Station, voice::VoiceSearchResult};\n\n"
    + ui_msg.replace("enum UiMsg", "pub(super) enum UiMsg")
    + """

pub(super) fn same_output_device(left: &OutputDevice, right: &OutputDevice) -> bool {
    match (left, right) {
        (OutputDevice::Local(a), OutputDevice::Local(b)) => a.id == b.id,
        (OutputDevice::Cast(a), OutputDevice::Cast(b)) => a.discovered.host == b.discovered.host,
        _ => false,
    }
}
"""
)
(root / "messages.rs").write_text(messages_rs, encoding="utf-8")

struct_start = text.index("pub struct RockCastApp")
struct_end = text.index("impl RockCastApp", struct_start)
struct_block = text[struct_start:struct_end]
struct_block = re.sub(
    r"^(\s+)([a-z_][a-z0-9_]*:)", r"\1pub(super) \2", struct_block, flags=re.M
)

impl_start = text.index("impl RockCastApp {")
eframe_impl = text.index("impl eframe::App for RockCastApp")
drop_impl = text.index("impl Drop for RockCastApp")
first_impl = text[impl_start:eframe_impl]

ui_methods = [
    "draw_rockserver_panel",
    "draw_device_row",
    "draw_station_list",
    "draw_now_playing",
    "draw_controls",
    "draw_status",
    "draw_eq",
    "panel",
]
action_methods = [
    "restore_station_selection",
    "restore_device_selection",
    "mark_settings_dirty",
    "persist_settings_if_needed",
    "queue_volume",
    "shutdown_playback",
    "bootstrap",
    "poll_messages",
    "refresh_stations",
    "refresh_devices",
    "start_voice",
    "stop_voice_recording",
    "play",
    "stop",
    "apply_volume_if_needed",
    "schedule_stream_tap",
    "sync_spectrum",
    "set_eq_enabled",
    "set_cast_relay",
    "tick_eq",
    "eq_ui_needs_frames",
    "set_language",
]
keep_methods = ["new", "can_start_play"]


def extract_method(src: str, name: str):
    pat = rf"\n    (?:pub )?fn {name}\("
    m = re.search(pat, src)
    if not m:
        raise SystemExit(f"missing method {name}")
    start = m.start() + 1
    i = m.end()
    depth = 0
    started = False
    while i < len(src):
        c = src[i]
        if c == "{":
            depth += 1
            started = True
        elif c == "}":
            depth -= 1
            if started and depth == 0:
                end = i + 1
                return src[start:end], src[:start] + src[end:]
        i += 1
    raise SystemExit(f"unclosed method {name}")


remaining = first_impl
extracted = {}
for name in ui_methods + action_methods + keep_methods:
    body, remaining = extract_method(remaining, name)
    extracted[name] = body

# Clean remaining impl wrapper debris
remaining = remaining.replace("impl RockCastApp {", "").strip()
if remaining.startswith("}"):
    remaining = remaining[1:].strip()

actions_body = "\n".join(extracted[m] for m in action_methods)
(root / "actions.rs").write_text(
    f"""//! Playback, settings, and catalog actions.

use std::{{
    sync::{{Arc, atomic::{{AtomicBool, Ordering}}}},
    time::{{Duration, Instant}},
}};

use crate::{{
    i18n,
    output::scan_streaming,
    playback::PlaybackEvent,
    stations::{{enrich_stations, load_catalog}},
    voice,
    voice_prompts,
}};

use super::{{messages::{{UiMsg, same_output_device}}, theme::*, RockCastApp}};

impl RockCastApp {{
{actions_body}
}}
""",
    encoding="utf-8",
)

ui_dir = root / "ui"
ui_dir.mkdir(exist_ok=True)
ui_groups = {
    "rockserver.rs": ["draw_rockserver_panel"],
    "devices.rs": ["draw_device_row"],
    "stations.rs": ["draw_station_list"],
    "controls.rs": ["draw_now_playing", "draw_controls", "draw_status"],
    "eq.rs": ["draw_eq"],
}
for fname, methods in ui_groups.items():
    body = "\n".join(extracted[m] for m in methods)
    (ui_dir / fname).write_text(
        f"""//! egui panel widgets.

use eframe::egui::{{
    self, Align, Color32, CornerRadius, Frame, Layout, Pos2, Rect, RichText, Sense, Stroke, Ui,
    Vec2,
}};

use super::super::{{theme::*, RockCastApp}};

impl RockCastApp {{
{body}
}}
""",
        encoding="utf-8",
    )
(ui_dir / "mod.rs").write_text(
    "//! egui panels.\n\n" + "\n".join(f"mod {f[:-3]};" for f in ui_groups) + "\n",
    encoding="utf-8",
)

mod_rs = (
    "//! RockCast GUI on egui.\n\n"
    "mod actions;\nmod messages;\nmod theme;\nmod ui;\n\n"
    "use std::{\n"
    "    collections::VecDeque,\n"
    "    sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc},\n"
    "    time::{Duration, Instant},\n"
    "};\n\n"
    "use eframe::egui::{self, Align, Color32, Frame, Layout, RichText, Stroke, Vec2};\n\n"
    "use crate::{\n"
    "    i18n::{self, Lang},\n"
    "    observers::{StreamObservers, BANDS},\n"
    "    output::OutputDevice,\n"
    "    playback::PlaybackController,\n"
    "    playback_diag,\n"
    "    settings::AppSettings,\n"
    "    stations::Station,\n"
    "    telemetry::{PlaybackSnapshot, Telemetry},\n"
    "};\n\n"
    "use messages::UiMsg;\n"
    "use theme::{EQ_REPAINT_INTERVAL, UI_SLOW_REPAINT_INTERVAL, ACCENT, BG, FG, MUTED, PANEL, PANEL_2};\n\n"
    + struct_block
    + "\nimpl RockCastApp {\n"
    + extracted["new"]
    + "\n"
    + extracted["can_start_play"]
    + "\n}\n\n"
    + text[eframe_impl:drop_impl]
    + "\n"
    + text[drop_impl : text.index("fn truncate")]
)
(root / "mod.rs").write_text(mod_rs, encoding="utf-8")
print("split complete")
