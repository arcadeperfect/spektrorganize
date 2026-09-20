//! The develop stage's geometry: quarter turns, straightening, and a crop.
//!
//! All three are stored with the photo and applied to the decoded linear image before the film
//! stage, so a print is made from the same frame you see in Develop. Nothing is ever written back
//! to the photo itself.
//!
//! Order is fixed: quarter turns, then the straighten angle, then the crop. Straightening trims to
//! the largest rectangle of the original aspect that fits inside the rotated frame, so the result
//! never has empty corners; the crop rectangle is then a fraction of what is left.

use crate::decode::{Crop, LinearImage, RawSettings};
use rayon::prelude::*;

/// Apply the photo's geometry. Returns `img` untouched when there is nothing to do.
pub fn apply(img: LinearImage, raw: &RawSettings) -> LinearImage {
    let img = quarter_turns(img, raw.rotate);
    let img = straighten(img, raw.straighten);
    match &raw.crop {
        Some(c) => crop(img, c),
        None => img,
    }
}

/// `n` quarter turns clockwise (negative turns anticlockwise).
pub fn quarter_turns(img: LinearImage, n: i32) -> LinearImage {
    let n = n.rem_euclid(4);
    if n == 0 {
        return img;
    }
    let (w, h) = (img.width as usize, img.height as usize);
    let (ow, oh) = if n % 2 == 1 { (h, w) } else { (w, h) };
    let mut out = vec![0.0f32; ow * oh * 3];
    out.par_chunks_exact_mut(ow * 3).enumerate().for_each(|(oy, row)| {
        for ox in 0..ow {
            // Where this output pixel comes from in the source.
            let (sx, sy) = match n {
                1 => (oy, h - 1 - ox),          // 90° clockwise
                2 => (w - 1 - ox, h - 1 - oy),  // 180°
                _ => (w - 1 - oy, ox),          // 270° clockwise
            };
            let s = (sy * w + sx) * 3;
            row[ox * 3..ox * 3 + 3].copy_from_slice(&img.data[s..s + 3]);
        }
    });
    LinearImage { width: ow as u32, height: oh as u32, data: out, color_space: img.color_space }
}

/// Rotate by `degrees` (positive = clockwise) and trim to the largest rectangle of the same
/// aspect that fits inside, so there are no empty corners. Bilinear sampling.
pub fn straighten(img: LinearImage, degrees: f64) -> LinearImage {
    if degrees.abs() < 0.001 {
        return img;
    }
    let a = degrees.to_radians();
    let (w, h) = (img.width as f64, img.height as f64);
    let (cw, ch) = inscribed(w, h, a);
    let (ow, oh) = ((cw.floor() as u32).max(1), (ch.floor() as u32).max(1));
    let (cx, cy) = (w / 2.0, h / 2.0);
    let (ocx, ocy) = (ow as f64 / 2.0, oh as f64 / 2.0);
    let (sin, cos) = ((-a).sin(), (-a).cos());
    let sw = img.width as usize;
    let sh = img.height as usize;

    let mut out = vec![0.0f32; ow as usize * oh as usize * 3];
    out.par_chunks_exact_mut(ow as usize * 3).enumerate().for_each(|(oy, row)| {
        let dy = oy as f64 + 0.5 - ocy;
        for ox in 0..ow as usize {
            let dx = ox as f64 + 0.5 - ocx;
            // Inverse rotation: where in the source this output pixel sits.
            let sx = cx + dx * cos - dy * sin - 0.5;
            let sy = cy + dx * sin + dy * cos - 0.5;
            let px = sample(&img.data, sw, sh, sx, sy);
            row[ox * 3..ox * 3 + 3].copy_from_slice(&px);
        }
    });
    LinearImage { width: ow, height: oh, data: out, color_space: img.color_space }
}

/// Cut out `c` (fractions of the frame, from the top left).
pub fn crop(img: LinearImage, c: &Crop) -> LinearImage {
    let (w, h) = (img.width as f64, img.height as f64);
    let x0 = (c.x.clamp(0.0, 1.0) * w).round() as u32;
    let y0 = (c.y.clamp(0.0, 1.0) * h).round() as u32;
    let cw = ((c.w.clamp(0.0, 1.0) * w).round() as u32).min(img.width.saturating_sub(x0)).max(1);
    let ch = ((c.h.clamp(0.0, 1.0) * h).round() as u32).min(img.height.saturating_sub(y0)).max(1);
    if x0 == 0 && y0 == 0 && cw == img.width && ch == img.height {
        return img;
    }
    let sw = img.width as usize;
    let mut out = vec![0.0f32; cw as usize * ch as usize * 3];
    out.par_chunks_exact_mut(cw as usize * 3).enumerate().for_each(|(y, row)| {
        let s = ((y + y0 as usize) * sw + x0 as usize) * 3;
        row.copy_from_slice(&img.data[s..s + cw as usize * 3]);
    });
    LinearImage { width: cw, height: ch, data: out, color_space: img.color_space }
}

