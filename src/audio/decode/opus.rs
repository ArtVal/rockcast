//! Live Ogg/Opus packet reader.

use std::{
    collections::VecDeque,
    io::{Cursor, Read},
    sync::atomic::{AtomicBool, Ordering},
};

pub struct LiveOggOpusReader<R: Read> {
    inner: R,
    packet: Vec<u8>,
    segments: VecDeque<Vec<u8>>,
    continued_at_page_start: Option<bool>,
    skipping_continued: bool,
    saw_head: bool,
    saw_tags: bool,
}

impl<R: Read> LiveOggOpusReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            packet: Vec::new(),
            segments: VecDeque::new(),
            continued_at_page_start: None,
            skipping_continued: false,
            saw_head: false,
            saw_tags: false,
        }
    }

    pub fn read_packet(&mut self, stop: &AtomicBool) -> Result<Option<Vec<u8>>, String> {
        loop {
            if self.segments.is_empty() {
                let page = match self.read_page(stop)? {
                    Some(page) => page,
                    None => return Ok(None),
                };
                self.continued_at_page_start = Some(page.continued);
                self.segments = page.segments.into();
            }
            if self.continued_at_page_start.take().unwrap_or(false) && self.packet.is_empty() {
                self.skipping_continued = true;
            }
            while let Some(segment) = self.segments.pop_front() {
                if self.skipping_continued {
                    if segment.len() < 255 {
                        self.skipping_continued = false;
                    }
                    continue;
                }
                self.packet.extend_from_slice(&segment);
                if segment.len() < 255 {
                    let packet = std::mem::take(&mut self.packet);
                    if !self.saw_head {
                        if !packet.starts_with(b"OpusHead") {
                            return Err("ogg/opus stream missing OpusHead".into());
                        }
                        self.saw_head = true;
                        continue;
                    }
                    if !self.saw_tags {
                        if !packet.starts_with(b"OpusTags") {
                            return Err("ogg/opus stream missing OpusTags".into());
                        }
                        self.saw_tags = true;
                        continue;
                    }
                    return Ok(Some(packet));
                }
            }
        }
    }

    fn read_page(&mut self, stop: &AtomicBool) -> Result<Option<OggPage>, String> {
        let mut header = [0u8; 27];
        if !read_exact_or_eof(&mut self.inner, &mut header, stop)? {
            return Ok(None);
        }
        if &header[0..4] != b"OggS" {
            return Err("invalid ogg page".into());
        }
        let continued = header[5] & 0x01 != 0;
        let page_segments = header[26] as usize;
        let mut lacing = vec![0u8; page_segments];
        read_exact_checked(&mut self.inner, &mut lacing, stop)?;
        let mut cursor = Cursor::new(lacing);
        let mut segments = Vec::new();
        while cursor.position() < cursor.get_ref().len() as u64 {
            let len = cursor.read_u8().map_err(|e| e.to_string())?;
            if len == 255 {
                let mut part = vec![0u8; 255];
                read_exact_checked(&mut self.inner, &mut part, stop)?;
                segments.push(part);
                continue;
            }
            let mut part = vec![0u8; usize::from(len)];
            read_exact_checked(&mut self.inner, &mut part, stop)?;
            segments.push(part);
        }
        Ok(Some(OggPage {
            continued,
            segments,
        }))
    }
}

struct OggPage {
    continued: bool,
    segments: Vec<Vec<u8>>,
}

fn read_exact_or_eof<R: Read>(
    reader: &mut R,
    buf: &mut [u8],
    stop: &AtomicBool,
) -> Result<bool, String> {
    let mut filled = 0;
    while filled < buf.len() {
        if stop.load(Ordering::SeqCst) {
            return Err("stopped".into());
        }
        match reader.read(&mut buf[filled..]) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => return Err("unexpected eof".into()),
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(true)
}

fn read_exact_checked<R: Read>(
    reader: &mut R,
    buf: &mut [u8],
    stop: &AtomicBool,
) -> Result<(), String> {
    if read_exact_or_eof(reader, buf, stop)? {
        Ok(())
    } else {
        Err("unexpected eof".into())
    }
}

trait ReadExt {
    fn read_u8(&mut self) -> std::io::Result<u8>;
}

impl<R: Read> ReadExt for R {
    fn read_u8(&mut self) -> std::io::Result<u8> {
        let mut b = [0u8; 1];
        self.read_exact(&mut b)?;
        Ok(b[0])
    }
}
