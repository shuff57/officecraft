//! A graph picture that remembers the graph: the viSHual document rides inside the PNG as a
//! `tEXt` chunk, so a picture in a document (or one copied to another file) can be opened again
//! in a Graph tab and edited.

const KEY: &str = "vishual-graph";
const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(B64[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0);
    for ch in s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=') {
        let v = B64.iter().position(|&x| x == ch)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

/// `png` with the graph document stored in it (after the header chunk). `None` if it isn't a PNG.
pub fn embed(png: &[u8], graph_json: &str) -> Option<Vec<u8>> {
    if png.get(..8)? != SIG {
        return None;
    }
    let ihdr_len = u32::from_be_bytes(png.get(8..12)?.try_into().ok()?) as usize;
    let split = 8 + 12 + ihdr_len;
    let mut body = Vec::new();
    body.extend_from_slice(KEY.as_bytes());
    body.push(0);
    body.extend_from_slice(base64_encode(graph_json.as_bytes()).as_bytes());
    let mut chunk = Vec::new();
    chunk.extend_from_slice(&(body.len() as u32).to_be_bytes());
    let mut typed = b"tEXt".to_vec();
    typed.extend_from_slice(&body);
    chunk.extend_from_slice(&typed);
    chunk.extend_from_slice(&crc32(&typed).to_be_bytes());
    let mut out = png.get(..split)?.to_vec();
    out.extend_from_slice(&chunk);
    out.extend_from_slice(png.get(split..)?);
    Some(out)
}

/// The graph document stored in `png` by [`embed`], if any.
pub fn extract(png: &[u8]) -> Option<String> {
    if png.get(..8)? != SIG {
        return None;
    }
    let mut i = 8;
    while i + 12 <= png.len() {
        let len = u32::from_be_bytes(png[i..i + 4].try_into().ok()?) as usize;
        let kind = &png[i + 4..i + 8];
        let data = png.get(i + 8..i + 8 + len)?;
        if kind == b"tEXt"
            && let Some(rest) = data.strip_prefix(KEY.as_bytes())
            && let Some(b64) = rest.strip_prefix(&[0])
        {
            return String::from_utf8(base64_decode(std::str::from_utf8(b64).ok()?)?).ok();
        }
        if kind == b"IDAT" || kind == b"IEND" {
            return None;
        }
        i += 12 + len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_png() -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, 2, 2);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&[255u8; 16]).unwrap();
        out
    }

    #[test]
    fn the_graph_survives_a_round_trip_and_the_picture_still_decodes() {
        let json = r#"{"items":[{"latex":"y=x^2 — π"}]}"#;
        let with = embed(&tiny_png(), json).unwrap();
        assert_eq!(extract(&with).as_deref(), Some(json));
        let dec = png::Decoder::new(std::io::Cursor::new(&with));
        let mut r = dec.read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        r.next_frame(&mut buf).unwrap();
        assert_eq!(buf, vec![255u8; 16]);
    }

    #[test]
    fn a_plain_picture_has_no_graph() {
        assert_eq!(extract(&tiny_png()), None);
        assert_eq!(extract(b"not a png"), None);
    }

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert_eq!(base64_decode("TWE=").unwrap(), b"Ma");
    }
}
