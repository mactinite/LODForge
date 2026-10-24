//! Checks that the GPU helpers of `bc7_ispc.wgsl` (fdiv, fsqrt, rsqrt_, cvt and fma) give the
//! same bits as IEEE f32 on the CPU, on random and edge-case inputs.

use wgpu::util::DeviceExt;

const SHADER: &str = r#"
@group(0) @binding(0) var<storage, read> a: array<f32>;
@group(0) @binding(1) var<storage, read> b: array<f32>;
@group(0) @binding(2) var<storage, read_write> o: array<u32>;

fn cvt(x: f32) -> i32 {
    if (((bitcast<u32>(x) >> 23u) & 0xffu) >= 158u) {
        return bitcast<i32>(0x80000000u);
    }
    return i32(x);
}
fn fdiv(a: f32, b: f32) -> f32 {
    if (b == 0.0) {
        return a / b;
    }
    let y = 1.0 / b;
    let q = a * y;
    let r = fma(-q, b, a);
    return fma(r, y, q);
}
fn fsqrt(x: f32) -> f32 {
    if (x <= 0.0) {
        return sqrt(x);
    }
    let s = sqrt(x);
    let r = fma(-s, s, x);
    return fma(r, 0.5 / s, s);
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) g: vec3<u32>) {
    let i = g.x;
    if (i >= arrayLength(&a)) {
        return;
    }
    let x = a[i];
    let y = b[i];
    o[i * 5u + 0u] = bitcast<u32>(fdiv(x, y));
    o[i * 5u + 1u] = bitcast<u32>(fsqrt(abs(x)));
    o[i * 5u + 2u] = bitcast<u32>(fdiv(1.0, fsqrt(abs(x))));
    o[i * 5u + 3u] = bitcast<u32>(cvt(x * y));
    o[i * 5u + 4u] = bitcast<u32>(fma(x, y, x - y));
}
"#;

fn main() {
    let gpu = lodforge_gpu::Gpu::new().unwrap();
    let n = 4_000_000usize;
    // xorshift
    let mut s = 0x2545F4914F6CDD1Du64;
    let mut rnd = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    let mut av = Vec::with_capacity(n);
    let mut bv = Vec::with_capacity(n);
    for i in 0..n {
        let r = rnd();
        let (x, y) = match i % 4 {
            // Values like the encoder's: 0..255 pixels, sums and products of them.
            0 => ((r % 256) as f32, ((r >> 16) % 256) as f32 + 1.0),
            1 => (((r % 4_000_000) as f32) * 0.37, ((r >> 24) % 70000) as f32 * 0.013 + 0.5),
            2 => (f32::from_bits((r as u32 & 0x3fff_ffff) | 0x3000_0000), f32::from_bits(((r >> 32) as u32 & 0x3fff_ffff) | 0x3000_0000)),
            _ => ((r % 255) as f32 / 255.0 * 63.0, 255.0),
        };
        av.push(x);
        bv.push(y);
    }
    let d = &gpu.device;
    let module = d.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
    let pipe = d.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: None, layout: None, module: &module, entry_point: Some("main"), compilation_options: Default::default(), cache: None });
    let ba = d.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&av), usage: wgpu::BufferUsages::STORAGE });
    let bb = d.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&bv), usage: wgpu::BufferUsages::STORAGE });
    let size = (n * 5 * 4) as u64;
    let out = d.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
    let rb = d.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    let bind = d.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipe.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: ba.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: bb.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: out.as_entire_binding() },
        ],
    });
    let mut enc = d.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipe);
        pass.set_bind_group(0, &bind, &[]);
        let groups = (n as u32).div_ceil(64);
        pass.dispatch_workgroups(groups.min(65535), groups.div_ceil(65535), 1);
    }
    enc.copy_buffer_to_buffer(&out, 0, &rb, 0, size);
    gpu.queue.submit([enc.finish()]);
    rb.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    d.poll(wgpu::PollType::Wait).unwrap();
    let got: Vec<u32> = bytemuck::cast_slice(&rb.slice(..).get_mapped_range()).to_vec();
    let names = ["div", "sqrt", "rsqrt", "cvt(x*y)", "fma"];
    let mut bad = [0usize; 5];
    let mut shown = [0usize; 5];
    for i in 0..n {
        let (x, y) = (av[i], bv[i]);
        let want = [(x / y).to_bits(), x.abs().sqrt().to_bits(), (1.0 / x.abs().sqrt()).to_bits(), {
            let p = x * y;
            (if p.is_finite() && p.abs() < 2147483648.0 { p as i32 } else { i32::MIN }) as u32
        }, x.mul_add(y, x - y).to_bits()];
        for k in 0..5 {
            if got[i * 5 + k] != want[k] && !(k != 3 && f32::from_bits(want[k]).is_nan() && f32::from_bits(got[i * 5 + k]).is_nan()) {
                bad[k] += 1;
                if shown[k] < 3 {
                    shown[k] += 1;
                    println!("{} x={x:e} y={y:e}: cpu {:e} gpu {:e}", names[k], f32::from_bits(want[k]), f32::from_bits(got[i * 5 + k]));
                }
            }
        }
    }
    // Note: the x*y inside cvt is itself rounded on the GPU; only gross differences matter there.
    for k in 0..5 {
        println!("{:>9}: {} of {n} differ", names[k], bad[k]);
    }
}
