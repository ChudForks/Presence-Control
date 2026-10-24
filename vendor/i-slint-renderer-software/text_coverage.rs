// Copyright 2012 Google Inc.
// SPDX-License-Identifier: BSD-3-Clause
//
// Based on Skia's mask gamma correction algorithm, implemented in Rust.
// See LICENSES/BSD-3-Clause-Skia.txt and LOCAL_CHANGES.md.

//! Correct glyph coverage for display blending without changing font weights.

use i_slint_core::Color;
use std::boxed::Box;
use std::sync::OnceLock;
use std::vec::Vec;

// Chromium/Skia's desktop defaults. Contrast is tapered with the inverse
// foreground luminance, so light and dark text retain their intended weight.
const GAMMA: f32 = 1.4;
const CONTRAST: f32 = 1.0;

pub(crate) fn table(color: Color) -> &'static [u8; 256] {
    static TABLES: OnceLock<Box<[[u8; 256]]>> = OnceLock::new();
    let tables = TABLES.get_or_init(|| {
        (0..=255)
            .map(build_table)
            .collect::<Vec<_>>()
            .into_boxed_slice()
    });
    let luminance = (54 * u32::from(color.red())
        + 183 * u32::from(color.green())
        + 19 * u32::from(color.blue())
        + 128)
        >> 8;
    &tables[luminance as usize]
}

fn build_table(luminance: u8) -> [u8; 256] {
    let foreground = f32::from(luminance) / 255.0;
    let background = 1.0 - foreground;
    let linear_foreground = foreground.powf(GAMMA);
    let linear_background = background.powf(GAMMA);
    let contrast = CONTRAST * linear_background;
    core::array::from_fn(|index| {
        let coverage = index as f32 / 255.0;
        let coverage = coverage + coverage * (1.0 - coverage) * contrast;
        let corrected = if (foreground - background).abs() < 1.0 / 256.0 {
            coverage
        } else {
            let linear_output = linear_foreground * coverage + linear_background * (1.0 - coverage);
            let output = linear_output.powf(1.0 / GAMMA);
            (output - background) / (foreground - background)
        };
        (corrected.clamp(0.0, 1.0) * 255.0).round() as u8
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_text_colors_preserve_transparency_solids_and_smooth_coverage() {
        for luminance in 0..=255 {
            let curve = build_table(luminance);
            assert_eq!(curve[0], 0, "transparent pixels must stay transparent");
            assert_eq!(curve[255], 255, "solid strokes must keep the text color");
            assert!(
                curve.windows(2).all(|p| p[0] <= p[1]),
                "coverage must be monotonic"
            );
        }
    }

    #[test]
    fn dark_and_light_text_get_display_contrast_without_changing_opacity() {
        let light = table(Color::from_rgb_u8(242, 242, 242));
        let dark = table(Color::from_rgb_u8(21, 21, 21));
        assert!(
            light[128] > 128,
            "light text edges must no longer look under-covered"
        );
        assert!(
            dark[128] > 128,
            "dark button text needs its own luminance correction"
        );
        assert_ne!(light[128], dark[128]);
        // Item opacity is multiplied after lookup, never gamma-corrected itself.
        assert_eq!(u16::from(light[255]) * 96 / 255, 96);
    }
}
