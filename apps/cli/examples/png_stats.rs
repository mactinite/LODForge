//! Scratch image measurements for the detail-bake comparisons. Background = the top-left pixel.
//! `png_stats <file.png> [slices]`: mean linear luminance of the non-background pixels in each of
//! N vertical slices. `png_stats <file.png> bbox <x0> <x1>`: bounding box (`x y w h`) of the
//! non-background pixels between columns x0 and x1.
fn lin(v: u8) -> f64 { let c = v as f64 / 255.0; if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) } }
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&a[0]).unwrap()));
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).unwrap();
    let (w, h, c) = (info.width as usize, info.height as usize, info.color_type.samples());
    let bg = buf[0..3].to_vec();
    let fg = |x: usize, y: usize| { let p = &buf[(y * w + x) * c..][..3]; p.iter().zip(&bg).any(|(a, b)| (*a as i32 - *b as i32).abs() > 12) };
    if a.get(1).map(String::as_str) == Some("bbox") {
        let x0: usize = a[2].parse::<i64>().unwrap().max(0) as usize;
        let x1: usize = (a[3].parse::<i64>().unwrap().max(0) as usize).min(w);
        let (mut bx0, mut by0, mut bx1, mut by1) = (w, h, 0, 0);
        for y in 0..h { for x in x0..x1 { if fg(x, y) { bx0 = bx0.min(x); by0 = by0.min(y); bx1 = bx1.max(x); by1 = by1.max(y); } } }
        println!("{} {} {} {}", bx0, by0, bx1 + 1 - bx0, by1 + 1 - by0);
        return;
    }
    let n: usize = a.get(1).map_or(3, |s| s.parse().unwrap());
    let mut sum = vec![0f64; n];
    let mut cnt = vec![0usize; n];
    for y in 0..h { for x in 0..w {
        if !fg(x, y) { continue }
        let p = &buf[(y * w + x) * c..][..3];
        let s = x * n / w;
        sum[s] += 0.2126 * lin(p[0]) + 0.7152 * lin(p[1]) + 0.0722 * lin(p[2]);
        cnt[s] += 1;
    }}
    let v: Vec<String> = (0..n).map(|i| format!("{:.4} ({} px)", sum[i] / cnt[i].max(1) as f64, cnt[i])).collect();
    println!("{}", v.join("  |  "));
}
