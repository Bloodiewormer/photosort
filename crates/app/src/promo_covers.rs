use catalog::{AssetCover, AssetId, Catalog, OcrLine, Status, WorkLineMatch, WorkMatcher};
use image::{DynamicImage, GenericImageView, RgbImage};
use std::path::Path;

/// Analysis stage that records the cover regions of an image as detected. Bump the version when
/// the detection changes, so every promotional page is decoded again once.
pub const COVERS_STAGE: &str = "covers";
pub const COVERS_VERSION: u32 = 1;

/// Minimum number of cover cards required to consider a page a promotional grid.
const MIN_COVERS: usize = 2;
/// Maximum number of cover cards in a typical grid.
const MAX_COVERS: usize = 24;

/// Detects rectangular cover regions forming a grid on a promotional page.
/// Returns bounding boxes `[x0, y0, x1, y1]` in original image pixels.
/// Returns an empty Vec if the image does not contain a typical card grid.
// shortcut: the page background is the mean colour of the outer border, so strong gradients,
// decorative frames and cards that touch without a gutter give no regions (soft gradients may
// lose some cards); upgrade to a local-contrast or learned detector if real pages need it.
pub fn detect_cover_regions(img: &DynamicImage) -> Vec<[f32; 4]> {
    let (orig_w, orig_h) = img.dimensions();
    if orig_w < 100 || orig_h < 100 {
        return Vec::new();
    }

    // Process on a standardized working size for speed and noise reduction
    let max_dim = 600.0f32;
    let scale = (max_dim / (orig_w.max(orig_h) as f32)).min(1.0);
    let proc_w = ((orig_w as f32) * scale).round().max(10.0) as u32;
    let proc_h = ((orig_h as f32) * scale).round().max(10.0) as u32;
    let rgb: RgbImage = if scale < 0.999 {
        img.resize_exact(proc_w, proc_h, image::imageops::FilterType::Triangle)
            .to_rgb8()
    } else {
        img.to_rgb8()
    };

    // 1. Estimate background color from outer perimeter pixels
    let mut border_r = Vec::new();
    let mut border_g = Vec::new();
    let mut border_b = Vec::new();

    let border_thick_x = 2.max(proc_w / 50);
    let border_thick_y = 2.max(proc_h / 50);

    for y in 0..proc_h {
        for x in 0..proc_w {
            if x < border_thick_x
                || x >= proc_w - border_thick_x
                || y < border_thick_y
                || y >= proc_h - border_thick_y
            {
                let p = rgb.get_pixel(x, y);
                border_r.push(p[0] as f32);
                border_g.push(p[1] as f32);
                border_b.push(p[2] as f32);
            }
        }
    }

    if border_r.is_empty() {
        return Vec::new();
    }

    let n = border_r.len() as f32;
    let mean_r = border_r.iter().sum::<f32>() / n;
    let mean_g = border_g.iter().sum::<f32>() / n;
    let mean_b = border_b.iter().sum::<f32>() / n;

    let var_r = border_r.iter().map(|&v| (v - mean_r).powi(2)).sum::<f32>() / n;
    let var_g = border_g.iter().map(|&v| (v - mean_g).powi(2)).sum::<f32>() / n;
    let var_b = border_b.iter().map(|&v| (v - mean_b).powi(2)).sum::<f32>() / n;
    let border_std = (var_r + var_g + var_b).sqrt();

    // If the border is too noisy, it's a full-bleed photo or irregular scene, not a grid with background
    if border_std > 55.0 {
        return Vec::new();
    }

    let bg = [mean_r as u8, mean_g as u8, mean_b as u8];

    // 2. Binary difference and gradient mask
    let mut mask = vec![vec![false; proc_w as usize]; proc_h as usize];
    for y in 0..proc_h {
        for x in 0..proc_w {
            let p = rgb.get_pixel(x, y);
            let diff = (p[0] as i32 - bg[0] as i32).abs()
                + (p[1] as i32 - bg[1] as i32).abs()
                + (p[2] as i32 - bg[2] as i32).abs();
            if diff > 35 {
                mask[y as usize][x as usize] = true;
            } else if x + 1 < proc_w && y + 1 < proc_h {
                let px = rgb.get_pixel(x + 1, y);
                let py = rgb.get_pixel(x, y + 1);
                let grad = (p[0] as i32 - px[0] as i32).abs()
                    + (p[1] as i32 - px[1] as i32).abs()
                    + (p[2] as i32 - px[2] as i32).abs()
                    + (p[0] as i32 - py[0] as i32).abs()
                    + (p[1] as i32 - py[1] as i32).abs()
                    + (p[2] as i32 - py[2] as i32).abs();
                if grad > 40 {
                    mask[y as usize][x as usize] = true;
                }
            }
        }
    }

    // 3. Try projection profile segmentation (gutters / bands)
    let candidates_proj = extract_grid_by_projection(&mask, proc_w, proc_h);
    let mut chosen_candidates = if is_valid_grid(&candidates_proj) {
        candidates_proj
    } else {
        // Fallback: connected components labeling
        let candidates_ccl = extract_grid_by_ccl(&mask, proc_w, proc_h);
        if is_valid_grid(&candidates_ccl) {
            candidates_ccl
        } else {
            Vec::new()
        }
    };

    if chosen_candidates.is_empty() {
        return Vec::new();
    }

    // Sort in row-major reading order (top-to-bottom, left-to-right)
    sort_grid_row_major(&mut chosen_candidates);

    // Scale back to original coordinates and clamp
    chosen_candidates
        .into_iter()
        .map(|b| {
            [
                (b[0] / scale).clamp(0.0, orig_w as f32),
                (b[1] / scale).clamp(0.0, orig_h as f32),
                (b[2] / scale).clamp(0.0, orig_w as f32),
                (b[3] / scale).clamp(0.0, orig_h as f32),
            ]
        })
        .collect()
}

