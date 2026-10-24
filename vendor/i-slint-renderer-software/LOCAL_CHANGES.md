# Presence Control desktop text patch

This is `i-slint-renderer-software` 1.18.1 from crates.io, pinned to the same
version as Slint. Original source and license notices are retained. Registry
metadata, the dependency's lockfile, and its redundant original manifest are
omitted. The workspace selects this copy with `[patch.crates-io]`; the machine's
Cargo registry is not modified.

Windows desktop text changes:

- `fonts/vectorfont.rs` enables Swash's grid-fitting hints for vector glyphs.
  Upstream leaves hinting disabled, which softens small text at desktop DPI.
- `draw_functions.rs` corrects glyph-mask coverage before applying item opacity.
  This is the common text path for labels, headings, buttons, `TextInput`, and
  standard controls, on every page. RGB/RGBA artwork and other primitives keep
  the upstream blending path.
- `text_coverage.rs` caches luminance-dependent correction curves, using the
  Skia mask-gamma algorithm with gamma 1.4 and contrast 1.0. Transparent and solid
  mask pixels are preserved. Dark text receives its own curve rather than using
  a blanket brightening filter. Tests cover all foreground luminances, smooth
  coverage, endpoints, and item opacity.

This remains the Rust software renderer. It does not introduce a browser, Skia
runtime, C++ renderer, or RGB subpixel/ClearType mode. Glyph shaping, variable
font coordinates, cache keys, advances, and application layout stay upstream.

References:

- [Swash hinting API](https://docs.rs/swash/0.2.10/swash/scale/struct.ScalerBuilder.html#method.hint)
- [Skia mask gamma algorithm](https://github.com/google/skia/blob/main/src/core/SkMaskGamma.cpp)
- [Skia's explanation of glyph coverage and blending](https://docs.skia.org/docs/dev/design/raster_tragedy/)
- [Chromium's Windows contrast changes](https://developer.chrome.com/blog/better-text-rendering-in-chromium-based-browsers-on-windows)

The mask correction follows Skia's BSD-licensed algorithm; its license is in
`LICENSES/BSD-3-Clause-Skia.txt`. Other vendored files keep Slint's license choices.

When upgrading Slint, compare the modified files against the new upstream
renderer before carrying this patch forward. Prefer an upstream rendering
option when one becomes available.
