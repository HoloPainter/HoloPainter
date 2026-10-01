use crate::{
    core::{damage::DamageMap, geometry::RectU32, surface::PaintSurfaceId},
    renderer::{
        features::composite::cache::CompositeCache,
        mutation::{MutationLog, SurfaceDamage},
    },
};

/// Applies composite-cache invalidation derived from renderer mutation logs.
///
/// Surface and document mutation code reports damage through `MutationLog`;
/// `CompositeFeature` is the boundary that translates those mutations into
/// `CompositeCache` dirty state.
pub(crate) fn apply_mutation_invalidation(cache: &mut CompositeCache, mutations: &MutationLog) {
    if mutations.scene.has_any() {
        cache.mark_all_dirty();
        return;
    }

    for material_index in mutations.composites.tree_material_indices() {
        cache.mark_dirty(material_index);
    }
    for material_index in mutations.composites.value_material_indices() {
        cache.mark_layer_value_dirty(material_index);
    }
    for (material_index, rect) in mutations.composites.value_rects() {
        cache.mark_layer_value_dirty_rect(material_index, rect);
    }

    // SurfaceDamageSet preserves command order across MutationLog::merge. Apply active-preview
    // transitions in that order so each material's final mutation wins within the batch.
    for mutation in mutations.surfaces.iter() {
        match mutation {
            SurfaceDamage::Preview { surface, damage } => {
                cache.merge_active_hint_for_surfaces([*surface]);
                mark_preview_surface_dirty(cache, *surface, damage.as_ref());
            }
            SurfaceDamage::Transient { surface, damage } => {
                mark_damage_dirty(cache, *surface, Some(damage));
                cache.clear_active_preview_state(surface.material_index().as_usize());
            }
            SurfaceDamage::Cancelled { surface } => {
                cache.mark_surface_dirty(*surface);
                cache.clear_active_preview_state(surface.material_index().as_usize());
            }
            SurfaceDamage::Full { surface } => {
                cache.mark_surface_dirty(*surface);
                for material_index in mutation.material_indices() {
                    cache.clear_active_preview_state(material_index);
                }
            }
            SurfaceDamage::Rects { surface, damage } => {
                mark_damage_dirty(cache, *surface, Some(damage));
                for material_index in mutation.material_indices() {
                    cache.clear_active_preview_state(material_index);
                }
            }
        }
    }
}

fn mark_preview_surface_dirty(
    cache: &mut CompositeCache,
    surface: PaintSurfaceId,
    damage: Option<&DamageMap>,
) {
    let Some(damage) = damage else {
        cache.mark_preview_dirty(surface);
        return;
    };
    if damage.is_empty() {
        return;
    }
    for pixel in &damage.pixels {
        cache.mark_preview_dirty_rect(
            surface,
            pixel.surface.material_index().as_usize(),
            pixel.rect,
        );
    }
}

fn mark_damage_dirty(
    cache: &mut CompositeCache,
    fallback_surface: PaintSurfaceId,
    damage: Option<&DamageMap>,
) {
    let Some(damage) = damage else {
        cache.mark_surface_dirty(fallback_surface);
        return;
    };
    if damage.is_empty() {
        cache.mark_surface_dirty(fallback_surface);
        return;
    }
    for pixel in &damage.pixels {
        mark_rect_dirty(cache, pixel.surface, pixel.rect);
    }
}