fn smooth_1d(arr: &[f32], radius: usize) -> Vec<f32> {
    let n = arr.len();
    let mut out = vec![0.0f32; n];
    for (i, item) in out.iter_mut().enumerate() {
        let start = i.saturating_sub(radius);
        let end = (i + radius + 1).min(n);
        let count = (end - start) as f32;
        let sum: f32 = arr[start..end].iter().sum();
        *item = sum / count;
    }
    out
}

fn merge_segments(segments: Vec<(u32, u32)>, max_gap: u32) -> Vec<(u32, u32)> {
    if segments.is_empty() {
        return Vec::new();
    }
    let mut merged = Vec::new();
    let mut cur = segments[0];
    for next in segments.into_iter().skip(1) {
        if next.0 <= cur.1 + max_gap {
            cur.1 = cur.1.max(next.1);
        } else {
            merged.push(cur);
            cur = next;
        }
    }
    merged.push(cur);
    merged
}

fn extract_grid_by_projection(mask: &[Vec<bool>], w: u32, h: u32) -> Vec<[f32; 4]> {
    // 1. Column density
    let mut col_density = vec![0.0f32; w as usize];
    for x in 0..w as usize {
        let count = mask.iter().filter(|row| row[x]).count();
        col_density[x] = count as f32 / h as f32;
    }
    let smoothed_col = smooth_1d(&col_density, 2);

    // Find column intervals with density > 0.08
    let mut raw_cols = Vec::new();
    let mut in_col = false;
    let mut start = 0;
    for (x, &d) in smoothed_col.iter().enumerate() {
        if d > 0.08 {
            if !in_col {
                in_col = true;
                start = x;
            }
        } else if in_col {
            in_col = false;
            let width = x - start;
            if width as f32 >= 0.10 * w as f32 && width as f32 <= 0.60 * w as f32 {
                raw_cols.push((start as u32, x as u32));
            }
        }
    }
    if in_col {
        let width = w as usize - start;
        if width as f32 >= 0.10 * w as f32 && width as f32 <= 0.60 * w as f32 {
            raw_cols.push((start as u32, w));
        }
    }

    // Merge adjacent columns if gap is tiny (< 4 px)
    let cols = merge_segments(raw_cols, 4);

    if cols.is_empty() || cols.len() > 5 {
        return Vec::new();
    }

    let mut boxes = Vec::new();
    for (cx0, cx1) in cols {
        let cw = (cx1 - cx0).max(1) as f32;
        let mut row_density = vec![0.0f32; h as usize];
        for y in 0..h as usize {
            let count = (cx0 as usize..cx1 as usize).filter(|&x| mask[y][x]).count();
            row_density[y] = count as f32 / cw;
        }
        let smoothed_row = smooth_1d(&row_density, 2);

        let mut in_row = false;
        let mut r_start = 0;
        let mut raw_rows = Vec::new();

        for (y, &d) in smoothed_row.iter().enumerate() {
            if d > 0.18 {
                if !in_row {
                    in_row = true;
                    r_start = y;
                }
            } else if in_row {
                in_row = false;
                let rh = (y - r_start) as f32;
                let aspect = cw / rh.max(1.0);
                if rh >= 0.08 * h as f32 && rh <= 0.70 * h as f32 && (0.40..=1.60).contains(&aspect)
                {
                    raw_rows.push((r_start as u32, y as u32));
                }
            }
        }
        if in_row {
            let rh = (h as usize - r_start) as f32;
            let aspect = cw / rh.max(1.0);
            if rh >= 0.08 * h as f32 && rh <= 0.70 * h as f32 && (0.40..=1.60).contains(&aspect) {
                raw_rows.push((r_start as u32, h));
            }
        }

        let merged_rows = merge_segments(raw_rows, 4);
        for (ry0, ry1) in merged_rows {
            boxes.push([cx0 as f32, ry0 as f32, cx1 as f32, ry1 as f32]);
        }
    }

    boxes
}

