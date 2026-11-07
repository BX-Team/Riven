use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, Asset, ImageCacheError, ImageSource, RenderImage, Window};
use image::imageops::FilterType;

/// An image file scaled down on the CPU to the pixels it covers.
/// The GPU samples big textures without mipmaps, so a 256 px icon drawn at 18 px comes out jagged.
pub fn image(path: PathBuf, size: f32) -> ImageSource {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
    {
        return path.into();
    }
    ImageSource::from(move |window: &mut Window, cx: &mut App| {
        let px = (size * window.scale_factor()).ceil() as u32;
        window.use_asset::<Scaled>(&(path.clone(), px.max(1)), cx)
    })
}

enum Scaled {}

impl Asset for Scaled {
    type Source = (PathBuf, u32);
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        (path, px): Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        cx.background_executor().spawn(async move {
            let bytes = std::fs::read(&path).map_err(|e| ImageCacheError::Io(Arc::new(e)))?;
            let decoded =
                image::load_from_memory(&bytes).map_err(|e| ImageCacheError::Image(Arc::new(e)))?;
            let scaled = if decoded.width().max(decoded.height()) > px {
                decoded.resize(px, px, FilterType::Lanczos3)
            } else {
                decoded
            };
            let mut data = scaled.into_rgba8();
            for pixel in data.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
            Ok(Arc::new(RenderImage::new(vec![image::Frame::new(data)])))
        })
    }
}