/// The largest rectangle with the aspect of `w`×`h` that fits inside that rectangle rotated by
/// `a` radians, centred on it. An axis-aligned `w'`×`h'` fits when
/// `w'·cos + h'·sin ≤ w` and `w'·sin + h'·cos ≤ h`; holding the aspect leaves one unknown.
fn inscribed(w: f64, h: f64, a: f64) -> (f64, f64) {
    let (sin, cos) = (a.sin().abs(), a.cos().abs());
    let r = w / h;
    let height = (w / (r * cos + sin)).min(h / (r * sin + cos));
    (r * height, height)
}

/// Bilinear sample, clamping at the edges.
fn sample(data: &[f32], w: usize, h: usize, x: f64, y: f64) -> [f32; 3] {
    let x = x.clamp(0.0, (w - 1) as f64);
    let y = y.clamp(0.0, (h - 1) as f64);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
    let at = |px: usize, py: usize| -> &[f32] { &data[(py * w + px) * 3..(py * w + px) * 3 + 3] };
    let (p00, p10, p01, p11) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
    let mut out = [0.0f32; 3];
    for i in 0..3 {
        let top = p00[i] + (p10[i] - p00[i]) * fx;
        let bottom = p01[i] + (p11[i] - p01[i]) * fx;
        out[i] = top + (bottom - top) * fy;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(w: u32, h: u32) -> LinearImage {
        let mut data = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                data.extend_from_slice(&[x as f32, y as f32, 0.0]);
            }
        }
        LinearImage { width: w, height: h, data, color_space: "Rec. 2020" }
    }

    fn px(img: &LinearImage, x: u32, y: u32) -> [f32; 3] {
        let i = ((y * img.width + x) * 3) as usize;
        [img.data[i], img.data[i + 1], img.data[i + 2]]
    }

    #[test]
    fn quarter_turn_moves_the_corners_the_right_way() {
        let img = ramp(4, 2); // top-left is (0,0), top-right is (3,0)
        let r = quarter_turns(img.clone(), 1);
        assert_eq!((r.width, r.height), (2, 4));
        // 90° clockwise puts the old top-left at the top-right.
        assert_eq!(px(&r, 1, 0), [0.0, 0.0, 0.0]);
        // Four turns is the original.
        let back = quarter_turns(quarter_turns(quarter_turns(quarter_turns(img.clone(), 1), 1), 1), 1);
        assert_eq!(back.data, img.data);
        // -1 and 3 are the same turn.
        assert_eq!(quarter_turns(img.clone(), -1).data, quarter_turns(img, 3).data);
    }

    #[test]
    fn straighten_keeps_the_aspect_and_fills_every_pixel() {
        let img = ramp(400, 300);
        let out = straighten(img.clone(), 5.0);
        assert!(out.width < img.width && out.height < img.height, "it trims to fit inside");
        let want = img.width as f64 / img.height as f64;
        let got = out.width as f64 / out.height as f64;
        assert!((want - got).abs() < 0.02, "aspect kept: {want} vs {got}");
        // No empty corners: every sample came from inside the frame, so the red
        // channel (the x ramp) is never negative and never past the right edge.
        assert!(out.data.chunks_exact(3).all(|p| p[0] >= 0.0 && p[0] <= 399.0));
        // A zero angle is a no-op.
        assert_eq!(straighten(img.clone(), 0.0).data, img.data);
    }

    #[test]
    fn crop_takes_the_right_rectangle() {
        let img = ramp(100, 50);
        let out = crop(img, &Crop { x: 0.5, y: 0.0, w: 0.5, h: 1.0 });
        assert_eq!((out.width, out.height), (50, 50));
        // The first pixel of the crop is x = 50 in the source.
        assert_eq!(px(&out, 0, 0), [50.0, 0.0, 0.0]);
    }
}

#[cfg(test)]
mod bench {
    use super::*;

    fn img(w: u32, h: u32) -> LinearImage {
        LinearImage { width: w, height: h, data: vec![0.5; (w * h * 3) as usize], color_space: "Rec. 2020" }
    }

    #[test]
    #[ignore = "timing, run with --ignored --nocapture"]
    fn how_long_does_straighten_take() {
        for (w, h) in [(1600u32, 1067u32), (2400, 1600), (4896, 3264)] {
            let src = img(w, h);
            let t = std::time::Instant::now();
            let out = straighten(src.clone(), 3.0);
            println!("straighten {w}x{h} -> {}x{}: {:?}", out.width, out.height, t.elapsed());
            let t = std::time::Instant::now();
            let _ = src.clone();
            println!("  (a bare clone of the same image: {:?})", t.elapsed());
        }
    }
}
