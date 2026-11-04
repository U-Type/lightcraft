use super::{Result, Transform};
use lightcraft_raster::Rgba8;
pub fn warp(image: &Rgba8, t: &Transform, crop: [f64; 4], width: usize, height: usize) -> Result<Rgba8> {
    if crop.iter().any(|v| !v.is_finite())
        || crop[0] < 0.
        || crop[1] < 0.
        || crop[2] <= 0.
        || crop[3] <= 0.
        || crop[0] + crop[2] > 1.00000001
        || crop[1] + crop[3] > 1.00000001
    {
        return Err("crop must lie inside the reference".into());
    }
    let len = width.checked_mul(height).filter(|n| *n > 0 && *n <= 100_000_000).ok_or("output must contain 1–100 million pixels")?;
    if image.width == 0 || image.height == 0 || image.width.checked_mul(image.height) != Some(image.data.len()) {
        return Err("invalid image buffer".into());
    }
    let mut data = Vec::new();
    data.try_reserve_exact(len).map_err(|_| "not enough memory for aligned image")?;
    for y in 0..height {
        for x in 0..width {
            let p = [crop[0] + (x as f64 + 0.5) / width as f64 * crop[2], crop[1] + (y as f64 + 0.5) / height as f64 * crop[3]];
            let Some(q) = t.map(p).filter(|q| q.iter().all(|v| (0.0..=1.0).contains(v))) else {
                data.push([0, 0, 0, 0]);
                continue;
            };
            let sx = (q[0] * image.width as f64 - 0.5).clamp(0., (image.width - 1) as f64);
            let sy = (q[1] * image.height as f64 - 0.5).clamp(0., (image.height - 1) as f64);
            let (ix, iy) = (sx.floor() as usize, sy.floor() as usize);
            let (fx, fy) = (sx - ix as f64, sy - iy as f64);
            let taps = [
                (ix, iy, (1. - fx) * (1. - fy)),
                ((ix + 1).min(image.width - 1), iy, fx * (1. - fy)),
                (ix, (iy + 1).min(image.height - 1), (1. - fx) * fy),
                ((ix + 1).min(image.width - 1), (iy + 1).min(image.height - 1), fx * fy),
            ];
            let mut color = [0.; 4];
            for (tx, ty, w) in taps {
                if let Some(pixel) = image.data.get(ty * image.width + tx) {
                    for k in 0..4 {
                        color[k] += f64::from(pixel[k]) * w;
                    }
                }
            }
            data.push(color.map(|v| v.round().clamp(0., 255.) as u8));
        }
    }
    Ok(Rgba8 { width, height, data })
}
/// Largest valid rectangle on a conservative grid. For local warps this is an approximation.
pub fn common_crop(transforms: &[Transform]) -> Result<[f64; 4]> {
    if transforms.is_empty() {
        return Err("include at least one image".into());
    }
    const N: usize = 256;
    let mut heights = [0usize; N];
    let mut best = (0, 0, 0, 0, 0);
    for y in 0..N {
        for (x, h) in heights.iter_mut().enumerate() {
            let valid = [(0., 0.), (1., 0.), (0., 1.), (1., 1.), (0.5, 0.5)]
                .iter()
                .all(|(dx, dy)| transforms.iter().all(|t| t.valid([(x as f64 + dx) / N as f64, (y as f64 + dy) / N as f64])));
            *h = if valid { *h + 1 } else { 0 };
        }
        let mut stack: Vec<(usize, usize)> = vec![];
        for x in 0..=N {
            let h = heights.get(x).copied().unwrap_or(0);
            let mut left = x;
            while let Some(&(start, old)) = stack.last() {
                if old <= h {
                    break;
                }
                stack.pop();
                let area = old * (x - start);
                if area > best.0 {
                    best = (area, start, y + 1 - old, x - start, old);
                }
                left = start;
            }
            stack.push((left, h));
        }
    }
    if best.0 == 0 {
        return Err("no common rectangular overlap".into());
    }
    Ok([best.1 as f64 / N as f64, best.2 as f64 / N as f64, best.3 as f64 / N as f64, best.4 as f64 / N as f64])
}
