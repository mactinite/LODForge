//! Pages of a mesh pack (class, size) and, per page, how many meshes use it, with sample models.
//! `pack_pages <StaticMeshes.bin> [model substring]`
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let p = lodforge_instances::meshpack::MeshPack::from_bytes(&std::fs::read(&a[1]).unwrap()).unwrap();
    let q = a.get(2).map(|s| s.to_ascii_lowercase());
    for (i, pg) in p.pages.iter().enumerate() {
        let ms: Vec<&str> = p.meshes.iter().filter(|m| p.clusters[m.first_cluster as usize..(m.first_cluster + m.cluster_count) as usize].iter().any(|c| c.page as usize == i)).map(|m| m.model.as_str()).collect();
        let hits: Vec<&&str> = ms.iter().filter(|m| q.as_ref().is_some_and(|q| m.to_ascii_lowercase().contains(q.as_str()))).take(4).collect();
        println!("page {i:2} class {} {}x{} meshes {:5} {:?}", pg.class, pg.width, pg.height, ms.len(), hits);
    }
}
