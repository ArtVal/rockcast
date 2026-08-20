//! UI strings: Russian / English.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    #[default]
    Ru,
    En,
}

impl Lang {
    pub fn native_name(self) -> &'static str {
        match self {
            Self::Ru => "Русский",
            Self::En => "English",
        }
    }

    pub fn t(self) -> &'static Strings {
        match self {
            Self::Ru => &RU,
            Self::En => &EN,
        }
    }
}

pub struct Strings {
    pub window_title: &'static str,
    pub subtitle: &'static str,
    pub menu_language: &'static str,
    pub device: &'static str,
    pub find: &'static str,
    pub searching: &'static str,
    pub device_none: &'static str,
    pub nothing_found: &'static str,
    pub stations: &'static str,
    pub refresh: &'static str,
    pub col_station: &'static str,
    pub col_tags: &'static str,
    pub col_country: &'static str,
    pub col_bitrate: &'static str,
    pub loading_stations: &'static str,
    pub list_empty: &'static str,
    pub now_playing: &'static str,
    pub spectrum: &'static str,
    pub spectrum_hint: &'static str,
    pub volume: &'static str,
    pub loading: &'static str,
    pub track_hint: &'static str,
    pub track_meta_hint: &'static str,
    pub stopped: &'static str,
    pub loading_stations_status: &'static str,
    pub searching_devices: &'static str,
    pub scan_panic: &'static str,
    pub pick_station: &'static str,
    pub pick_device: &'static str,
    pub connecting: &'static str,
    pub stations_count: &'static str, // "{} stations" / RU equivalent
    pub this_pc: &'static str,
    pub pc_speakers: &'static str,
    pub local_catalog: &'static str, // "local catalog · {} stations"
    pub catalog_plus_rb: &'static str, // "catalog + Radio Browser · {} stations"
    pub cast_none: &'static str,     // "Local: {}. No Chromecast found..."
    pub cast_found: &'static str,    // "Found: {} local + {} Cast. Selected: {}"
    pub cast_err: &'static str,      // "Local: {}. Cast search: {}"
    pub cast_relay: &'static str,
    pub cast_relay_hint: &'static str,
    pub cast_relay_note: &'static str,
    pub cast_relay_restart_on: &'static str,
    pub cast_relay_restart_off: &'static str,
    pub rockserver: &'static str,
    pub rockserver_autonomous: &'static str,
    pub rockserver_configure: &'static str,
    pub rockserver_hide: &'static str,
    pub rockserver_url: &'static str,
    pub rockserver_token: &'static str,
    pub rockserver_token_hint: &'static str,
    pub rockserver_status_ok: &'static str,
    pub rockserver_status_need_token: &'static str,
    pub rockserver_token_required: &'static str,
}

pub static RU: Strings = Strings {
    window_title: "RockCast — радио локально / Chromecast",
    subtitle: "Рок и металл — локально или на Chromecast",
    menu_language: "Язык",
    device: "Устройство",
    find: "Найти",
    searching: "Поиск…",
    device_none: "Устройство не выбрано",
    nothing_found: "Ничего не найдено",
    stations: "Станции",
    refresh: "Обновить",
    col_station: "Станция",
    col_tags: "Теги",
    col_country: "Страна",
    col_bitrate: "Битрейт / кодек",
    loading_stations: "Загрузка станций…",
    list_empty: "Список пуст",
    now_playing: "Сейчас играет",
    spectrum: "Спектр",
    spectrum_hint: "Анализ аудиопотока для эквалайзера (дополнительный трафик)",
    volume: "Громкость",
    loading: "Загрузка…",
    track_hint: "Трек появится после Play (если станция отдаёт метаданные)",
    track_meta_hint: "Метаданные трека появятся, если станция их отдаёт",
    stopped: "Остановлено",
    loading_stations_status: "Загрузка станций…",
    searching_devices: "Поиск устройств (локальные + Chromecast)…",
    scan_panic: "Ошибка поиска устройств. Нажмите «Найти» ещё раз.",
    pick_station: "Выберите радиостанцию в списке.",
    pick_device: "Сначала выберите устройство (локальные динамики или Cast).",
    connecting: "Подключение…",
    stations_count: "Станций: {}",
    this_pc: "Этот ПК",
    pc_speakers: "Динамики компьютера",
    local_catalog: "локальный каталог · {} станций",
    catalog_plus_rb: "каталог + Radio Browser · {} станций",
    cast_none: "Локальных: {}. Chromecast не найдены (проверьте Wi‑Fi / JBL).",
    cast_found: "Найдено: {} локальных + {} Cast. Выбрано: {}",
    cast_err: "Локальных: {}. Поиск Cast: {}",
    cast_relay: "Через ПК",
    cast_relay_hint: "ПК качает поток (в т.ч. через VPN) и отдаёт колонке по Wi‑Fi. При переключении во время Play стрим перезапускается.",
    cast_relay_note: "ПК -> колонка по Wi‑Fi (если станции нужны через VPN)",
    cast_relay_restart_on: "Включаю трансляцию через ПК…",
    cast_relay_restart_off: "Прямой Cast без ретранслятора…",
    rockserver: "RockServer (поиск и голос)",
    rockserver_autonomous: "Автономный режим: локальный каталог и Radio Browser",
    rockserver_configure: "Настроить",
    rockserver_hide: "Скрыть",
    rockserver_url: "URL",
    rockserver_token: "Токен",
    rockserver_token_hint: "Токен сохраняется только в локальных настройках RockCast.",
    rockserver_status_ok: "Токен сохранён",
    rockserver_status_need_token: "Нужен токен для поиска и голоса",
    rockserver_token_required: "Укажите токен RockServer в настройках.",
};

