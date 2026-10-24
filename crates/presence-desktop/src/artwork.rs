//! Bounded HTTPS artwork loading on a worker. Pixels cross into the UI thread.
use image::{imageops::FilterType, DynamicImage, ImageReader, RgbaImage};
use std::{io::Cursor, time::Duration};

pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct ArtworkPixels {
    pub card: Pixels,
    pub editor: Pixels,
    pub preview: Pixels,
    pub badge: Pixels,
}

#[derive(Clone)]
pub struct ArtworkImages {
    pub card: slint::Image,
    pub editor: slint::Image,
    pub preview: slint::Image,
    pub badge: slint::Image,
}

impl From<ArtworkPixels> for ArtworkImages {
    fn from(pixels: ArtworkPixels) -> Self {
        fn image(pixels: Pixels) -> slint::Image {
            slint::Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(
                &pixels.rgba,
                pixels.width,
                pixels.height,
            ))
        }
        Self {
            card: image(pixels.card),
            editor: image(pixels.editor),
            preview: image(pixels.preview),
            badge: image(pixels.badge),
        }
    }
}

pub async fn fetch(url: &str) -> Result<ArtworkPixels, String> {
    if !url.starts_with("https://") {
        return Err("Artwork requires HTTPS.".into());
    }
    let client = reqwest::Client::builder()
        .https_only(true)
        .timeout(Duration::from_secs(12))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| e.to_string())?;
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    const MAX_BYTES: usize = 8 * 1024 * 1024;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BYTES as u64)
    {
        return Err("Artwork is larger than 8 MiB.".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if bytes.len() + chunk.len() > MAX_BYTES {
            return Err("Artwork is larger than 8 MiB.".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    tokio::task::spawn_blocking(move || decode(&bytes).map(|pixels| pixels.rounded()))
        .await
        .map_err(|e| e.to_string())?
}

impl Pixels {
    pub fn rounded(self) -> ArtworkPixels {
        let source = DynamicImage::ImageRgba8(
            RgbaImage::from_raw(self.width, self.height, self.rgba)
                .expect("decoded artwork has matching dimensions"),
        );
        let side = source.width().min(source.height());
        let source = source.crop_imm(
            (source.width() - side) / 2,
            (source.height() - side) / 2,
            side,
            side,
        );
        // The software renderer cannot clip images to a rounded Rectangle.
        // Crop to cover first, then bake antialiased alpha at 3x the logical
        // display size so corners remain smooth at different Windows scales.
        fn rounded(source: &DynamicImage, size: u32, radius: f32) -> Pixels {
            let size = size * 3;
            let radius = radius * 3.0;
            let mut image = source
                .resize_exact(size, size, FilterType::Lanczos3)
                .into_rgba8();
            for (x, y, pixel) in image.enumerate_pixels_mut() {
                let dx = (radius - (x as f32 + 0.5).min(size as f32 - x as f32 - 0.5)).max(0.0);
                let dy = (radius - (y as f32 + 0.5).min(size as f32 - y as f32 - 0.5)).max(0.0);
                let coverage = (radius + 0.5 - dx.hypot(dy)).clamp(0.0, 1.0);
                pixel[3] = (pixel[3] as f32 * coverage).round() as u8;
            }
            Pixels {
                width: size,
                height: size,
                rgba: image.into_raw(),
            }
        }
        ArtworkPixels {
            card: rounded(&source, 42, 10.0),
            editor: rounded(&source, 88, 13.0),
            preview: rounded(&source, 62, 9.0),
            badge: rounded(&source, 19, 9.5),
        }
    }
}

fn decode(bytes: &[u8]) -> Result<Pixels, String> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|e| e.to_string())?
        .thumbnail(512, 512)
        .into_rgba8();
    Ok(Pixels {
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn artwork_covers_each_display_with_smooth_round_corners_and_a_circular_badge() {
        // A wide image must be center-cropped before rounding; its red sidebars
        // must not appear in any of the square displays.
        let source = image::RgbaImage::from_fn(300, 100, |x, _| {
            if (100..200).contains(&x) {
                image::Rgba([0, 255, 0, 255])
            } else {
                image::Rgba([255, 0, 0, 255])
            }
        });
        let artwork = super::Pixels {
            width: 300,
            height: 100,
            rgba: source.into_raw(),
        }
        .rounded();
        for (pixels, size) in [
            (&artwork.card, 126),
            (&artwork.editor, 264),
            (&artwork.preview, 186),
            (&artwork.badge, 57),
        ] {
            assert_eq!((pixels.width, pixels.height), (size, size));
            let pixel = |x: u32, y: u32| &pixels.rgba[((y * size + x) * 4) as usize..][..4];
            assert_eq!(pixel(0, 0)[3], 0);
            assert_eq!(pixel(size - 1, size - 1)[3], 0);
            assert_eq!(pixel(size / 2, size / 2), &[0, 255, 0, 255]);
            assert_eq!(pixel(size / 2, 0)[3], 255);
            assert!(pixels
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[3] > 0 && p[3] < 255));
        }
        let badge = &artwork.badge;
        assert_eq!(
            badge.rgba[((5 * badge.width + 5) * 4 + 3) as usize],
            0,
            "badge is circular, not a rounded square"
        );
    }

    #[test]
    fn rounding_preserves_source_transparency_and_handles_thin_images() {
        let pixels = super::Pixels {
            width: 512,
            height: 1,
            rgba: [12, 34, 56, 80].repeat(512),
        }
        .rounded()
        .editor;
        let center = ((132 * pixels.width + 132) * 4 + 3) as usize;
        assert_eq!(pixels.rgba[center], 80);
        assert_eq!(pixels.rgba[3], 0);
    }
    #[test]
    fn preview_pixels_are_bounded_before_caching() {
        let source = image::DynamicImage::new_rgba8(1024, 768);
        let mut bytes = std::io::Cursor::new(Vec::new());
        source
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let decoded = super::decode(bytes.get_ref()).unwrap();
        assert_eq!((decoded.width, decoded.height), (512, 384));
        assert_eq!(decoded.rgba.len(), 512 * 384 * 4);
    }
    #[test]
    fn malformed_artwork_is_recoverable() {
        assert!(super::decode(b"not an image").is_err());
    }
    #[tokio::test]
    async fn local_or_insecure_artwork_is_rejected() {
        assert!(super::fetch("file:///config.json").await.is_err());
        assert!(super::fetch("http://example.com/image.png").await.is_err());
    }
}