fn extract_grid_by_ccl(mask: &[Vec<bool>], w: u32, h: u32) -> Vec<[f32; 4]> {
    let mut visited = vec![vec![false; w as usize]; h as usize];
    let mut boxes = Vec::new();

    for y in 0..h as usize {
        for x in 0..w as usize {
            if mask[y][x] && !visited[y][x] {
                let mut min_x = x;
                let mut max_x = x;
                let mut min_y = y;
                let mut max_y = y;
                let mut count = 0usize;

                let mut queue = std::collections::VecDeque::new();
                queue.push_back((x, y));
                visited[y][x] = true;

                while let Some((qx, qy)) = queue.pop_front() {
                    count += 1;
                    min_x = min_x.min(qx);
                    max_x = max_x.max(qx);
                    min_y = min_y.min(qy);
                    max_y = max_y.max(qy);

                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            if dx == 0 && dy == 0 {
                                continue;
                            }
                            let nx = qx as isize + dx;
                            let ny = qy as isize + dy;
                            if nx >= 0 && nx < w as isize && ny >= 0 && ny < h as isize {
                                let (ux, uy) = (nx as usize, ny as usize);
                                if mask[uy][ux] && !visited[uy][ux] {
                                    visited[uy][ux] = true;
                                    queue.push_back((ux, uy));
                                }
                            }
                        }
                    }
                }

                let bw = (max_x - min_x + 1) as f32;
                let bh = (max_y - min_y + 1) as f32;
                let aspect = bw / bh.max(1.0);
                let fill = count as f32 / (bw * bh).max(1.0);

                if bw >= 0.08 * w as f32
                    && bw <= 0.60 * w as f32
                    && bh >= 0.08 * h as f32
                    && bh <= 0.70 * h as f32
                    && (0.40..=1.60).contains(&aspect)
                    && fill >= 0.30
                {
                    boxes.push([
                        min_x as f32,
                        min_y as f32,
                        (max_x + 1) as f32,
                        (max_y + 1) as f32,
                    ]);
                }
            }
        }
    }

    boxes
}

fn box_iou(r1: [f32; 4], r2: [f32; 4]) -> f32 {
    let inter_x0 = r1[0].max(r2[0]);
    let inter_y0 = r1[1].max(r2[1]);
    let inter_x1 = r1[2].min(r2[2]);
    let inter_y1 = r1[3].min(r2[3]);
    let inter_w = (inter_x1 - inter_x0).max(0.0);
    let inter_h = (inter_y1 - inter_y0).max(0.0);
    let inter_area = inter_w * inter_h;
    let area1 = (r1[2] - r1[0]).max(0.0) * (r1[3] - r1[1]).max(0.0);
    let area2 = (r2[2] - r2[0]).max(0.0) * (r2[3] - r2[1]).max(0.0);
    let union_area = area1 + area2 - inter_area;
    if union_area <= 0.0 {
        0.0
    } else {
        inter_area / union_area
    }
}

fn count_clusters(values: &[f32], tolerance: f32) -> usize {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let mut clusters = 1;
    let mut cur = sorted[0];
    for &v in &sorted[1..] {
        if v - cur > tolerance {
            clusters += 1;
            cur = v;
        }
    }
    clusters
}

