/// Standard namespace URI prefix required by the XMP specification for JPEG APP1 segments.
pub const XMP_NAMESPACE: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

/// Returns true if the byte slice starts with the JPEG SOI marker (`0xFF, 0xD8`).
pub fn is_jpeg(data: &[u8]) -> bool {
    data.len() >= 2 && data[0] == 0xFF && data[1] == 0xD8
}

#[derive(Debug, Clone)]
struct SegmentSpan {
    marker: u8,
    start: usize,
    end: usize,
    payload_start: usize,
    payload_end: usize,
}

impl SegmentSpan {
    fn payload<'a>(&self, data: &'a [u8]) -> &'a [u8] {
        &data[self.payload_start..self.payload_end]
    }

    fn full_bytes<'a>(&self, data: &'a [u8]) -> &'a [u8] {
        &data[self.start..self.end]
    }

    fn is_xmp(&self, data: &[u8]) -> bool {
        self.marker == 0xE1 && self.payload(data).starts_with(XMP_NAMESPACE)
    }

    fn is_exif(&self, data: &[u8]) -> bool {
        self.marker == 0xE1 && self.payload(data).starts_with(b"Exif\0\0")
    }

    fn is_jfif(&self, data: &[u8]) -> bool {
        self.marker == 0xE0 && self.payload(data).starts_with(b"JFIF\0")
    }
}

/// Parses the segments before SOS (`0xFF, 0xDA`) and returns the segments plus the byte offset
/// of the start of SOS (header and entropy-coded scan data).
fn parse_segments(data: &[u8]) -> Result<(Vec<SegmentSpan>, usize), String> {
    if !is_jpeg(data) {
        return Err("not a valid JPEG: missing SOI marker (0xFF, 0xD8)".to_string());
    }

    let mut pos = 2; // right after SOI
    let mut segments = Vec::new();

    while pos < data.len() {
        // Skip any fill bytes 0xFF
        while pos < data.len() && data[pos] == 0xFF {
            pos += 1;
        }
        if pos >= data.len() {
            return Err("truncated JPEG: marker type missing".to_string());
        }
        let marker = data[pos];
        let marker_start = pos - 1; // including the 0xFF before it
        pos += 1;

        // Restart markers (RST0..RST7) have no payload length
        if (0xD0..=0xD7).contains(&marker) {
            continue;
        }

        // EOI (0xD9): End of Image
        if marker == 0xD9 {
            return Err("encountered EOI before SOS marker".to_string());
        }

        // SOS (0xDA): Start of Scan
        if marker == 0xDA {
            // SOS marks the end of header segments.
            // The remainder of the file (from marker_start to EOF) contains the SOS header and scan data.
            return Ok((segments, marker_start));
        }

        // Other markers require a 2-byte big-endian length
        if pos + 2 > data.len() {
            return Err("truncated JPEG segment length".to_string());
        }
        let seg_len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
        if seg_len < 2 {
            return Err(format!("invalid JPEG segment length {seg_len}"));
        }
        let seg_end = pos + seg_len;
        if seg_end > data.len() {
            return Err("JPEG segment extends beyond end of file".to_string());
        }

        let payload_start = pos + 2;
        let payload_end = seg_end;

        segments.push(SegmentSpan {
            marker,
            start: marker_start,
            end: seg_end,
            payload_start,
            payload_end,
        });

        pos = seg_end;
    }

    Err("no SOS marker found in JPEG".to_string())
}

/// Builds a new APP1 XMP segment containing `xmp_packet`.
fn build_xmp_segment(xmp_packet: &[u8]) -> Result<Vec<u8>, String> {
    let payload_len = XMP_NAMESPACE.len() + xmp_packet.len();
    let total_len = payload_len + 2; // includes 2 bytes of length
    if total_len > 65535 {
        return Err(format!(
            "XMP packet length ({total_len} bytes) exceeds JPEG segment maximum (65535 bytes)"
        ));
    }
    let len_u16 = total_len as u16;

    let mut seg = Vec::with_capacity(2 + 2 + payload_len);
    seg.push(0xFF);
    seg.push(0xE1); // APP1
    seg.extend_from_slice(&len_u16.to_be_bytes());
    seg.extend_from_slice(XMP_NAMESPACE);
    seg.extend_from_slice(xmp_packet);
    Ok(seg)
}

/// Inserts or replaces the XMP APP1 segment in a JPEG copy without re-encoding and keeping
/// all other segments (Exif, ICC, tables) and the compressed scan data 100% byte-for-byte identical.
pub fn embed_xmp_in_jpeg(data: &[u8], xmp_packet: &[u8]) -> Result<Vec<u8>, String> {
    let (segments, sos_offset) = parse_segments(data)?;
    let new_xmp_seg = build_xmp_segment(xmp_packet)?;

    let has_xmp = segments.iter().any(|s| s.is_xmp(data));

    let mut out = Vec::with_capacity(data.len() + new_xmp_seg.len());
    // Write SOI
    out.extend_from_slice(&[0xFF, 0xD8]);

    if has_xmp {
        // Replace the first XMP segment in-place, drop any secondary XMP segments,
        // and keep all other segments in place.
        let mut replaced = false;
        for s in &segments {
            if s.is_xmp(data) {
                if !replaced {
                    out.extend_from_slice(&new_xmp_seg);
                    replaced = true;
                }
            } else {
                out.extend_from_slice(s.full_bytes(data));
            }
        }
    } else {
        // No existing XMP segment:
        // Ideal placement per XMP & Exif specs:
        // - After Exif APP1 if Exif exists;
        // - Else after JFIF APP0 if JFIF exists;
        // - Else right after SOI before any segments.
        let exif_idx = segments.iter().position(|s| s.is_exif(data));
        let jfif_idx = segments.iter().position(|s| s.is_jfif(data));

        let insert_after_idx = exif_idx.or(jfif_idx);

        match insert_after_idx {
            Some(idx) => {
                for (i, s) in segments.iter().enumerate() {
                    out.extend_from_slice(s.full_bytes(data));
                    if i == idx {
                        out.extend_from_slice(&new_xmp_seg);
                    }
                }
            }
            None => {
                out.extend_from_slice(&new_xmp_seg);
                for s in &segments {
                    out.extend_from_slice(s.full_bytes(data));
                }
            }
        }
    }

    // Append the entire SOS section and compressed scan data verbatim
    out.extend_from_slice(&data[sos_offset..]);
    Ok(out)
}

