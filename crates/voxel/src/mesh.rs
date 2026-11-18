//! Meshing lives in `nascio-chunk` (the client meshes without the generation crates); this
//! module re-exports it and tests it on generated terrain.

pub use nascio_chunk::mesh::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate::ColumnSet;
    use crate::terrain::ChunkTint;
    use crate::terrain::tests::test_terrain;

    #[test]
    fn generated_terrain_meshes() {
        let t = test_terrain();
        let col = ColumnSet::new(&t, 7, 7);
        let total: usize = col
            .chunk_range(1)
            .map(|cy| {
                mesh_chunk(&col.padded_chunk(&t, cy), t.blocks(), &ChunkTint::NONE)
                    .opaque
                    .quad_count()
            })
            .sum();
        assert!(total > 0);
    }

    #[test]
    fn same_mesh_as_the_reference_on_generated_terrain() {
        // Columns of the test world, land and sea, with their grass and water tint.
        let t = test_terrain();
        let mut meshed = 0;
        for (cx, cz) in [(7, 7), (0, 0), (30, 12), (60, 60), (100, 3)] {
            let col = ColumnSet::new(&t, cx, cz);
            let tint = t.chunk_tint(cx, cz);
            for cy in col.chunk_range(1) {
                let p = col.padded_chunk(&t, cy);
                let m = mesh_chunk(&p, t.blocks(), &tint);
                assert_eq!(
                    m,
                    mesh_reference(&p, t.blocks(), &tint),
                    "chunk {cx} {cy} {cz}"
                );
                meshed += usize::from(!m.is_empty());
            }
        }
        assert!(meshed > 0);
    }
}
