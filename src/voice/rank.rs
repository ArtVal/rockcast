//! Re-rank RockServer voice candidates using transcript keywords.

use crate::stations::Station;

const STOP_WORDS: &[&str] = &[
    "\u{0440}\u{0430}\u{0434}\u{0438}\u{043e}",
    "\u{0441}\u{0442}\u{0430}\u{043d}\u{0446}\u{0438}\u{044e}",
    "\u{0441}\u{0442}\u{0430}\u{043d}\u{0446}\u{0438}\u{0438}",
    "\u{0432}\u{043a}\u{043b}\u{044e}\u{0447}\u{0438}",
    "\u{0432}\u{043a}\u{043b}\u{044e}\u{0447}\u{0438}\u{0442}\u{044c}",
    "\u{043f}\u{043e}\u{0441}\u{0442}\u{0430}\u{0432}\u{044c}",
    "\u{043f}\u{043e}\u{0441}\u{0442}\u{0430}\u{0432}\u{0438}\u{0442}\u{044c}",
    "\u{0437}\u{0430}\u{043f}\u{0443}\u{0441}\u{0442}\u{0438}",
    "\u{043d}\u{0430}\u{0439}\u{0434}\u{0438}",
    "\u{0438}\u{0449}\u{0438}",
    "\u{043a}\u{0440}\u{0443}\u{0442}\u{0438}",
    "\u{043f}\u{043e}\u{0436}\u{0430}\u{043b}\u{0443}\u{0439}\u{0441}\u{0442}\u{0430}",
    "\u{043a}\u{043e}\u{043c}\u{0430}\u{043d}\u{0434}\u{0443}",
];

const COMMAND_WORDS: &[&str] = &[
    "\u{0432}\u{043a}\u{043b}\u{044e}\u{0447}\u{0438}",
    "\u{0432}\u{043a}\u{043b}\u{044e}\u{0447}\u{0438}\u{0442}\u{044c}",
    "\u{043f}\u{043e}\u{0441}\u{0442}\u{0430}\u{0432}\u{044c}",
    "\u{043f}\u{043e}\u{0441}\u{0442}\u{0430}\u{0432}\u{0438}\u{0442}\u{044c}",
    "\u{0437}\u{0430}\u{043f}\u{0443}\u{0441}\u{0442}\u{0438}",
    "\u{043d}\u{0430}\u{0439}\u{0434}\u{0438}",
    "\u{0438}\u{0449}\u{0438}",
    "\u{043a}\u{0440}\u{0443}\u{0442}\u{0438}",
    "\u{043f}\u{043e}\u{0436}\u{0430}\u{043b}\u{0443}\u{0439}\u{0441}\u{0442}\u{0430}",
];

pub(super) fn rerank_voice_candidates(transcript: &str, stations: &mut Vec<Station>) {
    let lower_transcript = transcript.to_lowercase();
    let terms: Vec<String> = split_words(&lower_transcript)
        .filter(|term| term.len() >= 3)
        .filter(|term| !STOP_WORDS.contains(term))
        .map(str::to_owned)
        .collect();
    let station_phrase: Vec<String> = split_words(&lower_transcript)
        .filter(|term| !COMMAND_WORDS.contains(term))
        .map(phonetic_token)
        .filter(|term| !term.is_empty())
        .collect();

    if terms.is_empty() && station_phrase.is_empty() {
        return;
    }

    let original = std::mem::take(stations);
    let mut scored: Vec<(usize, i32, Station)> = original
        .into_iter()
        .enumerate()
        .map(|(idx, s)| {
            let name = s.name.to_lowercase();
            let tags = s.tags.to_lowercase();
            let mut score = station_prefix_score(&station_phrase, &s.name);
            for term in &terms {
                if name == *term {
                    score += 120;
                } else if name.contains(term) {
                    score += 60;
                }
                if tags.contains(term) {
                    score += 15;
                }
            }
            if lower_transcript.contains(&name) {
                score += 30;
            }
            (idx, score, s)
        })
        .collect();

    scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    *stations = scored.into_iter().map(|(_, _, station)| station).collect();
}

fn split_words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|term| !term.is_empty())
}

