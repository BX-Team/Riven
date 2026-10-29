use std::path::Path;

use gpui_kit::App;

/// Puts the image file at `path` on the clipboard.
pub fn copy_image(path: &Path, cx: &mut App) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    #[cfg(windows)]
    {
        let _ = cx;
        std::thread::spawn(move || {
            if let Err(e) = windows::put_image(&bytes) {
                tracing::warn!("cannot copy the image: {e}");
            }
        });
    }
    #[cfg(not(windows))]
    {
        let format = match image::guess_format(&bytes) {
            Ok(image::ImageFormat::Jpeg) => gpui_kit::ImageFormat::Jpeg,
            _ => gpui_kit::ImageFormat::Png,
        };
        let image = gpui_kit::Image::from_bytes(format, bytes);
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_image(&image));
    }
}

/// gpui offers only the registered "PNG" format, which Paint, Discord and many others ignore;
/// they read `CF_DIB`.
#[cfg(windows)]
mod windows {
    use std::io::Cursor;
    use std::time::Duration;

    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock,
    };

    const CF_DIB: u32 = 8;

    pub fn put_image(bytes: &[u8]) -> Result<(), String> {
        let decoded = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
        let png = if image::guess_format(bytes).ok() == Some(image::ImageFormat::Png) {
            bytes.to_vec()
        } else {
            let mut out = Vec::new();
            decoded
                .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
                .map_err(|e| e.to_string())?;
            out
        };
        let dib = dib(&decoded.to_rgba8());
        let png_format: Vec<u16> = "PNG\0".encode_utf16().collect();
        unsafe {
            open()?;
            EmptyClipboard();
            let result = set(CF_DIB, &dib).and_then(|()| {
                let format = RegisterClipboardFormatW(png_format.as_ptr());
                if format == 0 {
                    Ok(())
                } else {
                    set(format, &png)
                }
            });
            CloseClipboard();
            result
        }
    }

    /// Another program may hold the clipboard for a moment.
    unsafe fn open() -> Result<(), String> {
        for _ in 0..10 {
            if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        Err("the clipboard is busy".into())
    }

    unsafe fn set(format: u32, data: &[u8]) -> Result<(), String> {
        unsafe {
            let global = GlobalAlloc(GMEM_MOVEABLE, data.len());
            if global.is_null() {
                return Err("out of memory".into());
            }
            let ptr = GlobalLock(global);
            if ptr.is_null() {
                GlobalFree(global);
                return Err("cannot lock clipboard memory".into());
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr.cast(), data.len());
            GlobalUnlock(global);
            if SetClipboardData(format, global).is_null() {
                GlobalFree(global);
                return Err("cannot set clipboard data".into());
            }
        }
        Ok(())
    }

    /// A bottom-up 32-bit `BITMAPINFOHEADER` bitmap.
    fn dib(image: &image::RgbaImage) -> Vec<u8> {
        let (width, height) = image.dimensions();
        let size = width as usize * height as usize * 4;
        let mut out = Vec::with_capacity(40 + size);
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&(width as i32).to_le_bytes());
        out.extend_from_slice(&(height as i32).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(size as u32).to_le_bytes());
        out.extend_from_slice(&[0; 16]);
        for row in image.as_raw().chunks_exact(width as usize * 4).rev() {
            for pixel in row.chunks_exact(4) {
                out.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
            }
        }
        out
    }
}
