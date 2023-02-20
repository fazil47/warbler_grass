use bevy::core_pipeline::core_3d::Opaque3d;
use bevy::pbr::{MeshPipelineKey, MeshUniform};
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_phase::{DrawFunctions, RenderPhase};
use bevy::render::render_resource::{
    BindGroupDescriptor, BindGroupEntry, BindingResource, BufferBinding,
    BufferInitDescriptor, BufferUsages, PipelineCache, SpecializedMeshPipelines,
};
use bevy::render::renderer::RenderDevice;
use bevy::render::texture::FallbackImage;
use bevy::render::view::ExtractedView;
use bevy::{
    pbr::{SetMeshBindGroup, SetMeshViewBindGroup},
    render::render_phase::SetItemPipeline,
};

use crate::{Grass, RegionConfiguration};

use self::cache::GrassCache;
use self::grass_pipeline::GrassPipeline;
mod draw_mesh;
pub(crate) mod grass_pipeline;
pub mod extract;
pub mod cache;

pub(crate) type GrassDrawCall = (
    // caches pipeline instead of reinit every call
    SetItemPipeline,
    SetMeshViewBindGroup<0>,
    SetMeshBindGroup<1>,
    draw_mesh::DrawMeshInstanced,
);

pub(crate) fn prepare_instance_buffers(
    pipeline: Res<GrassPipeline>,
    mut cache: ResMut<GrassCache>,
    region_config: Res<RegionConfiguration>,
    fallback_img: Res<FallbackImage>,
    render_device: Res<RenderDevice>,
    images: Res<RenderAssets<Image>>,
) {
    for instance_data in cache.values_mut() {
        let entity_buffer = render_device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("instance entity data buffer"),
            contents: bytemuck::cast_slice(instance_data.grass.instances.as_slice()),
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
        });
        let region_color_buffer = render_device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("region color buffer"),
            contents: bytemuck::cast_slice(&region_config.color.as_rgba_f32()),
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        });
        let region_wind_buffer = render_device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("region wind buffer"),
            contents: bytemuck::cast_slice(&region_config.wind.to_array()),
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        });

        let layout = pipeline.region_layout.clone();
        let bind_group_des = BindGroupDescriptor {
            label: Some("grass uniform bind group"),
            layout: &layout,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::Buffer(BufferBinding {
                        buffer: &region_color_buffer,
                        offset: 0,
                        size: None,
                    }),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Buffer(BufferBinding {
                        buffer: &region_wind_buffer,
                        offset: 0,
                        size: None,
                    }),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::TextureView({
                        if let Some(img) = images.get(&region_config.wind_noise_texture) {
                            &img.texture_view
                        } else {
                            &fallback_img.texture_view
                        }
                    }),
                },
            ],
        };

        let bind_group = render_device.create_bind_group(&bind_group_des);
        instance_data.grass_buffer = Some(entity_buffer);
        instance_data.uniform_bindgroup = Some(bind_group);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn queue_grass_buffers(
    transparent_3d_draw_functions: Res<DrawFunctions<Opaque3d>>,
    grass_pipeline: Res<GrassPipeline>,
    msaa: Res<Msaa>,
    mut pipelines: ResMut<SpecializedMeshPipelines<GrassPipeline>>,
    mut pipeline_cache: ResMut<PipelineCache>,
    meshes: Res<RenderAssets<Mesh>>,
    material_meshes: Query<(Entity, &MeshUniform, &Handle<Mesh>), With<Grass>>,
    mut views: Query<(&ExtractedView, &mut RenderPhase<Opaque3d>)>,
) {
    let draw_custom = transparent_3d_draw_functions
        .read()
        .get_id::<GrassDrawCall>()
        .unwrap();

    let msaa_key = MeshPipelineKey::from_msaa_samples(msaa.samples);

    for (view, mut transparent_phase) in &mut views {
        let view_key = msaa_key | MeshPipelineKey::from_hdr(view.hdr);
        let rangefinder = view.rangefinder3d();
        for (entity, mesh_uniform, mesh_handle) in &material_meshes {
            if let Some(mesh) = meshes.get(mesh_handle) {
                let key =
                    view_key | MeshPipelineKey::from_primitive_topology(mesh.primitive_topology);

                let pipeline = pipelines
                    .specialize(&mut pipeline_cache, &grass_pipeline, key, &mesh.layout)
                    .unwrap();
                transparent_phase.add(Opaque3d {
                    entity,
                    pipeline,
                    draw_function: draw_custom,
                    distance: rangefinder.distance(&mesh_uniform.transform),
                });
            }
        }
    }
}