/// Transliterates just enough to compare Russian speech recognition with Latin station names.
fn phonetic_token(token: &str) -> String {
    let mut normalized = String::with_capacity(token.len());
    for character in token.chars() {
        normalized.push_str(match character {
            '\u{0430}' => "a",
            '\u{0431}' => "b",
            '\u{0432}' => "v",
            '\u{0433}' => "g",
            '\u{0434}' => "d",
            '\u{0435}' | '\u{0451}' | '\u{044d}' => "e",
            '\u{0436}' => "zh",
            '\u{0437}' => "z",
            '\u{0438}' | '\u{0439}' => "i",
            '\u{043a}' => "k",
            '\u{043b}' => "l",
            '\u{043c}' => "m",
            '\u{043d}' => "n",
            '\u{043e}' => "o",
            '\u{043f}' => "p",
            '\u{0440}' => "r",
            '\u{0441}' => "s",
            '\u{0442}' => "t",
            '\u{0443}' => "u",
            '\u{0444}' => "f",
            '\u{0445}' => "h",
            '\u{0446}' => "ts",
            '\u{0447}' => "ch",
            '\u{0448}' | '\u{0449}' => "sh",
            '\u{044a}' | '\u{044c}' => "",
            '\u{044b}' => "y",
            '\u{044e}' => "yu",
            '\u{044f}' => "ya",
            _ => {
                normalized.push(character);
                continue;
            }
        });
    }
    normalized
}

fn station_prefix_score(spoken_phrase: &[String], station_name: &str) -> i32 {
    // A single term such as "rock" is a genre, not a station name. Requiring two
    // words prevents it from overwhelming RockServer's normal relevance ranking.
    if spoken_phrase.len() < 2 {
        return 0;
    }

    let station_words: Vec<String> = split_words(&station_name.to_lowercase())
        .map(phonetic_token)
        .filter(|term| !term.is_empty())
        .collect();
    if station_words.len() < spoken_phrase.len() {
        return 0;
    }

    let last = spoken_phrase.len() - 1;
    let matches_prefix = spoken_phrase[..last] == station_words[..last]
        && (station_words[last] == spoken_phrase[last]
            || station_words[last].starts_with(&spoken_phrase[last]));
    if matches_prefix { 500 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stations::Station;

    fn station(name: &str) -> Station {
        Station::from_primary(
            "test".into(),
            name.into(),
            "https://example.com/stream".into(),
            "rock".into(),
            String::new(),
            0,
            "mp3".into(),
        )
    }

    #[test]
    fn rerank_promotes_latin_radio_roks_after_cyrillic_voice_transcript() {
        let mut stations = vec![
            station(
                "\u{041d}\u{0430}\u{0448}\u{0435} \u{0420}\u{0430}\u{0434}\u{0438}\u{043e} \u{041a}\u{043b}\u{0430}\u{0441}\u{0441}\u{0438}\u{043a} \u{0420}\u{043e}\u{043a}",
            ),
            station("Radio ROKS Classic Rock"),
            station("\u{0420}\u{0430}\u{0434}\u{0438}\u{043e} ROKS"),
        ];

        rerank_voice_candidates(
            "\u{0432}\u{043a}\u{043b}\u{044e}\u{0447}\u{0438} \u{0440}\u{0430}\u{0434}\u{0438}\u{043e} \u{0440}\u{043e}\u{043a}",
            &mut stations,
        );

        assert_eq!(stations[0].name, "Radio ROKS Classic Rock");
    }

    #[test]
    fn rerank_keeps_server_order_when_the_final_sound_is_ambiguous() {
        let mut stations = vec![
            station("Radio ROKS Classic Rock"),
            station("\u{0420}\u{0430}\u{0434}\u{0438}\u{043e} ROKS"),
        ];

        rerank_voice_candidates(
            "\u{0432}\u{043a}\u{043b}\u{044e}\u{0447}\u{0438} \u{0440}\u{0430}\u{0434}\u{0438}\u{043e} \u{0440}\u{043e}\u{043a}\u{0441}",
            &mut stations,
        );

        assert_eq!(stations[0].name, "Radio ROKS Classic Rock");
    }
}
