use crate::ThumbError;
use std::path::Path;

pub const WIC_EXTENSIONS: &[&str] = &["heic", "heif", "avif", "jxl"];

pub fn is_wic_extension(ext: &str) -> bool {
    WIC_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
}

pub fn is_wic_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(is_wic_extension)
        .unwrap_or(false)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageFacts {
    pub width: u32,
    pub height: u32,
    pub format: String,
}

#[cfg(windows)]
pub use imp::*;

#[cfg(not(windows))]
pub mod imp {
    use super::*;
    use image::DynamicImage;

    pub fn decode_wic(_path: &Path, _max_px: u32) -> Result<DynamicImage, ThumbError> {
        Err(ThumbError::Unsupported(
            "Windows Imaging Component solo está disponible en Windows".into(),
        ))
    }

    pub fn read_image_metadata(_path: &Path) -> Result<ImageFacts, ThumbError> {
        Err(ThumbError::Unsupported(
            "Windows Imaging Component solo está disponible en Windows".into(),
        ))
    }
}

/// Checks whether the leading bytes match a valid HEIC/HEIF, AVIF, or JXL container/codestream
/// signature. Used to distinguish a genuinely missing WIC codec from a corrupt or unrelated file
/// when `CreateDecoderFromFilename` returns `WINCODEC_ERR_COMPONENTNOTFOUND`.
fn has_known_wic_signature(buf: &[u8]) -> bool {
    // JPEG XL naked codestream (0xFF 0x0A) or ISOBMFF container signature.
    if buf.starts_with(&[0xFF, 0x0A])
        || buf.starts_with(&[
            0x00, 0x00, 0x00, 0x0C, 0x4A, 0x58, 0x4C, 0x20, 0x0D, 0x0A, 0x87, 0x0A,
        ])
    {
        return true;
    }

    // ISOBMFF `ftyp` box for HEIC/HEIF and AVIF.
    if buf.len() >= 12 && &buf[4..8] == b"ftyp" {
        const BRANDS: &[[u8; 4]] = &[
            *b"heic", *b"heix", *b"hevc", *b"hevx", *b"heim", *b"heis", *b"hevm", *b"hevs",
            *b"mif1", *b"msf1", *b"avif", *b"avis",
        ];
        if BRANDS.iter().any(|b| &buf[8..12] == b) {
            return true;
        }
        let box_size = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        let end = if box_size >= 16 {
            box_size.min(buf.len())
        } else {
            buf.len()
        };
        if end > 16 {
            for chunk in buf[16..end].as_chunks::<4>().0 {
                if BRANDS.iter().any(|b| chunk == b) {
                    return true;
                }
            }
        }
    }

    false
}

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::{MAX_ALLOC_BYTES, MAX_DIMENSION};
    use image::imageops::FilterType;
    use image::metadata::Orientation;
    use image::{DynamicImage, RgbaImage};
    use std::fs::File;
    use std::io::Read;
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PropVariantClear};
    use windows::Win32::System::Com::*;
    use windows::Win32::System::Variant::*;
    use windows::core::{HRESULT, PCWSTR};

    const WINCODEC_ERR_COMPONENTNOTFOUND_I32: i32 = -2003292336; // 0x88982F50
    const WINCODEC_ERR_UNSUPPORTEDFORMAT_I32: i32 = -2003292409; // 0x88982F07
    const REGDB_E_CLASSNOTREG_I32: i32 = -2147221164; // 0x80040154

    struct ComGuard(bool);

    impl ComGuard {
        fn enter() -> Result<Self, ThumbError> {
            unsafe {
                let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
                if hr == S_OK || hr == S_FALSE {
                    Ok(ComGuard(true))
                } else if hr == RPC_E_CHANGED_MODE {
                    Ok(ComGuard(false))
                } else {
                    Err(ThumbError::Unsupported(format!(
                        "CoInitializeEx error: {hr}"
                    )))
                }
            }
        }
    }

    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe {
                    CoUninitialize();
                }
            }
        }
    }

    fn is_missing_codec_error(hr: HRESULT) -> bool {
        hr.0 == WINCODEC_ERR_COMPONENTNOTFOUND_I32
            || hr.0 == WINCODEC_ERR_UNSUPPORTEDFORMAT_I32
            || hr.0 == REGDB_E_CLASSNOTREG_I32
    }

    fn map_wic_error(e: windows::core::Error) -> ThumbError {
        if is_missing_codec_error(e.code()) {
            ThumbError::Unsupported(format!("códec de imagen no disponible: {e}"))
        } else {
            ThumbError::Decode(format!("error al decodificar la imagen con WIC: {e}"))
        }
    }

    fn create_factory() -> Result<IWICImagingFactory, ThumbError> {
        unsafe {
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| ThumbError::Unsupported(format!("CoCreateInstance WIC: {e}")))
        }
    }

    fn open_decoder(
        factory: &IWICImagingFactory,
        path: &Path,
    ) -> Result<IWICBitmapDecoder, ThumbError> {
        // Ensure file exists and is accessible read-only.
        let mut file = File::open(path)?;

        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            factory
                .CreateDecoderFromFilename(
                    PCWSTR(wide.as_ptr()),
                    None,
                    GENERIC_READ,
                    WICDecodeMetadataCacheOnDemand,
                )
                .map_err(|e| {
                    if is_missing_codec_error(e.code()) {
                        let mut header = [0u8; 64];
                        let n = file.read(&mut header).unwrap_or(0);
                        if has_known_wic_signature(&header[..n]) {
                            ThumbError::Unsupported(format!("códec de imagen no disponible: {e}"))
                        } else {
                            ThumbError::Decode(format!(
                                "cabecera de imagen corrupta o no reconocida con WIC: {e}"
                            ))
                        }
                    } else {
                        map_wic_error(e)
                    }
                })
        }
    }

    // shortcut: algunos decodificadores WIC (en especial las extensiones HEIF/AVIF de Windows
    // cuando el contenedor ISOBMFF trae transformación `irot`/`imir` junto a una etiqueta EXIF
    // de orientación) pueden devolver los píxeles ya orientados y seguir exponiendo un valor
    // distinto de 1 en los metadatos. Sin un juego de pruebas por cada extensión de códec no es
    // posible detectar con certeza si el decodificador ya rotó los píxeles, así que aplicamos la
    // orientación reportada y dejamos esta limitación documentada para ajustarla por formato si
    // aparece doble rotación en alguna versión del códec.
    fn read_orientation(frame: &IWICBitmapFrameDecode) -> Orientation {
        unsafe {
            let Ok(query_reader) = frame.GetMetadataQueryReader() else {
                return Orientation::NoTransforms;
            };

            for query_str in ["System.Photo.Orientation\0", "/app1/ifd/{ushort=274}\0"] {
                let wide: Vec<u16> = query_str.encode_utf16().collect();
                let mut prop = PROPVARIANT::default();
                if query_reader
                    .GetMetadataByName(PCWSTR(wide.as_ptr()), &mut prop)
                    .is_ok()
                {
                    let val = match (*prop.Anonymous.Anonymous).vt {
                        VT_UI2 => Some(u32::from((*prop.Anonymous.Anonymous).Anonymous.uiVal)),
                        VT_UI4 => Some((*prop.Anonymous.Anonymous).Anonymous.ulVal),
                        VT_I2 => u32::try_from((*prop.Anonymous.Anonymous).Anonymous.iVal).ok(),
                        VT_I4 => u32::try_from((*prop.Anonymous.Anonymous).Anonymous.lVal).ok(),
                        _ => None,
                    };
                    let _ = PropVariantClear(&mut prop);
                    if let Some(orient) = val
                        .and_then(|v| u8::try_from(v).ok())
                        .and_then(Orientation::from_exif)
                    {
                        return orient;
                    }
                }
            }
            Orientation::NoTransforms
        }
    }

    pub fn decode_wic(path: &Path, max_px: u32) -> Result<DynamicImage, ThumbError> {
        let _com = ComGuard::enter()?;
        let factory = create_factory()?;
        let decoder = open_decoder(&factory, path)?;

        let frame = unsafe { decoder.GetFrame(0).map_err(map_wic_error)? };

        let mut orig_w = 0u32;
        let mut orig_h = 0u32;
        unsafe {
            frame
                .GetSize(&mut orig_w, &mut orig_h)
                .map_err(map_wic_error)?
        };

        if orig_w == 0 || orig_h == 0 {
            return Err(ThumbError::Decode(format!(
                "invalid dimensions {orig_w}x{orig_h}"
            )));
        }
        if orig_w > MAX_DIMENSION || orig_h > MAX_DIMENSION {
            return Err(ThumbError::Decode(format!(
                "image dimensions {orig_w}x{orig_h} exceed limit {MAX_DIMENSION}"
            )));
        }

        let estimated_bytes = u64::from(orig_w)
            .saturating_mul(u64::from(orig_h))
            .saturating_mul(4);
        if estimated_bytes > MAX_ALLOC_BYTES {
            return Err(ThumbError::Decode(format!(
                "image buffer {estimated_bytes} bytes exceeds limit {MAX_ALLOC_BYTES}"
            )));
        }

        let orientation = read_orientation(&frame);

        let converter = unsafe { factory.CreateFormatConverter().map_err(map_wic_error)? };
        unsafe {
            converter
                .Initialize(
                    &frame,
                    &GUID_WICPixelFormat32bppRGBA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )
                .map_err(map_wic_error)?;
        }

        let stride = orig_w
            .checked_mul(4)
            .ok_or_else(|| ThumbError::Decode("stride overflow".into()))?;
        let buffer_size = (stride as usize)
            .checked_mul(orig_h as usize)
            .ok_or_else(|| ThumbError::Decode("buffer size overflow".into()))?;

        let mut buffer = vec![0u8; buffer_size];
        unsafe {
            converter
                .CopyPixels(std::ptr::null(), stride, &mut buffer)
                .map_err(map_wic_error)?;
        }

        let rgba = RgbaImage::from_raw(orig_w, orig_h, buffer)
            .ok_or_else(|| ThumbError::Decode("failed to create image buffer".into()))?;
        let mut img = DynamicImage::ImageRgba8(rgba);
        img.apply_orientation(orientation);

        if max_px > 0 && (img.width() > max_px || img.height() > max_px) {
            img = img.resize(max_px, max_px, FilterType::Lanczos3);
        }

        // Drop the synthetic alpha channel for opaque images so thumbnail caching writes JPEG
        // instead of PNG for regular HEIC/AVIF/JXL photos.
        let is_opaque = img.as_rgba8().is_some_and(|rgba| {
            rgba.as_raw()
                .as_chunks::<4>()
                .0
                .iter()
                .all(|px| px[3] == 255)
        });
        if is_opaque {
            Ok(DynamicImage::ImageRgb8(img.into_rgb8()))
        } else {
            Ok(img)
        }
    }

    pub fn read_image_metadata(path: &Path) -> Result<ImageFacts, ThumbError> {
        let _com = ComGuard::enter()?;
        let factory = create_factory()?;
        let decoder = open_decoder(&factory, path)?;

        let frame = unsafe { decoder.GetFrame(0).map_err(map_wic_error)? };

        let mut orig_w = 0u32;
        let mut orig_h = 0u32;
        unsafe {
            frame
                .GetSize(&mut orig_w, &mut orig_h)
                .map_err(map_wic_error)?
        };

        if orig_w == 0 || orig_h == 0 {
            return Err(ThumbError::Decode(format!(
                "invalid dimensions {orig_w}x{orig_h}"
            )));
        }

        let orientation = read_orientation(&frame);
        let (width, height) = match orientation {
            Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH => (orig_h, orig_w),
            _ => (orig_w, orig_h),
        };

        let format = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|ext| ext.to_ascii_uppercase())
            .unwrap_or_else(|| "IMAGEN".into());

        Ok(ImageFacts {
            width,
            height,
            format,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_detection_distinguishes_valid_headers_from_corrupt_data() {
        // HEIC major brand
        assert!(has_known_wic_signature(
            b"\x00\x00\x00\x18ftypheic\x00\x00\x00\x00mif1heic"
        ));
        // AVIF compatible brand under mif1
        assert!(has_known_wic_signature(
            b"\x00\x00\x00\x1cftypmif1\x00\x00\x00\x00miafavif"
        ));
        // JXL naked codestream and ISOBMFF container
        assert!(has_known_wic_signature(&[0xFF, 0x0A, 0x00, 0x10]));
        assert!(has_known_wic_signature(&[
            0x00, 0x00, 0x00, 0x0C, b'J', b'X', b'L', b' ', 0x0D, 0x0A, 0x87, 0x0A, 0x00,
        ]));

        // Corrupt / unrelated payloads
        assert!(!has_known_wic_signature(b""));
        assert!(!has_known_wic_signature(b"dummy"));
        assert!(!has_known_wic_signature(b"not a real heic header"));
        assert!(!has_known_wic_signature(
            b"\x00\x00\x00\x18ftypmp42\x00\x00\x00\x00mp42isom"
        ));
    }

    #[cfg(windows)]
    #[test]
    fn decode_wic_works_on_sta_thread_with_rpc_e_changed_mode() {
        use image::{ImageFormat, Rgb, RgbImage};
        use windows::Win32::System::Com::{
            COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize,
        };

        let tmp = tempfile::tempdir().unwrap();
        let png = tmp.path().join("sta.png");
        let mut img = RgbImage::new(20, 10);
        for px in img.pixels_mut() {
            *px = Rgb([10, 200, 30]);
        }
        img.save_with_format(&png, ImageFormat::Png).unwrap();

        let handle = std::thread::spawn(move || unsafe {
            let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            assert!(hr.is_ok(), "STA CoInitializeEx failed: {hr}");
            let decoded = decode_wic(&png, 100).expect("decode_wic must handle RPC_E_CHANGED_MODE");
            assert_eq!((decoded.width(), decoded.height()), (20, 10));
            CoUninitialize();
        });
        handle.join().unwrap();
    }
}