fn mark_rect_dirty(cache: &mut CompositeCache, surface: PaintSurfaceId, rect: RectU32) {
    if rect.origin == [0, 0]
        && cache
            .texture_size(surface.material_index().as_usize())
            .is_some_and(|texture_size| rect.size == texture_size)
    {
        cache.mark_surface_dirty(surface);
    } else {
        cache.mark_surface_dirty_rect(surface, rect);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::surface::LayerId;
    use slotmap::SlotMap;

    fn test_surfaces(material_index: usize, count: usize) -> Vec<PaintSurfaceId> {
        let mut layers = SlotMap::<LayerId, ()>::with_key();
        (0..count)
            .map(|_| PaintSurfaceId::raster(material_index.into(), layers.insert(())))
            .collect()
    }

    #[test]
    fn preview_invalidation_does_not_shrink_active_hint() {
        let mut cache = CompositeCache::new();
        let surfaces = test_surfaces(0, 2);
        cache.set_active_hint_for_surfaces(surfaces.iter().copied());
        let mut mutations = MutationLog::default();
        mutations.surfaces.preview(surfaces[1]);

        apply_mutation_invalidation(&mut cache, &mutations);

        let hint = cache.active_hint(0).expect("active hint should remain");
        assert_eq!(hint.surfaces().len(), 2);
        assert!(hint.surfaces().contains(&surfaces[0]));
        assert!(hint.surfaces().contains(&surfaces[1]));
    }

    #[test]
    fn transient_damage_does_not_seed_stroke_preview_hint() {
        let mut cache = CompositeCache::new();
        cache.ensure_test_material_state(0, [512, 512]);
        let surface = test_surfaces(0, 1)[0];
        let rect = RectU32 {
            origin: [32, 48],
            size: [64, 32],
        };
        let mut damage = DamageMap::default();
        damage.add_rect(surface, rect);
        let mut mutations = MutationLog::default();
        mutations.surfaces.transient_damage(surface, damage);

        apply_mutation_invalidation(&mut cache, &mutations);

        assert!(cache.active_hint(0).is_none());
        assert!(!cache.dirty_state(0).rects.is_empty());
    }

    #[test]
    fn layer_value_rect_keeps_composite_invalidation_partial() {
        let mut cache = CompositeCache::new();
        cache.ensure_test_material_state(0, [4096, 4096]);
        let rect = RectU32 {
            origin: [128, 256],
            size: [512, 384],
        };
        let mut mutations = MutationLog::default();
        mutations.composites.layer_value_rect(0, rect);

        apply_mutation_invalidation(&mut cache, &mutations);

        let dirty = cache.dirty_state(0);
        assert!(!dirty.full);
        assert_eq!(dirty.rects, vec![rect]);
    }

    #[test]
    fn committed_rect_damage_clears_active_preview_and_preserves_rect_dirty_state() {
        let mut cache = CompositeCache::new();
        cache.ensure_test_material_state(0, [512, 512]);
        let surfaces = test_surfaces(0, 1);
        let mut preview = MutationLog::default();
        preview.surfaces.preview(surfaces[0]);
        apply_mutation_invalidation(&mut cache, &preview);
        assert!(cache.active_hint(0).is_some());
        cache.ensure_test_material_state(0, [512, 512]);

        let rect = RectU32 {
            origin: [32, 48],
            size: [64, 32],
        };
        let mut damage = DamageMap::default();
        damage.add_rect(surfaces[0], rect);
        let mut committed = MutationLog::default();
        committed.surfaces.rects(surfaces[0], damage);

        apply_mutation_invalidation(&mut cache, &committed);

        assert!(cache.active_hint(0).is_none());
        let dirty = cache.dirty_state(0);
        assert!(!dirty.full);
        assert_eq!(dirty.rects, vec![rect]);
    }

    #[test]
    fn committed_full_damage_clears_active_preview_and_marks_full_dirty() {
        let mut cache = CompositeCache::new();
        cache.ensure_test_material_state(0, [512, 512]);
        let surfaces = test_surfaces(0, 1);
        let mut preview = MutationLog::default();
        preview.surfaces.preview(surfaces[0]);
        apply_mutation_invalidation(&mut cache, &preview);
        assert!(cache.active_hint(0).is_some());

        let mut committed = MutationLog::default();
        committed.surfaces.full(surfaces[0]);

        apply_mutation_invalidation(&mut cache, &committed);

        assert!(cache.active_hint(0).is_none());
        assert!(cache.dirty_state(0).full);
    }

    #[test]
    fn committed_rect_damage_clears_active_preview_for_damage_map_material() {
        let mut cache = CompositeCache::new();
        cache.ensure_test_material_state(0, [512, 512]);
        cache.ensure_test_material_state(1, [512, 512]);
        let fallback_surface = test_surfaces(0, 1)[0];
        let damaged_surface = test_surfaces(1, 1)[0];
        let mut preview = MutationLog::default();
        preview.surfaces.preview(damaged_surface);
        apply_mutation_invalidation(&mut cache, &preview);
        assert!(cache.active_hint(1).is_some());
        cache.ensure_test_material_state(1, [512, 512]);

        let rect = RectU32 {
            origin: [8, 16],
            size: [32, 32],
        };
        let mut damage = DamageMap::default();
        damage.add_rect(damaged_surface, rect);
        let mut committed = MutationLog::default();
        committed.surfaces.rects(fallback_surface, damage);

        apply_mutation_invalidation(&mut cache, &committed);

        assert!(cache.active_hint(1).is_none());
        assert_eq!(cache.dirty_state(1).rects, vec![rect]);
    }

    #[test]
    fn preview_then_commit_in_same_batch_clears_active_preview() {
        let mut cache = CompositeCache::new();
        cache.ensure_test_material_state(0, [512, 512]);
        let surface = test_surfaces(0, 1)[0];

        let mut prior_preview = MutationLog::default();
        prior_preview.surfaces.preview(surface);
        apply_mutation_invalidation(&mut cache, &prior_preview);
        assert!(cache.active_hint(0).is_some());
        cache.ensure_test_material_state(0, [512, 512]);

        let rect = RectU32 {
            origin: [32, 48],
            size: [64, 32],
        };
        let mut damage = DamageMap::default();
        damage.add_rect(surface, rect);
        let mut preview = MutationLog::default();
        preview.surfaces.preview_damage(surface, damage.clone());
        let mut commit = MutationLog::default();
        commit.surfaces.rects(surface, damage);
        let mut same_batch = MutationLog::default();
        same_batch.merge(preview);
        same_batch.merge(commit);

        apply_mutation_invalidation(&mut cache, &same_batch);

        assert!(
            cache.active_hint(0).is_none(),
            "the final Commit mutation must win over an earlier Preview mutation in the same batch"
        );
        let dirty = cache.dirty_state(0);
        assert!(!dirty.full);
        assert_eq!(dirty.rects, vec![rect]);
    }

    #[test]
    fn commit_then_preview_in_same_batch_keeps_active_preview() {
        let mut cache = CompositeCache::new();
        cache.ensure_test_material_state(0, [512, 512]);
        let surface = test_surfaces(0, 1)[0];

        let rect = RectU32 {
            origin: [32, 48],
            size: [64, 32],
        };
        let mut damage = DamageMap::default();
        damage.add_rect(surface, rect);
        let mut commit = MutationLog::default();
        commit.surfaces.rects(surface, damage.clone());
        let mut preview = MutationLog::default();
        preview.surfaces.preview_damage(surface, damage);
        let mut same_batch = MutationLog::default();
        same_batch.merge(commit);
        same_batch.merge(preview);

        apply_mutation_invalidation(&mut cache, &same_batch);

        assert!(
            cache.active_hint(0).is_some(),
            "the final Preview mutation must win over an earlier Commit mutation in the same batch"
        );
    }

    #[test]
    fn preview_preview_commit_in_same_batch_clears_active_preview() {
        let mut cache = CompositeCache::new();
        cache.ensure_test_material_state(0, [512, 512]);
        let surface = test_surfaces(0, 1)[0];

        let rect = RectU32 {
            origin: [32, 48],
            size: [64, 32],
        };
        let mut damage = DamageMap::default();
        damage.add_rect(surface, rect);
        let mut first_preview = MutationLog::default();
        first_preview
            .surfaces
            .preview_damage(surface, damage.clone());
        let mut second_preview = MutationLog::default();
        second_preview
            .surfaces
            .preview_damage(surface, damage.clone());
        let mut commit = MutationLog::default();
        commit.surfaces.rects(surface, damage);
        let mut same_batch = MutationLog::default();
        same_batch.merge(first_preview);
        same_batch.merge(second_preview);
        same_batch.merge(commit);

        apply_mutation_invalidation(&mut cache, &same_batch);

        assert!(
            cache.active_hint(0).is_none(),
            "the final Commit mutation must win over all earlier Preview mutations"
        );
    }

    #[test]
    fn preview_commit_preview_in_same_batch_keeps_active_preview() {
        let mut cache = CompositeCache::new();
        cache.ensure_test_material_state(0, [512, 512]);
        let surface = test_surfaces(0, 1)[0];

        let rect = RectU32 {
            origin: [32, 48],
            size: [64, 32],
        };
        let mut damage = DamageMap::default();
        damage.add_rect(surface, rect);
        let mut first_preview = MutationLog::default();
        first_preview
            .surfaces
            .preview_damage(surface, damage.clone());
        let mut commit = MutationLog::default();
        commit.surfaces.rects(surface, damage.clone());
        let mut last_preview = MutationLog::default();
        last_preview.surfaces.preview_damage(surface, damage);
        let mut same_batch = MutationLog::default();
        same_batch.merge(first_preview);
        same_batch.merge(commit);
        same_batch.merge(last_preview);

        apply_mutation_invalidation(&mut cache, &same_batch);

        assert!(
            cache.active_hint(0).is_some(),
            "the final Preview mutation must reactivate preview after an earlier Commit"
        );
    }
}