pub static EN: Strings = Strings {
    window_title: "RockCast — local radio / Chromecast",
    subtitle: "Rock and metal — local speakers or Chromecast",
    menu_language: "Language",
    device: "Device",
    find: "Find",
    searching: "Searching…",
    device_none: "No device selected",
    nothing_found: "Nothing found",
    stations: "Stations",
    refresh: "Refresh",
    col_station: "Station",
    col_tags: "Tags",
    col_country: "Country",
    col_bitrate: "Bitrate / codec",
    loading_stations: "Loading stations…",
    list_empty: "List is empty",
    now_playing: "Now playing",
    spectrum: "Spectrum",
    spectrum_hint: "Analyze the audio stream for the equalizer (extra traffic)",
    volume: "Volume",
    loading: "Loading…",
    track_hint: "Track info appears after Play (if the station provides metadata)",
    track_meta_hint: "Track metadata will appear if the station provides it",
    stopped: "Stopped",
    loading_stations_status: "Loading stations…",
    searching_devices: "Searching devices (local + Chromecast)…",
    scan_panic: "Device scan failed. Press Find again.",
    pick_station: "Select a radio station from the list.",
    pick_device: "Select a device first (PC speakers or Cast).",
    connecting: "Connecting…",
    stations_count: "Stations: {}",
    this_pc: "This PC",
    pc_speakers: "Computer speakers",
    local_catalog: "local catalog · {} stations",
    catalog_plus_rb: "catalog + Radio Browser · {} stations",
    cast_none: "Local: {}. No Chromecast found (check Wi‑Fi / JBL).",
    cast_found: "Found: {} local + {} Cast. Selected: {}",
    cast_err: "Local: {}. Cast search: {}",
    cast_relay: "Via PC",
    cast_relay_hint: "PC fetches the stream (e.g. through VPN) and serves it to the speaker on Wi‑Fi. Toggling while playing restarts the stream.",
    cast_relay_note: "PC -> speaker on Wi-Fi (when stations need VPN)",
    cast_relay_restart_on: "Switching to Via PC relay…",
    cast_relay_restart_off: "Switching to direct Cast…",
    rockserver: "RockServer (search & voice)",
    rockserver_autonomous: "Standalone: local catalog and Radio Browser",
    rockserver_configure: "Configure",
    rockserver_hide: "Hide",
    rockserver_url: "URL",
    rockserver_token: "Token",
    rockserver_token_hint: "The token is stored only in RockCast local settings.",
    rockserver_status_ok: "Token saved",
    rockserver_status_need_token: "Token required for search and voice",
    rockserver_token_required: "Set the RockServer token in settings.",
};

/// Simple `{}` placeholder replace (one or more, left-to-right).
pub fn fmt1(template: &str, a: impl std::fmt::Display) -> String {
    template.replacen("{}", &a.to_string(), 1)
}

pub fn fmt2(template: &str, a: impl std::fmt::Display, b: impl std::fmt::Display) -> String {
    template
        .replacen("{}", &a.to_string(), 1)
        .replacen("{}", &b.to_string(), 1)
}

pub fn fmt3(
    template: &str,
    a: impl std::fmt::Display,
    b: impl std::fmt::Display,
    c: impl std::fmt::Display,
) -> String {
    template
        .replacen("{}", &a.to_string(), 1)
        .replacen("{}", &b.to_string(), 1)
        .replacen("{}", &c.to_string(), 1)
}