fn is_valid_grid(boxes: &[[f32; 4]]) -> bool {
    let n = boxes.len();
    if !(MIN_COVERS..=MAX_COVERS).contains(&n) {
        return false;
    }

    // Check no significant overlaps between any pair
    for i in 0..n {
        for j in i + 1..n {
            if box_iou(boxes[i], boxes[j]) > 0.15 {
                return false;
            }
        }
    }

    // Size consistency: standard deviation of width and height must be within bounds
    let widths: Vec<f32> = boxes.iter().map(|b| b[2] - b[0]).collect();
    let heights: Vec<f32> = boxes.iter().map(|b| b[3] - b[1]).collect();

    let mean_w = widths.iter().sum::<f32>() / n as f32;
    let mean_h = heights.iter().sum::<f32>() / n as f32;

    if mean_w <= 1.0 || mean_h <= 1.0 {
        return false;
    }

    let var_w = widths.iter().map(|&v| (v - mean_w).powi(2)).sum::<f32>() / n as f32;
    let var_h = heights.iter().map(|&v| (v - mean_h).powi(2)).sum::<f32>() / n as f32;

    let std_w = var_w.sqrt();
    let std_h = var_h.sqrt();

    if std_w / mean_w > 0.30 || std_h / mean_h > 0.30 {
        return false;
    }

    // Grid alignment: cards should form columns and/or rows
    let col_centers: Vec<f32> = boxes.iter().map(|b| (b[0] + b[2]) / 2.0).collect();
    let row_centers: Vec<f32> = boxes.iter().map(|b| (b[1] + b[3]) / 2.0).collect();

    let num_cols = count_clusters(&col_centers, mean_w * 0.4);
    let num_rows = count_clusters(&row_centers, mean_h * 0.4);

    (num_cols >= 2 || num_rows >= 2) && num_cols >= 1 && num_rows >= 1
}

fn sort_grid_row_major(boxes: &mut [[f32; 4]]) {
    if boxes.is_empty() {
        return;
    }
    let avg_h = boxes.iter().map(|b| b[3] - b[1]).sum::<f32>() / boxes.len() as f32;
    let row_threshold = (avg_h * 0.4).max(10.0);

    boxes.sort_by(|a, b| {
        let diff_y = a[1] - b[1];
        if diff_y.abs() > row_threshold {
            diff_y.total_cmp(&0.0)
        } else {
            a[0].total_cmp(&b[0])
        }
    });
}

