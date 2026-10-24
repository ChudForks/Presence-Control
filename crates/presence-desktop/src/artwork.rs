//! Bounded HTTPS artwork loading on a worker. Pixels cross into the UI thread.
use image::ImageReader;
use std::{io::Cursor, time::Duration};

pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub async fn fetch(url: &str) -> Result<Pixels, String> {
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
    tokio::task::spawn_blocking(move || decode(&bytes))
        .await
        .map_err(|e| e.to_string())?
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
