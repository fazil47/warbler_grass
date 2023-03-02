use std::num::NonZeroU32;
use std::ops::Mul;

use super::extract::EntityStore;
use super::grass_pipeline::GrassPipeline;
use crate::grass_spawner::{GrassSpawner, GrassSpawnerFlags, HeightRepresentation};
use crate::render::cache::GrassCache;
use crate::GrassConfiguration;
use bevy::prelude::*;
use bevy::render::primitives::Aabb;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{
    BindGroupDescriptor, BindGroupEntry, BindingResource, BufferBinding, BufferInitDescriptor,
    BufferUsages, ShaderType, TextureViewId, TextureUsages, TextureFormat, TextureDimension, TextureDescriptor, Extent3d, TextureViewDescriptor, TextureViewDimension, TextureAspect, ImageCopyTexture, Origin3d, ImageDataLayout,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::texture::FallbackImage;
use bytemuck::{Pod, Zeroable};

pub(crate) fn prepare_instance_buffer(
    mut cache: ResMut<GrassCache>,
    render_device: Res<RenderDevice>,
    inserted_grass: Query<(&GrassSpawner, &EntityStore)>,
) {
    for (spawner, EntityStore(id)) in inserted_grass.iter() {
        if !spawner.flags.contains(GrassSpawnerFlags::Y_DEFINED) {
            panic!("Cannot spawn grass without the y-positions defined");
        }
        if !spawner.flags.contains(GrassSpawnerFlags::XZ_DEFINED) {
            panic!("Cannot spawn grass without the xz-positions defined");
        }
        let heights = match &spawner.heights {
            HeightRepresentation::Uniform(height) => vec![*height; spawner.positions_xz.len()],
            HeightRepresentation::PerBlade(heights) => heights.clone(),
        };
        let instance_slice: Vec<Vec3> = if spawner.flags.contains(GrassSpawnerFlags::HEIGHT_MAP) {
            spawner
                .positions_xz
                .iter()
                .zip(heights)
                .map(|(xz, height)| Vec3::new(xz.x, xz.y, height))
                .collect()
        } else {
            spawner
                .positions_xz
                .iter()
                // .zip(spawner.positions_y.iter())
                .zip(heights)
                .map(|(xz, height)| Vec3::new(xz.x, xz.y, height))
                .collect()
        };
        if let Some(chunk) = cache.get_mut(&id) {
            chunk.instances = Some(instance_slice);
            let inst = render_device.create_buffer_with_data(&BufferInitDescriptor {
                label: Some("Instance entity buffer"),
                contents: bytemuck::cast_slice(chunk.instances.as_ref().unwrap().as_slice()),
                usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            });
            chunk.instance_buffer = Some(inst);
            chunk.flags = spawner.flags;
        } else {
            warn!(
                "Tried to prepare a entity buffer for a grass chunk which wasn't registered before"
            );
        }
    }
}
pub(crate) fn prepare_explicit_y_buffer(
    mut cache: ResMut<GrassCache>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    pipeline: Res<GrassPipeline>,
    inserted_grass: Query<(&GrassSpawner, &EntityStore)>,
) {
    for (spawner, EntityStore(id)) in inserted_grass.iter() {
        if !spawner.flags.contains(GrassSpawnerFlags::Y_DEFINED) {
            panic!("Cannot spawn grass without the y-positions defined");
        }
        if spawner.flags.contains(GrassSpawnerFlags::HEIGHT_MAP) {
            continue;
        }
        if let Some(chunk) = cache.get_mut(&id) {
            let mut y_positions = spawner.positions_y.clone();

            let device = render_device.wgpu_device();
           
            // the dimensions of the texture are choosen to be nxn for the tiniest n which can contain the data
            let sqrt = (y_positions.len() as f32).sqrt() as u32 + 1;

            let fill_data = vec![0.;(sqrt * sqrt) as usize - y_positions.len()];
            y_positions.extend(fill_data);

            let size = Extent3d {
                width: sqrt,
                height: sqrt,
                depth_or_array_layers: 1,
            };
            // wgpu expects a byte array
            let data_slice = bytemuck::cast_slice(y_positions.as_slice());
            let texture = device.create_texture(&TextureDescriptor { 
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::R32Float,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                label: Some("y positions texture"),
                view_formats: &[],
            });
            // write data to texture
            render_queue.write_texture(ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            }, 
            data_slice, 
            ImageDataLayout {
                offset: 0,
                // Multiplication with 4 because 1 pixel = 1_f32 = 4_u8 
                bytes_per_row:  NonZeroU32::new(4 * size.width),
                rows_per_image: NonZeroU32::new(size.height),
            }, 
            size);
            
            let view = texture.create_view(&TextureViewDescriptor {
                label: "y positions".into(),
                format: Some(TextureFormat::R32Float),
                dimension: Some(TextureViewDimension::D2),
                aspect: TextureAspect::All,
                base_mip_level: 0,
                mip_level_count: NonZeroU32::new(1),
                base_array_layer: 0,
                array_layer_count: NonZeroU32::new(1),
            });
            let layout = pipeline.explicit_y_layout.clone();
            let bind_group_descriptor = BindGroupDescriptor {
                label: Some("grass explicit y positions bind group"),
                layout: &layout,
                entries: &[
                    BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::TextureView(&view),
                    },
                    
                ],
            };
            let bind_group = render_device.create_bind_group(&bind_group_descriptor);
            chunk.explicit_y_buffer = Some(bind_group);
        } else {
            warn!(
                "Tried to prepare a entity buffer for a grass chunk which wasn't registered before"
            );
        }
    }
}
pub(crate) fn prepare_height_map_buffer(
    mut cache: ResMut<GrassCache>,
    render_device: Res<RenderDevice>,
    pipeline: Res<GrassPipeline>,
    fallback_img: Res<FallbackImage>,
    images: Res<RenderAssets<Image>>,
    inserted_grass: Query<(&GrassSpawner, &EntityStore, &Aabb)>,
    mut local_height_map_buffer: Local<Vec<(EntityStore, Handle<Image>, Aabb)>>,
) {
    let mut to_remove = Vec::new();

    for (EntityStore(e), handle, aabb) in local_height_map_buffer.iter() {
        if let Some(tex) = images.get(&handle) {
            to_remove.push(*e);
            let height_map_texture = &tex.texture_view;
            let aabb_buffer = render_device.create_buffer_with_data(&BufferInitDescriptor {
                label: Some("aabb buffer"),
                contents: bytemuck::bytes_of(&aabb.half_extents.mul(2.).as_dvec3().as_vec3()),
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            });
            let layout = pipeline.height_map_layout.clone();
            let bind_group_descriptor = BindGroupDescriptor {
                label: Some("grass height map bind group"),
                layout: &layout,
                entries: &[
                    BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::TextureView(height_map_texture),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::Buffer(BufferBinding {
                            buffer: &aabb_buffer,
                            offset: 0,
                            size: None,
                        }),
                    },
                ],
            };

            let bind_group = render_device.create_bind_group(&bind_group_descriptor);
            if let Some(chunk) = cache.get_mut(&e) {
                chunk.height_map = Some(bind_group);
            } else {
                warn!("Tried to prepare a buffer for a grass chunk which wasn't registered before");
            }
        }
    }
    local_height_map_buffer.retain(|map| !to_remove.contains(&map.0 .0));
    for (spawner, entity_store, aabb) in inserted_grass.iter() {
        let id = entity_store.0;
        if spawner.flags.contains(GrassSpawnerFlags::HEIGHT_MAP) {
            let handle = &spawner.height_map.as_ref().unwrap().height_map;
            if images.get(&handle).is_none() {
                local_height_map_buffer.push((entity_store.clone(), handle.clone(), aabb.clone()));
            }
        }
        let (height_map_texture, aabb_buffer) =
            if !spawner.flags.contains(GrassSpawnerFlags::HEIGHT_MAP) {
                let height_map_texture = &fallback_img.texture_view;
                let aabb_buffer = render_device.create_buffer_with_data(&BufferInitDescriptor {
                    label: Some("aabb buffer"),
                    contents: &[0],
                    usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                });
                (height_map_texture, aabb_buffer)
            } else {
                let handle = spawner.height_map.as_ref().unwrap().height_map.clone();
                let height_map_texture = if let Some(tex) = images.get(&handle) {
                    &tex.texture_view
                } else {
                    &fallback_img.texture_view
                };

                let aabb_buffer = render_device.create_buffer_with_data(&BufferInitDescriptor {
                    label: Some("aabb buffer"),
                    contents: bytemuck::bytes_of(&aabb.half_extents.mul(2.).as_dvec3().as_vec3()),
                    usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                });
                (height_map_texture, aabb_buffer)
            };
        let layout = pipeline.height_map_layout.clone();

        let bind_group_descriptor = BindGroupDescriptor {
            label: Some("grass height map bind group"),
            layout: &layout,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(height_map_texture),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Buffer(BufferBinding {
                        buffer: &aabb_buffer,
                        offset: 0,
                        size: None,
                    }),
                },
            ],
        };

        let bind_group = render_device.create_bind_group(&bind_group_descriptor);
        if let Some(chunk) = cache.get_mut(&id) {
            chunk.height_map = Some(bind_group);
        } else {
            warn!("Tried to prepare a buffer for a grass chunk which wasn't registered before");
        }
    }
}
pub(crate) fn prepare_uniform_buffers(
    pipeline: Res<GrassPipeline>,
    mut cache: ResMut<GrassCache>,
    region_config: Res<GrassConfiguration>,
    fallback_img: Res<FallbackImage>,
    render_device: Res<RenderDevice>,
    images: Res<RenderAssets<Image>>,
    mut last_texture_id: Local<Option<TextureViewId>>,
) {
    let texture = &images
        .get(&region_config.wind_noise_texture)
        .unwrap_or(&fallback_img)
        .texture_view;
    if !region_config.is_changed() && Some(texture.id()) == *last_texture_id && !cache.is_changed()
    {
        return;
    }
    *last_texture_id = Some(texture.id());

    let shader_config = ShaderRegionConfiguration::from(region_config.as_ref());
    let config_buffer = render_device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some("region config buffer"),
        contents: bytemuck::bytes_of(&shader_config),
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
    });

    let layout = pipeline.region_layout.clone();
    let bind_group_descriptor = BindGroupDescriptor {
        label: Some("grass uniform bind group"),
        layout: &layout,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: &config_buffer,
                    offset: 0,
                    size: None,
                }),
            },
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::TextureView(texture),
            },
        ],
    };
    let bind_group = render_device.create_bind_group(&bind_group_descriptor);

    for instance_data in cache.values_mut() {
        instance_data.uniform_bindgroup = Some(bind_group.clone());
    }
}

#[derive(Debug, Clone, Copy, Pod, Zeroable, ShaderType)]
#[repr(C)]
struct ShaderRegionConfiguration {
    main_color: Vec4,
    bottom_color: Vec4,
    wind: Vec2,
    /// Wasm requires shader uniforms to be aligned to 16 bytes
    _wasm_padding: Vec2,
}

impl From<&GrassConfiguration> for ShaderRegionConfiguration {
    fn from(config: &GrassConfiguration) -> Self {
        Self {
            main_color: config.main_color.into(),
            bottom_color: config.bottom_color.into(),
            wind: config.wind,
            _wasm_padding: Vec2::ZERO,
        }
    }
}