/// Associates detected cover regions with matching works based on spatial proximity.
/// Matches each OCR line/work to the closest region: inside it, just below, or just above.
pub fn associate_covers(
    asset: AssetId,
    covers: &[[f32; 4]],
    works: &[WorkLineMatch],
    cat: &Catalog,
) -> Vec<AssetCover> {
    if covers.is_empty() {
        return Vec::new();
    }

    struct PairCandidate {
        cover_idx: usize,
        work_idx: usize,
        dist: f32,
    }

    let mut pair_candidates = Vec::new();

    for (c_idx, &cover) in covers.iter().enumerate() {
        let c_w = cover[2] - cover[0];
        let c_h = cover[3] - cover[1];
        let c_xmid = (cover[0] + cover[2]) / 2.0;
        let max_dist = (c_h * 0.9).max(120.0);

        for (w_idx, work) in works.iter().enumerate() {
            let l = work.rect;
            let l_w = l[2] - l[0];
            let l_xmid = (l[0] + l[2]) / 2.0;
            let l_ymid = (l[1] + l[3]) / 2.0;

            // Check 1: Inside the cover region
            let inside = l_xmid >= cover[0] - 15.0
                && l_xmid <= cover[2] + 15.0
                && l_ymid >= cover[1] - 15.0
                && l_ymid <= cover[3] + 15.0;

            if inside {
                // Zero only when truly inside; within the slack, how far outside it is.
                let out_x = (cover[0] - l_xmid).max(l_xmid - cover[2]).max(0.0);
                let out_y = (cover[1] - l_ymid).max(l_ymid - cover[3]).max(0.0);
                pair_candidates.push(PairCandidate {
                    cover_idx: c_idx,
                    work_idx: w_idx,
                    dist: out_x + out_y,
                });
                continue;
            }

            // Check horizontal alignment
            let overlap_x = cover[2].min(l[2]) - cover[0].max(l[0]);
            let dx = (l_xmid - c_xmid).abs();
            let horizontally_aligned = overlap_x > 0.0 || dx <= 0.45 * (c_w + l_w);

            if !horizontally_aligned {
                continue;
            }

            // Check 2: Just below the cover
            if l[1] >= cover[3] - 15.0 {
                let dy = (l[1] - cover[3]).max(0.0);
                if dy <= max_dist {
                    pair_candidates.push(PairCandidate {
                        cover_idx: c_idx,
                        work_idx: w_idx,
                        dist: dy + 0.3 * dx,
                    });
                    continue;
                }
            }

            // Check 3: Just above the cover
            if l[3] <= cover[1] + 15.0 {
                let dy = (cover[1] - l[3]).max(0.0);
                if dy <= max_dist {
                    pair_candidates.push(PairCandidate {
                        cover_idx: c_idx,
                        work_idx: w_idx,
                        dist: dy + 0.3 * dx,
                    });
                }
            }
        }
    }

    // A title about as close to two covers (e.g. in the gutter between two rows) is ambiguous:
    // leaving it unassigned beats giving it to the wrong card.
    let mean_h = covers.iter().map(|c| c[3] - c[1]).sum::<f32>() / covers.len() as f32;
    let tie = (0.02 * mean_h).max(1.0);
    let ambiguous: Vec<bool> = (0..works.len())
        .map(|w| {
            let mut d: Vec<f32> = pair_candidates
                .iter()
                .filter(|c| c.work_idx == w)
                .map(|c| c.dist)
                .collect();
            d.sort_by(f32::total_cmp);
            d.len() >= 2 && d[1] - d[0] <= tie
        })
        .collect();
    pair_candidates.retain(|c| !ambiguous[c.work_idx]);

    // Sort candidates by distance ascending
    pair_candidates.sort_by(|a, b| a.dist.total_cmp(&b.dist));

    // Greedy 1-to-1 matching
    let mut cover_assigned: Vec<Option<usize>> = vec![None; covers.len()];
    let mut work_assigned = vec![false; works.len()];

    for cand in pair_candidates {
        if cover_assigned[cand.cover_idx].is_none() && !work_assigned[cand.work_idx] {
            cover_assigned[cand.cover_idx] = Some(cand.work_idx);
            work_assigned[cand.work_idx] = true;
        }
    }

    // Construct AssetCover results
    let mut result = Vec::with_capacity(covers.len());
    for (c_idx, &rect) in covers.iter().enumerate() {
        let (work_id, work_name, confidence) = if let Some(w_idx) = cover_assigned[c_idx] {
            let w = &works[w_idx];
            let name = cat.label_name(w.label).ok().flatten();
            (Some(w.label), name, w.confidence)
        } else {
            (None, None, 0.5)
        };

        result.push(AssetCover {
            id: None,
            asset,
            cover_idx: c_idx,
            rect,
            work: work_id,
            work_name,
            confidence,
            status: Status::Suggested,
        });
    }

    result
}

/// Detects covers and associates titles on a promotional page.
pub fn detect_and_associate(
    asset: AssetId,
    img: &DynamicImage,
    lines: &[OcrLine],
    matcher: &WorkMatcher,
    cat: &Catalog,
    is_screenshot: bool,
) -> Vec<AssetCover> {
    let covers = detect_cover_regions(img);
    if covers.is_empty() {
        return Vec::new();
    }

    let matches = matcher.match_asset(lines, is_screenshot);
    associate_covers(asset, &covers, &matches.work_rects, cat)
}

/// Updates the cover regions of a promotional page and their works. The image is decoded only the
/// first time (per `COVERS_VERSION`); later runs reuse the stored regions and only redo the
/// association, which changes when works or their aliases change.
pub fn refresh_covers(
    cat: &mut Catalog,
    asset: AssetId,
    path: &Path,
    works: &[WorkLineMatch],
) -> Result<(), String> {
    let done = cat
        .is_stage_done(asset, COVERS_STAGE, COVERS_VERSION)
        .map_err(|e| e.to_string())?;
    let regions: Vec<[f32; 4]> = if done {
        let stored = cat.asset_covers(asset).map_err(|e| e.to_string())?;
        stored.into_iter().map(|c| c.rect).collect()
    } else {
        // Full size so the regions share the pixel space of the OCR lines.
        let img = thumbs::decode(path, u32::MAX).map_err(|e| e.to_string())?;
        detect_cover_regions(&img)
    };
    let covers = associate_covers(asset, &regions, works, cat);
    cat.save_asset_covers(asset, &covers)
        .map_err(|e| e.to_string())?;
    cat.mark_done(asset, COVERS_STAGE, COVERS_VERSION)
        .map_err(|e| e.to_string())
}
