//! Station catalog sorting and audio codec inference.

use super::Station;
#[cfg(test)]
use std::collections::HashSet;

pub(crate) fn order_stations(stations: Vec<Station>) -> Vec<Station> {
    let tags = [
        "metal",
        "hard rock",
        "punk",
        "thrash",
        "death",
        "doom",
        "industrial",
    ];
    let (mut metal, mut others): (Vec<_>, Vec<_>) = stations.into_iter().partition(|station| {
        tags.iter()
            .any(|tag| station.tags.to_lowercase().contains(tag))
    });
    let key = |station: &Station| {
        (
            !station.url.starts_with("https://"),
            station.name.to_lowercase(),
        )
    };
    metal.sort_by_key(key);
    others.sort_by_key(key);
    metal.extend(others);
    metal
}

#[cfg(test)]
pub(crate) fn dedupe(stations: Vec<Station>) -> Vec<Station> {
    let mut seen = HashSet::new();
    stations
        .into_iter()
        .filter(|station| seen.insert(station.url.trim_end_matches('/').to_lowercase()))
        .collect()
}

pub fn infer_codec(codec: &str, url: &str) -> String {
    let normalized = codec.trim().to_lowercase();
    let url = url.to_lowercase();
    if url.ends_with(".opus") || url.contains(".opus?") {
        return "opus".into();
    }
    if url.ends_with(".ogg") || url.contains(".ogg?") {
        return if normalized.is_empty() {
            "ogg".into()
        } else {
            normalized
        };
    }
    if url.contains("aacp")
        || url.ends_with(".aac")
        || url.contains(".aac?")
        || url.contains("/aac")
    {
        return "aac+".into();
    }
    if matches!(
        normalized.as_str(),
        "aac" | "aac+" | "he-aac" | "aacp" | "he-aac-v2"
    ) {
        return normalized;
    }
    normalized
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut input = bytes.to_vec();
    let length = (input.len() as u64) * 8;
    input.push(0x80);
    while !(input.len() + 8).is_multiple_of(64) {
        input.push(0)
    }
    input.extend_from_slice(&length.to_be_bytes());
    let mut state = [
        0x6a09e667_u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    for chunk in input.chunks_exact(64) {
        let mut w = [0_u32; 64];
        for (i, g) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes(g.try_into().expect("four bytes"));
        }
        for i in 16..64 {
            let a = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let b = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(a)
                .wrapping_add(w[i - 7])
                .wrapping_add(b);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h) = (
            state[0], state[1], state[2], state[3], state[4], state[5], state[6], state[7],
        );
        for i in 0..64 {
            let t1 = h
                .wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25))
                .wrapping_add((e & f) ^ (!e & g))
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let t2 = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22))
                .wrapping_add((a & b) ^ (a & c) ^ (b & c));
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }
    state.iter().map(|word| format!("{word:08x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupe_and_order_keep_the_selected_playback_url() {
        let stations = vec![
            Station::from_primary(
                "a".into(),
                "Z".into(),
                "https://x.test/".into(),
                "rock".into(),
                "".into(),
                128,
                "mp3".into(),
            ),
            Station::from_primary(
                "b".into(),
                "A".into(),
                "https://x.test".into(),
                "metal".into(),
                "".into(),
                128,
                "mp3".into(),
            ),
        ];
        let result = order_stations(dedupe(stations));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].url, "https://x.test/");
    }

    #[test]
    fn sha256_hex_hashes_correctly() {
        assert_eq!(
            sha256_hex(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
}
