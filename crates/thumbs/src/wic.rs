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

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::{MAX_ALLOC_BYTES, MAX_DIMENSION};
    use image::imageops::FilterType;
    use image::metadata::Orientation;
    use image::{DynamicImage, RgbaImage};
    use std::fs::File;
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
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
        // Ensure file exists and is accessible read-only
        let _file = File::open(path)?;

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
                .map_err(map_wic_error)
        }
    }

    #[link(name = "ole32")]
    unsafe extern "system" {
        fn PropVariantClear(pvar: *mut PROPVARIANT) -> HRESULT;
    }

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
                        VT_UI2 => Some((*prop.Anonymous.Anonymous).Anonymous.uiVal as u32),
                        VT_UI4 => Some((*prop.Anonymous.Anonymous).Anonymous.ulVal),
                        VT_I2 => Some((*prop.Anonymous.Anonymous).Anonymous.iVal as u32),
                        VT_I4 => Some((*prop.Anonymous.Anonymous).Anonymous.lVal as u32),
                        _ => None,
                    };
                    let _ = PropVariantClear(&mut prop);
                    if let Some(orient) = val.and_then(|v| Orientation::from_exif(v as u8)) {
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

        if img.width() > max_px || img.height() > max_px {
            Ok(img.resize(max_px, max_px, FilterType::Lanczos3))
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
