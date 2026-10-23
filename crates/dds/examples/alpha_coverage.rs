//! Alpha coverage per mip level of a DDS (or of a pixel rectangle of mip 0, scaled down with the
//! level): the share of texels at or above each test threshold among texels with any alpha, and
//! the mean alpha of those. Shows how billboard foliage thins out in the smaller mips that distant
//! LOD samples. `cargo run -p lodforge-dds --example alpha_coverage -- <file.dds> [x,y,w,h]`

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).unwrap();
    let info = lodforge_dds::parse(&bytes).unwrap();
    let rect: Option<[u32; 4]> = a.get(2).map(|s| {
        let v: Vec<u32> = s.split(',').map(|x| x.parse().unwrap()).collect();
        [v[0], v[1], v[2], v[3]]
    });
    println!("{} {}x{} {:?} mips {}", a[1], info.width, info.height, info.format, info.mip_count);
    for level in 0..info.mip_count {
        let img = lodforge_dds::decode(&bytes, &info, level).unwrap();
        let [x0, y0, w, h] = rect.map(|r| r.map(|v| v >> level)).unwrap_or([0, 0, img.width, img.height]);
        let (mut any, mut sum, mut over, mut rgb) = (0u64, 0u64, [0u64; 3], [0u64; 3]);
        for y in y0..(y0 + h.max(1)).min(img.height) {
            for x in x0..(x0 + w.max(1)).min(img.width) {
                let al = img.rgba[((y * img.width + x) * 4 + 3) as usize];
                if al > 0 {
                    any += 1;
                    sum += u64::from(al);
                    if al >= 128 {
                        let i = ((y * img.width + x) * 4) as usize;
                        for c in 0..3 {
                            rgb[c] += u64::from(img.rgba[i + c]);
                        }
                    }
                    for (k, t) in [64u8, 128, 192].iter().enumerate() {
                        if al >= *t {
                            over[k] += 1;
                        }
                    }
                }
            }
        }
        let pct = |n: u64| if any == 0 { 0.0 } else { 100.0 * n as f64 / any as f64 };
        println!("  mip {level:2} {:5}x{:<5} texels with alpha {any:9}  >=64 {:5.1}%  >=128 {:5.1}%  >=192 {:5.1}%  mean {:5.1}  rgb>=128 {:?}", img.width, img.height, pct(over[0]), pct(over[1]), pct(over[2]), if any == 0 { 0.0 } else { sum as f64 / any as f64 }, rgb.map(|v| if over[1] == 0 { 0 } else { v / over[1] }));
    }
}
