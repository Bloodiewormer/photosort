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

    /// ExtendedXMP chunks belong to the main packet they extend, so they go when it is replaced.
    fn is_extended_xmp(&self, data: &[u8]) -> bool {
        self.marker == 0xE1
            && self
                .payload(data)
                .starts_with(b"http://ns.adobe.com/xmp/extension/\0")
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
        return Err("no es un JPEG válido: falta el marcador inicial (0xFF, 0xD8)".to_string());
    }

    let mut pos = 2; // right after SOI
    let mut segments = Vec::new();

    while pos < data.len() {
        if data[pos] != 0xFF {
            return Err(format!(
                "JPEG dañado: se esperaba un marcador en el byte {pos}"
            ));
        }
        // Skip any fill bytes 0xFF
        while pos < data.len() && data[pos] == 0xFF {
            pos += 1;
        }
        if pos >= data.len() {
            return Err("JPEG incompleto: falta el tipo de marcador".to_string());
        }
        let marker = data[pos];
        let marker_start = pos - 1; // including the 0xFF before it
        pos += 1;

        // Restart markers (RST0..RST7) have no payload length
        if (0xD0..=0xD7).contains(&marker) {
            continue;
        }

        // EOI, a second SOI or a stuffed zero cannot appear before SOS.
        if matches!(marker, 0xD9 | 0xD8 | 0x00) {
            return Err(format!(
                "JPEG dañado: marcador 0x{marker:02X} inesperado antes de los datos de imagen"
            ));
        }

        // SOS (0xDA): Start of Scan
        if marker == 0xDA {
            // SOS marks the end of header segments.
            // The remainder of the file (from marker_start to EOF) contains the SOS header and scan data.
            return Ok((segments, marker_start));
        }

        // Other markers require a 2-byte big-endian length
        if pos + 2 > data.len() {
            return Err("JPEG incompleto: falta la longitud de un segmento".to_string());
        }
        let seg_len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
        if seg_len < 2 {
            return Err(format!(
                "JPEG dañado: longitud de segmento no válida ({seg_len})"
            ));
        }
        let seg_end = pos + seg_len;
        if seg_end > data.len() {
            return Err("JPEG incompleto: un segmento pasa del final del archivo".to_string());
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

    Err("JPEG incompleto: no tiene datos de imagen".to_string())
}

/// Builds a new APP1 XMP segment containing `xmp_packet`.
fn build_xmp_segment(xmp_packet: &[u8]) -> Result<Vec<u8>, String> {
    let payload_len = XMP_NAMESPACE.len() + xmp_packet.len();
    let total_len = payload_len + 2; // includes 2 bytes of length
    if total_len > 65535 {
        return Err(format!(
            "las etiquetas ocupan {total_len} bytes y un segmento JPEG admite 65535 como máximo"
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
    let segments: Vec<_> = segments
        .into_iter()
        .filter(|s| !s.is_extended_xmp(data))
        .collect();

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

    fn seg(marker: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![0xFF, marker];
        v.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
        v.extend_from_slice(payload);
        v
    }

    const SCAN: &[u8] = &[
        0xFF, 0xDA, 0x00, 0x04, 0x01, 0x02, 0xAB, 0xFF, 0x00, 0xFF, 0xD9,
    ];

    #[test]
    fn a_byte_that_is_not_a_marker_is_an_error_not_a_misparse() {
        let mut jpeg = vec![0xFF, 0xD8];
        jpeg.extend(seg(0xE0, b"JFIF\0abc"));
        jpeg.extend_from_slice(&[0x12, 0x00, 0x02]); // garbage that looks like a 2-byte segment
        jpeg.extend_from_slice(SCAN);
        assert!(embed_xmp_in_jpeg(&jpeg, b"x").is_err());
    }

    #[test]
    fn malformed_headers_are_errors() {
        // Length past the end of the file.
        let mut long = vec![0xFF, 0xD8, 0xFF, 0xE1, 0x10, 0x00, 0x01];
        long.extend_from_slice(SCAN);
        assert!(embed_xmp_in_jpeg(&long, b"x").is_err());
        // EOI before SOS, length below 2, truncated marker, nothing after SOI.
        assert!(embed_xmp_in_jpeg(&[0xFF, 0xD8, 0xFF, 0xD9], b"x").is_err());
        assert!(embed_xmp_in_jpeg(&[0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x01], b"x").is_err());
        assert!(embed_xmp_in_jpeg(&[0xFF, 0xD8, 0xFF], b"x").is_err());
        assert!(embed_xmp_in_jpeg(&[0xFF, 0xD8], b"x").is_err());
    }

    #[test]
    fn fill_bytes_are_skipped_and_scan_is_copied_verbatim() {
        let mut jpeg = vec![0xFF, 0xD8];
        jpeg.extend(seg(0xE1, b"Exif\0\0e"));
        jpeg.extend_from_slice(&[0xFF, 0xFF]); // fill before the next marker
        jpeg.extend(seg(0xE2, b"ICC_PROFILE\0p"));
        jpeg.extend_from_slice(SCAN);
        let out = embed_xmp_in_jpeg(&jpeg, b"<x/>").unwrap();
        let mut expected = vec![0xFF, 0xD8];
        expected.extend(seg(0xE1, b"Exif\0\0e"));
        expected.extend(build_xmp_segment(b"<x/>").unwrap());
        expected.extend(seg(0xE2, b"ICC_PROFILE\0p"));
        expected.extend_from_slice(SCAN);
        assert_eq!(out, expected);
    }

    #[test]
    fn replacing_xmp_drops_the_old_extended_xmp() {
        let mut main = XMP_NAMESPACE.to_vec();
        main.extend_from_slice(b"old");
        let mut jpeg = vec![0xFF, 0xD8];
        jpeg.extend(seg(0xE1, b"Exif\0\0e"));
        jpeg.extend(seg(0xE1, &main));
        jpeg.extend(seg(
            0xE1,
            b"http://ns.adobe.com/xmp/extension/\0old-extension",
        ));
        jpeg.extend_from_slice(SCAN);
        let out = embed_xmp_in_jpeg(&jpeg, b"new").unwrap();
        let mut expected = vec![0xFF, 0xD8];
        expected.extend(seg(0xE1, b"Exif\0\0e"));
        expected.extend(build_xmp_segment(b"new").unwrap());
        expected.extend_from_slice(SCAN);
        assert_eq!(out, expected);
    }

    #[test]
    fn the_segment_limit_is_65535_bytes() {
        let max = 65_533 - XMP_NAMESPACE.len();
        assert_eq!(build_xmp_segment(&vec![b'a'; max]).unwrap().len(), 65_537);
        assert!(build_xmp_segment(&vec![b'a'; max + 1]).is_err());
    }

    #[test]
    fn embed_xmp_fails_on_non_jpeg() {
        let non_jpeg = b"PNG image data";
        let res = embed_xmp_in_jpeg(non_jpeg, b"xmp");
        assert!(res.is_err());
    }
}
