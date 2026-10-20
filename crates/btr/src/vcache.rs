//! Triangle ordering for the GPU vertex cache: Tom Forsyth's "Linear-Speed Vertex Cache
//! Optimisation" (2006), written from the published description. Vertices score by their position
//! in a 32-entry LRU cache (the three vertices of the last triangle score 0.75, older entries
//! decay as (1 - age)^1.5) plus a valence boost (2 / sqrt(remaining triangles)); the triangle
//! with the highest summed score is emitted next, and only the triangles of vertices whose score
//! changed are re-evaluated. Vertices with no remaining triangle score -1 so finished vertices
//! leave the cache quickly.

const CACHE: usize = 32;
const LAST_TRIANGLE_SCORE: f32 = 0.75;
const DECAY_POWER: f32 = 1.5;
const VALENCE_SCALE: f32 = 2.0;
const VALENCE_POWER: f32 = 0.5;
const NONE: u32 = u32::MAX;

fn vertex_score(cache_pos: Option<usize>, active: u32) -> f32 {
    if active == 0 {
        return -1.0;
    }
    let position = match cache_pos {
        None => 0.0,
        Some(p) if p < 3 => LAST_TRIANGLE_SCORE,
        Some(p) => (1.0 - (p - 3) as f32 / (CACHE - 3) as f32).powf(DECAY_POWER),
    };
    position + VALENCE_SCALE * (active as f32).powf(-VALENCE_POWER)
}

/// The triangles reordered for cache locality; vertices are not renumbered.
pub fn forsyth_order<I: Copy + Into<u32>>(
    vertex_count: usize,
    triangles: &[[I; 3]],
) -> Vec<[I; 3]> {
    let n = triangles.len();
    if n == 0 {
        return Vec::new();
    }
    // Adjacency: triangles per vertex.
    let mut offsets = vec![0u32; vertex_count + 1];
    for t in triangles {
        for &v in t {
            offsets[v.into() as usize + 1] += 1;
        }
    }
    for i in 0..vertex_count {
        offsets[i + 1] += offsets[i];
    }
    let mut fill = offsets.clone();
    let mut adjacent = vec![0u32; n * 3];
    for (ti, t) in triangles.iter().enumerate() {
        for &v in t {
            let v = v.into() as usize;
            adjacent[fill[v] as usize] = ti as u32;
            fill[v] += 1;
        }
    }
    let mut active: Vec<u32> = (0..vertex_count)
        .map(|v| offsets[v + 1] - offsets[v])
        .collect();
    let mut cache_pos = vec![NONE; vertex_count];
    let mut score: Vec<f32> = active.iter().map(|&a| vertex_score(None, a)).collect();
    let mut tri_score: Vec<f32> = triangles
        .iter()
        .map(|t| t.iter().map(|&v| score[v.into() as usize]).sum())
        .collect();
    let mut emitted = vec![false; n];
    let mut cache: Vec<u32> = Vec::with_capacity(CACHE + 3);
    let mut out = Vec::with_capacity(n);
    let mut best = (0..n)
        .max_by(|&a, &b| tri_score[a].partial_cmp(&tri_score[b]).unwrap())
        .map(|t| t as u32)
        .unwrap_or(NONE);
    let mut touched: Vec<u32> = Vec::new();
    while out.len() < n {
        if best == NONE {
            // No candidate among the cache's triangles: take the best remaining one.
            best = (0..n)
                .filter(|&t| !emitted[t])
                .max_by(|&a, &b| tri_score[a].partial_cmp(&tri_score[b]).unwrap())
                .map(|t| t as u32)
                .unwrap_or(NONE);
            if best == NONE {
                break;
            }
        }
        let t = best as usize;
        emitted[t] = true;
        out.push(triangles[t]);
        touched.clear();
        for &v in &triangles[t] {
            let v: u32 = v.into();
            active[v as usize] -= 1;
            if let Some(i) = cache.iter().position(|&c| c == v) {
                cache.remove(i);
            }
            cache.insert(0, v);
        }
        // Entries pushed beyond the cache size leave it.
        while cache.len() > CACHE {
            let v = cache.pop().unwrap();
            cache_pos[v as usize] = NONE;
            score[v as usize] = vertex_score(None, active[v as usize]);
            touched.push(v);
        }
        for (pos, &v) in cache.iter().enumerate() {
            cache_pos[v as usize] = pos as u32;
            score[v as usize] = vertex_score(Some(pos), active[v as usize]);
            touched.push(v);
        }
        best = NONE;
        let mut best_score = f32::NEG_INFINITY;
        for &v in &touched {
            for &ti in &adjacent[offsets[v as usize] as usize..offsets[v as usize + 1] as usize] {
                if emitted[ti as usize] {
                    continue;
                }
                let s: f32 = triangles[ti as usize]
                    .iter()
                    .map(|&w| score[w.into() as usize])
                    .sum();
                tri_score[ti as usize] = s;
                if s > best_score {
                    best_score = s;
                    best = ti;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cache misses of a triangle list for an LRU cache of `size`.
    fn misses(tris: &[[u16; 3]], size: usize) -> usize {
        let mut cache: Vec<u16> = Vec::new();
        let mut m = 0;
        for t in tris {
            for &v in t {
                if let Some(i) = cache.iter().position(|&c| c == v) {
                    cache.remove(i);
                } else {
                    m += 1;
                }
                cache.insert(0, v);
                cache.truncate(size);
            }
        }
        m
    }

    #[test]
    fn keeps_every_triangle_and_improves_locality() {
        // A 40 x 40 grid triangulated row by row, then shuffled deterministically.
        let w = 41u16;
        let mut tris = Vec::new();
        for y in 0..40u16 {
            for x in 0..40u16 {
                let a = y * w + x;
                tris.push([a, a + 1, a + w]);
                tris.push([a + 1, a + w + 1, a + w]);
            }
        }
        let mut shuffled = tris.clone();
        let mut seed = 12345u64;
        for i in (1..shuffled.len()).rev() {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let j = (seed >> 33) as usize % (i + 1);
            shuffled.swap(i, j);
        }
        let ordered = forsyth_order(41 * 41, &shuffled);
        assert_eq!(ordered.len(), tris.len());
        let mut a = ordered.clone();
        let mut b = tris.clone();
        a.sort();
        b.sort();
        assert_eq!(a, b);
        let before = misses(&shuffled, 32);
        let after = misses(&ordered, 32);
        assert!(after * 2 < before, "misses before {before}, after {after}");
        // Close to the ideal of one miss per vertex.
        assert!(after < 41 * 41 * 3 / 2, "misses after {after}");
    }

    #[test]
    fn empty_and_single() {
        assert!(forsyth_order::<u16>(0, &[]).is_empty());
        assert_eq!(forsyth_order(3, &[[0u16, 1, 2]]), vec![[0u16, 1, 2]]);
    }
}