/// Helper for tests to extract the embedded XMP packet from a JPEG byte slice.
pub fn extract_xmp_from_jpeg(data: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let (segments, _) = parse_segments(data)?;
    for s in segments {
        if s.is_xmp(data) {
            let payload = s.payload(data);
            return Ok(Some(payload[XMP_NAMESPACE.len()..].to_vec()));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Constructs a valid synthetic JPEG with SOI, Exif APP1, DQT, SOS, and scan data with EOI.
    fn make_synthetic_jpeg(exif_payload: &[u8], scan_data: &[u8]) -> Vec<u8> {
        let mut jpeg = Vec::new();
        // SOI
        jpeg.extend_from_slice(&[0xFF, 0xD8]);

        // Exif APP1
        let mut exif_seg_payload = Vec::new();
        exif_seg_payload.extend_from_slice(b"Exif\0\0");
        exif_seg_payload.extend_from_slice(exif_payload);
        let exif_len = (exif_seg_payload.len() + 2) as u16;
        jpeg.extend_from_slice(&[0xFF, 0xE1]);
        jpeg.extend_from_slice(&exif_len.to_be_bytes());
        jpeg.extend_from_slice(&exif_seg_payload);

        // DQT (Quantization table)
        jpeg.extend_from_slice(&[0xFF, 0xDB]);
        let dqt_len = 5u16; // 2 bytes len + 3 bytes dummy table
        jpeg.extend_from_slice(&dqt_len.to_be_bytes());
        jpeg.extend_from_slice(&[0x00, 0x01, 0x02]);

        // SOS (Start of Scan)
        jpeg.extend_from_slice(&[0xFF, 0xDA]);
        let sos_len = 4u16; // 2 bytes len + 2 bytes dummy components
        jpeg.extend_from_slice(&sos_len.to_be_bytes());
        jpeg.extend_from_slice(&[0x01, 0x02]);

        // Scan bitstream data
        jpeg.extend_from_slice(scan_data);

        // EOI
        jpeg.extend_from_slice(&[0xFF, 0xD9]);
        jpeg
    }

    #[test]
    fn embed_xmp_inserts_after_exif_and_preserves_scan_data() {
        let original_exif = b"test exif data";
        let original_scan = b"compressed entropy bitstream";
        let jpeg = make_synthetic_jpeg(original_exif, original_scan);

        let xmp_content = b"<x:xmpmeta>hello xmp</x:xmpmeta>";
        let embedded = embed_xmp_in_jpeg(&jpeg, xmp_content).unwrap();

        // Check that XMP can be extracted
        let extracted = extract_xmp_from_jpeg(&embedded).unwrap();
        assert_eq!(extracted.as_deref(), Some(&xmp_content[..]));

        // Check that Exif is preserved
        let (segments, sos_offset) = parse_segments(&embedded).unwrap();
        assert!(segments.len() >= 3);
        assert!(segments[0].is_exif(&embedded));
        assert!(segments[1].is_xmp(&embedded));

        // Verify scan data at the end is completely unchanged
        assert!(
            embedded[sos_offset..]
                .windows(original_scan.len())
                .any(|w| w == original_scan)
        );
        assert!(embedded.ends_with(b"\xFF\xD9"));
    }

    #[test]
    fn embed_xmp_replaces_existing_xmp_segment() {
        let original_exif = b"exif";
        let original_scan = b"scan";
        let jpeg = make_synthetic_jpeg(original_exif, original_scan);

        let xmp_v1 = b"<x:xmpmeta>v1</x:xmpmeta>";
        let embedded_v1 = embed_xmp_in_jpeg(&jpeg, xmp_v1).unwrap();
        assert_eq!(
            extract_xmp_from_jpeg(&embedded_v1).unwrap().as_deref(),
            Some(&xmp_v1[..])
        );

        let xmp_v2 = b"<x:xmpmeta>v2-updated</x:xmpmeta>";
        let embedded_v2 = embed_xmp_in_jpeg(&embedded_v1, xmp_v2).unwrap();
        assert_eq!(
            extract_xmp_from_jpeg(&embedded_v2).unwrap().as_deref(),
            Some(&xmp_v2[..])
        );

        // Count XMP segments: exactly 1
        let (segments, _) = parse_segments(&embedded_v2).unwrap();
        let xmp_count = segments.iter().filter(|s| s.is_xmp(&embedded_v2)).count();
        assert_eq!(xmp_count, 1);
    }

    #[test]
    fn embed_xmp_fails_on_non_jpeg() {
        let non_jpeg = b"PNG image data";
        let res = embed_xmp_in_jpeg(non_jpeg, b"xmp");
        assert!(res.is_err());
    }
}
