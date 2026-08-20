//! Re-rank RockServer voice candidates using transcript keywords.

use crate::stations::Station;

pub(super) fn rerank_voice_candidates(transcript: &str, stations: &mut Vec<Station>) {
    // Keep it simple: split transcript into words, reward stations whose `name`
    // or `tags` contain those words.
    let stop_words: &[&str] = &[
        "радио",
        "станцию",
        "станции",
        "включи",
        "включить",
        "поставь",
        "поставить",
        "запусти",
        "найди",
        "ищи",
        "найди",
        "крути",
        "пожалуйста",
        "команду",
    ];

    let terms: Vec<String> = transcript
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 3)
        .filter(|t| !stop_words.contains(&t.as_ref()))
        .map(|s| s.to_string())
        .collect();

    if terms.is_empty() {
        return;
    }

    let original = std::mem::take(stations);
    let mut scored: Vec<(usize, i32, Station)> = original
        .into_iter()
        .enumerate()
        .map(|(idx, s)| {
            let name = s.name.to_lowercase();
            let tags = s.tags.to_lowercase();
            let mut score: i32 = 0;
            for t in &terms {
                if name == *t {
                    score += 120;
                } else if name.contains(t) {
                    score += 60;
                }
                if tags.contains(t) {
                    score += 15;
                }
            }
            // Small bias: if transcript contains station name as a whole substring,
            // keep it near the top.
            if transcript.to_lowercase().contains(&s.name.to_lowercase()) {
                score += 30;
            }
            (idx, score, s)
        })
        .collect();

    scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    *stations = scored.into_iter().map(|(_, _, s)| s).collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stations::Station;

    #[test]
    fn rerank_prefers_station_name_match() {
        let mut stations = vec![
            Station {
                name: "Наше радио".into(),
                url: "https://example.com/1".into(),
                tags: "rock".into(),
                country: String::new(),
                bitrate: 0,
                codec: "mp3".into(),
            },
            Station {
                name: "Рокс".into(),
                url: "https://example.com/2".into(),
                tags: "rock".into(),
                country: String::new(),
                bitrate: 0,
                codec: "mp3".into(),
            },
        ];

        rerank_voice_candidates("Поставь радио рокс", &mut stations);
        assert_eq!(stations[0].name, "Рокс");
    }
}
