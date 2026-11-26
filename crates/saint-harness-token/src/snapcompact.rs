#[cfg(feature = "snapcompact")]
pub fn rasterize_summary_png(lines: &[String]) -> Result<Vec<u8>, String> {
    let width = 800u32;
    let height = ((lines.len().max(1) as u32) * 16 + 32).min(2048);
    let mut pixmap = tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| "failed to allocate snapcompact pixmap".to_string())?;
    pixmap.fill(tiny_skia::Color::from_rgba8(17, 17, 17, 255));

    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer
            .write_image_data(pixmap.data())
            .map_err(|e| e.to_string())?;
    }
    Ok(out)
}

#[cfg(not(feature = "snapcompact"))]
pub fn rasterize_summary_png(_lines: &[String]) -> Result<Vec<u8>, String> {
    Err("snapcompact feature is not enabled in this build".to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn snapcompact_behaves_according_to_feature_flag() {
        let res = rasterize_summary_png(&["line 1".to_string(), "line 2".to_string()]);
        #[cfg(feature = "snapcompact")]
        {
            let bytes = res.unwrap();
            assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        }
        #[cfg(not(feature = "snapcompact"))]
        {
            assert!(res.is_err());
        }
    }
}
