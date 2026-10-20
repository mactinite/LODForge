//! One-dimensional greedy simplification of a border profile: the same idea as the greedy
//! insertion of the mesher, on a line. Starting from the fixed points, the candidate with the
//! largest distance to the polyline through the selected points is added until that distance is
//! at or below the threshold. Used for the edges shared by neighbouring quads and cells, so that
//! both sides select the same points.

/// Worst candidate strictly between the selected points `a` and `b`: (error, index).
fn worst(
    heights: &[f32],
    candidate: &[bool],
    selected: &[bool],
    a: usize,
    b: usize,
) -> (f64, usize) {
    let (za, zb) = (f64::from(heights[a]), f64::from(heights[b]));
    let mut best = (-1.0, a);
    for i in a + 1..b {
        if !candidate[i] || selected[i] {
            continue;
        }
        let t = (i - a) as f64 / (b - a) as f64;
        let z = za + (zb - za) * t;
        let d = (f64::from(heights[i]) - z).abs();
        if d > best.0 {
            best = (d, i);
        }
    }
    best
}

/// Selects points of `heights`. `fixed[i]` points (and both ends) are always selected,
/// `candidate[i]` points may be; the rest are never selected. Returns the selection mask.
pub fn simplify(heights: &[f32], fixed: &[bool], candidate: &[bool], threshold: f64) -> Vec<bool> {
    let n = heights.len();
    let mut selected = vec![false; n];
    if n == 0 {
        return selected;
    }
    selected[0] = true;
    selected[n - 1] = true;
    for i in 0..n {
        if fixed[i] {
            selected[i] = true;
        }
    }
    let mut points: Vec<usize> = (0..n).filter(|&i| selected[i]).collect();
    let mut segs: Vec<(f64, usize)> = points
        .windows(2)
        .map(|w| worst(heights, candidate, &selected, w[0], w[1]))
        .collect();
    loop {
        let Some((k, &(err, idx))) = segs
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.0.partial_cmp(&b.1.0).unwrap())
        else {
            break;
        };
        if err <= threshold {
            break;
        }
        selected[idx] = true;
        points.insert(k + 1, idx);
        let (a, b) = (points[k], points[k + 2]);
        segs[k] = worst(heights, candidate, &selected, a, idx);
        let right = worst(heights, candidate, &selected, idx, b);
        segs.insert(k + 1, right);
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_line_keeps_ends_and_fixed() {
        let h: Vec<f32> = (0..33).map(|i| i as f32 * 2.0).collect();
        let fixed: Vec<bool> = (0..33).map(|i| i == 16).collect();
        let sel = simplify(&h, &fixed, &vec![true; 33], 0.5);
        assert_eq!(sel.iter().filter(|&&s| s).count(), 3);
        assert!(sel[0] && sel[16] && sel[32]);
    }

    #[test]
    fn meets_threshold() {
        let h: Vec<f32> = (0..33).map(|i| ((i as f32) * 0.7).sin() * 10.0).collect();
        let sel = simplify(&h, &vec![false; 33], &vec![true; 33], 1.0);
        let pts: Vec<usize> = (0..33).filter(|&i| sel[i]).collect();
        for w in pts.windows(2) {
            for i in w[0]..=w[1] {
                let t = (i - w[0]) as f32 / (w[1] - w[0]) as f32;
                let z = h[w[0]] + (h[w[1]] - h[w[0]]) * t;
                assert!((h[i] - z).abs() <= 1.0 + 1e-5);
            }
        }
    }

    #[test]
    fn non_candidates_are_skipped() {
        let h: Vec<f32> = (0..9).map(|i| if i == 4 { 100.0 } else { 0.0 }).collect();
        let cand: Vec<bool> = (0..9).map(|i| i != 4).collect();
        let sel = simplify(&h, &vec![false; 9], &cand, 0.1);
        assert!(!sel[4]);
        assert_eq!(sel.iter().filter(|&&s| s).count(), 2);
    }
}
