//! Video frame extraction and metadata reading via Windows Media Foundation.
#![allow(unsafe_code)]

use crate::ThumbError;
use image::RgbImage;
use std::path::Path;

pub const VIDEO_EXTENSIONS: &[&str] = &["mp4", "webm", "3gp", "mkv", "wmv", "mov", "avi"];

pub fn is_video_extension(ext: &str) -> bool {
    let lower = ext.to_ascii_lowercase();
    VIDEO_EXTENSIONS.contains(&lower.as_str())
}

pub fn is_video_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(is_video_extension)
        .unwrap_or(false)
}

pub fn video_format_from_extension(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "mp4" => "MP4",
        "webm" => "WEBM",
        "3gp" => "3GP",
        "mkv" => "MKV",
        "wmv" => "WMV",
        "mov" => "MOV",
        "avi" => "AVI",
        _ => "VÍDEO",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoMetadata {
    pub width: u32,
    pub height: u32,
    pub duration_ms: u64,
    pub format: String,
}

#[cfg(windows)]
pub use imp::*;

#[cfg(not(windows))]
pub mod imp {
    use super::*;

    pub fn read_metadata(_path: &Path) -> Result<VideoMetadata, ThumbError> {
        Err(ThumbError::Unsupported(
            "Windows Media Foundation solo está disponible en Windows".into(),
        ))
    }

    pub fn extract_frame(
        _path: &Path,
        _position_ratio: f32,
        _max_px: u32,
    ) -> Result<RgbImage, ThumbError> {
        Err(ThumbError::Unsupported(
            "Windows Media Foundation solo está disponible en Windows".into(),
        ))
    }

    pub fn extract_frames(
        _path: &Path,
        _count: usize,
        _max_px: u32,
    ) -> Result<Vec<RgbImage>, ThumbError> {
        Err(ThumbError::Unsupported(
            "Windows Media Foundation solo está disponible en Windows".into(),
        ))
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use image::Rgb;
    use image::imageops::FilterType;
    use std::os::windows::ffi::OsStrExt;
    use std::sync::OnceLock;
    use windows::Win32::Foundation::{RPC_E_CHANGED_MODE, S_FALSE, S_OK};
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::*;
    use windows::Win32::System::Variant::VT_I8;
    use windows::core::{PCWSTR, Result as WinResult};

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

    struct MediaReader {
        reader: IMFSourceReader,
        width: u32,
        height: u32,
        duration_hns: u64,
        _com: ComGuard,
    }

    fn init_mf() -> Result<(), ThumbError> {
        static INIT: OnceLock<Result<(), String>> = OnceLock::new();
        let res = INIT.get_or_init(|| unsafe {
            MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).map_err(|e| format!("MFStartup error: {e}"))
        });
        match res.as_ref() {
            Ok(()) => Ok(()),
            Err(e) => Err(ThumbError::Unsupported(e.clone())),
        }
    }

    fn open_reader(path: &Path) -> Result<MediaReader, ThumbError> {
        let com = ComGuard::enter()?;
        init_mf()?;
        let path_wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        unsafe {
            let mut attrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attrs, 2)
                .map_err(|e| ThumbError::Decode(format!("MFCreateAttributes: {e}")))?;
            if let Some(attrs) = &attrs {
                let _ = attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1);
            }

            let reader = MFCreateSourceReaderFromURL(PCWSTR(path_wide.as_ptr()), attrs.as_ref())
                .map_err(|e| {
                    ThumbError::Unsupported(format!(
                        "códec no disponible o formato no compatible: {e}"
                    ))
                })?;

            let _ = reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false);
            let _ = reader.SetStreamSelection(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, true);

            // Request uncompressed RGB32
            let target_type = MFCreateMediaType()
                .map_err(|e| ThumbError::Decode(format!("MFCreateMediaType: {e}")))?;
            target_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| ThumbError::Decode(format!("SetGUID major type: {e}")))?;
            target_type
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
                .map_err(|e| ThumbError::Decode(format!("SetGUID subtype: {e}")))?;

            reader
                .SetCurrentMediaType(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                    None,
                    &target_type,
                )
                .map_err(|e| {
                    ThumbError::Unsupported(format!("códec de vídeo no compatible: {e}"))
                })?;

            let cur_type = reader
                .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
                .map_err(|e| ThumbError::Decode(format!("GetCurrentMediaType: {e}")))?;

            let frame_size = cur_type
                .GetUINT64(&MF_MT_FRAME_SIZE)
                .map_err(|e| ThumbError::Decode(format!("GetUINT64 frame size: {e}")))?;
            let width = (frame_size >> 32) as u32;
            let height = (frame_size & 0xFFFF_FFFF) as u32;

            let duration_hns = reader
                .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
                .map(|var| {
                    if (*var.Anonymous.Anonymous).vt == windows::Win32::System::Variant::VT_UI8 {
                        (*var.Anonymous.Anonymous).Anonymous.uhVal
                    } else if (*var.Anonymous.Anonymous).vt == VT_I8 {
                        (*var.Anonymous.Anonymous).Anonymous.hVal as u64
                    } else {
                        0
                    }
                })
                .unwrap_or(0);

            Ok(MediaReader {
                reader,
                width,
                height,
                duration_hns,
                _com: com,
            })
        }
    }

    pub fn read_metadata(path: &Path) -> Result<VideoMetadata, ThumbError> {
        let media = open_reader(path)?;
        let duration_ms = media.duration_hns / 10_000;
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        let format = video_format_from_extension(ext).to_string();

        Ok(VideoMetadata {
            width: media.width,
            height: media.height,
            duration_ms,
            format,
        })
    }

    fn seek_reader(reader: &IMFSourceReader, target_hns: i64) -> WinResult<()> {
        unsafe {
            let mut var = PROPVARIANT::default();
            (*var.Anonymous.Anonymous).vt = VT_I8;
            (*var.Anonymous.Anonymous).Anonymous.hVal = target_hns;
            reader.SetCurrentPosition(&windows::core::GUID::zeroed(), &var)
        }
    }

    fn read_single_frame(
        reader: &IMFSourceReader,
        width: u32,
        height: u32,
    ) -> Result<RgbImage, ThumbError> {
        if width == 0 || height == 0 {
            return Err(ThumbError::Decode(
                "dimensiones de vídeo inválidas (0)".into(),
            ));
        }

        unsafe {
            let cur_type = reader
                .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
                .map_err(|e| ThumbError::Decode(format!("GetCurrentMediaType: {e}")))?;
            let default_stride = cur_type.GetUINT32(&MF_MT_DEFAULT_STRIDE).ok();
            let (stride, bottom_up) = match default_stride {
                Some(s) => {
                    let s_signed = s as i32;
                    (s_signed.unsigned_abs() as usize, s_signed < 0)
                }
                None => (width as usize * 4, false),
            };

            for _ in 0..100 {
                let mut actual_stream_index = 0u32;
                let mut stream_flags = 0u32;
                let mut timestamp = 0i64;
                let mut sample: Option<IMFSample> = None;

                reader
                    .ReadSample(
                        MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                        0,
                        Some(&mut actual_stream_index),
                        Some(&mut stream_flags),
                        Some(&mut timestamp),
                        Some(&mut sample),
                    )
                    .map_err(|e| ThumbError::Decode(format!("ReadSample: {e}")))?;

                if stream_flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }

                if let Some(sample) = sample {
                    let buffer = sample.ConvertToContiguousBuffer().map_err(|e| {
                        ThumbError::Decode(format!("ConvertToContiguousBuffer: {e}"))
                    })?;
                    let mut ptr: *mut u8 = std::ptr::null_mut();
                    let mut current_len = 0u32;
                    buffer
                        .Lock(&mut ptr, None, Some(&mut current_len))
                        .map_err(|e| ThumbError::Decode(format!("buffer.Lock: {e}")))?;

                    let buffer_slice = std::slice::from_raw_parts(ptr, current_len as usize);
                    let mut img = RgbImage::new(width, height);

                    for y in 0..height {
                        let row_idx = if bottom_up {
                            (height - 1 - y) as usize
                        } else {
                            y as usize
                        };
                        let row_offset = row_idx * stride;
                        for x in 0..width {
                            let px_offset = row_offset + (x as usize) * 4;
                            if px_offset + 3 <= buffer_slice.len() {
                                let b = buffer_slice[px_offset];
                                let g = buffer_slice[px_offset + 1];
                                let r = buffer_slice[px_offset + 2];
                                img.put_pixel(x, y, Rgb([r, g, b]));
                            }
                        }
                    }

                    let _ = buffer.Unlock();
                    return Ok(img);
                }
            }

            Err(ThumbError::Decode(
                "no se pudo leer ningún fotograma del vídeo".into(),
            ))
        }
    }

    pub fn extract_frame(
        path: &Path,
        position_ratio: f32,
        max_px: u32,
    ) -> Result<RgbImage, ThumbError> {
        let media = open_reader(path)?;
        let target_hns = if media.duration_hns > 0 {
            let clamped = position_ratio.clamp(0.0, 1.0) as f64;
            (media.duration_hns as f64 * clamped) as i64
        } else {
            0
        };

        if target_hns > 0 {
            let _ = seek_reader(&media.reader, target_hns);
        }

        let mut img = match read_single_frame(&media.reader, media.width, media.height) {
            Ok(frame) => frame,
            Err(_) if target_hns > 0 => {
                // If seeking to target position failed or hit EOF, retry from position 0
                let _ = seek_reader(&media.reader, 0);
                read_single_frame(&media.reader, media.width, media.height)?
            }
            Err(e) => return Err(e),
        };

        if max_px > 0 && (img.width() > max_px || img.height() > max_px) {
            img = image::DynamicImage::ImageRgb8(img)
                .resize(max_px, max_px, FilterType::Lanczos3)
                .into_rgb8();
        }

        Ok(img)
    }

    pub fn extract_frames(
        path: &Path,
        count: usize,
        max_px: u32,
    ) -> Result<Vec<RgbImage>, ThumbError> {
        let count = count.clamp(1, 10);
        let media = open_reader(path)?;

        let mut frames = Vec::with_capacity(count);
        let ratios: Vec<f32> = if count == 1 {
            vec![0.1]
        } else {
            (0..count)
                .map(|i| (i as f32 + 1.0) / (count as f32 + 1.0))
                .collect()
        };

        for ratio in ratios {
            let target_hns = if media.duration_hns > 0 {
                (media.duration_hns as f64 * ratio as f64) as i64
            } else {
                0
            };

            let _ = seek_reader(&media.reader, target_hns);
            if let Ok(mut frame) = read_single_frame(&media.reader, media.width, media.height) {
                if max_px > 0 && (frame.width() > max_px || frame.height() > max_px) {
                    frame = image::DynamicImage::ImageRgb8(frame)
                        .resize(max_px, max_px, FilterType::Lanczos3)
                        .into_rgb8();
                }
                frames.push(frame);
            }
        }

        if frames.is_empty() {
            // Fallback: try from beginning
            let _ = seek_reader(&media.reader, 0);
            let mut frame = read_single_frame(&media.reader, media.width, media.height)?;
            if max_px > 0 && (frame.width() > max_px || frame.height() > max_px) {
                frame = image::DynamicImage::ImageRgb8(frame)
                    .resize(max_px, max_px, FilterType::Lanczos3)
                    .into_rgb8();
            }
            frames.push(frame);
        }

        Ok(frames)
    }

    /// Creates a small synthetic MP4 video file using Windows Media Foundation IMFSinkWriter.
    /// Used for testing that reading video frames leaves the file untouched.
    pub fn create_synthetic_video(
        path: &Path,
        width: u32,
        height: u32,
        fps: u32,
        num_frames: u32,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let _com = ComGuard::enter().map_err(|e| format!("ComGuard: {e}"))?;
        init_mf().map_err(|e| format!("init_mf: {e}"))?;
        let path_wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        unsafe {
            let sink_writer = MFCreateSinkWriterFromURL(PCWSTR(path_wide.as_ptr()), None, None)?;

            let out_type = MFCreateMediaType()?;
            out_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            out_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            out_type.SetUINT32(&MF_MT_AVG_BITRATE, 800_000)?;
            out_type.SetUINT64(&MF_MT_FRAME_SIZE, ((width as u64) << 32) | (height as u64))?;
            out_type.SetUINT64(&MF_MT_FRAME_RATE, ((fps as u64) << 32) | 1)?;
            out_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1 << 32) | 1)?;
            out_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;

            let stream_index = sink_writer.AddStream(&out_type)?;

            let in_type = MFCreateMediaType()?;
            in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            in_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
            in_type.SetUINT64(&MF_MT_FRAME_SIZE, ((width as u64) << 32) | (height as u64))?;
            in_type.SetUINT64(&MF_MT_FRAME_RATE, ((fps as u64) << 32) | 1)?;
            in_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1 << 32) | 1)?;
            in_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;

            sink_writer.SetInputMediaType(stream_index, &in_type, None)?;
            sink_writer.BeginWriting()?;

            let frame_duration_hns = (10_000_000 / fps as i64) as u64;
            let buffer_size = width * height * 4;

            for i in 0..num_frames {
                let media_buffer = MFCreateMemoryBuffer(buffer_size)?;

                let mut ptr: *mut u8 = std::ptr::null_mut();
                let mut max_len = 0u32;
                let mut cur_len = 0u32;
                media_buffer.Lock(&mut ptr, Some(&mut max_len), Some(&mut cur_len))?;

                let slice = std::slice::from_raw_parts_mut(ptr, buffer_size as usize);
                let color_val = ((i * 255 / num_frames.max(1)) % 256) as u8;
                for px in slice.as_chunks_mut::<4>().0 {
                    px[0] = color_val; // B
                    px[1] = 128; // G
                    px[2] = 255 - color_val; // R
                    px[3] = 255; // X
                }

                media_buffer.Unlock()?;
                media_buffer.SetCurrentLength(buffer_size)?;

                let sample = MFCreateSample()?;
                sample.AddBuffer(&media_buffer)?;
                sample.SetSampleTime(i as i64 * frame_duration_hns as i64)?;
                sample.SetSampleDuration(frame_duration_hns as i64)?;

                sink_writer.WriteSample(stream_index, &sample)?;
            }

            sink_writer.Finalize()?;
            Ok(())
        }
    }
}
