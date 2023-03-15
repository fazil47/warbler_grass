use std::mem;
use std::num::{NonZeroU32, NonZeroU64};
use std::ops::Mul;

use super::grass_pipeline::GrassPipeline;
use crate::bundle::{Grass, WarblerHeight};
use crate::height_map::HeightMap;
use crate::render::cache::GrassCache;
use crate::{GrassConfiguration, GrassNoiseTexture};
use bevy::math::Vec3Swizzles;
use bevy::prelude::*;
use bevy::render::primitives::Aabb;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{
    BindGroupDescriptor, BindGroupEntry, BindingResource, BufferBinding, BufferInitDescriptor,
    BufferUsages, Extent3d, ImageCopyTexture, ImageDataLayout, Origin3d, TextureAspect,
    TextureDescriptor, TextureDimension, TextureFormat, TextureUsages, TextureView,
    TextureViewDescriptor, TextureViewDimension, TextureViewId,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::texture::FallbackImage;
use bytemuck::{Pod, Zeroable};

pub(crate) fn prepare_explicit_positions_buffer(
    mut cache: ResMut<GrassCache>,
    pipeline: Res<GrassPipeline>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    mut inserted_grass: Query<(Entity, &mut Grass)>,
) {
    for (id, grass) in inserted_grass.iter_mut() {
        if let Some(chunk) = cache.get_mut(&id) {
            chunk.explicit_count = grass.positions.len() as u32;
            let (xz, mut y): (Vec<Vec2>, Vec<f32>) =
                grass.positions.iter().map(|v| (v.xz(), v.y)).unzip();
            let buffer = render_device.create_buffer_with_data(&BufferInitDescriptor {
                label: "xz vertex buffer".into(),
                contents: bytemuck::cast_slice(xz.as_slice()),
                usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            });

            chunk.explicit_xz_buffer = Some(buffer);

            let view = prepare_texture_from_data(
                &mut y,
                &render_device,
                &render_queue,
                TextureFormat::R32Float,
            );
            let layout = pipeline.explicit_y_layout.clone();
            let bind_group_descriptor = BindGroupDescriptor {
                label: Some("grass explicit y positions bind group"),
                layout: &layout,
                entries: &[BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&view),
                }],
            };
            let bind_group = render_device.create_bind_group(&bind_group_descriptor);
            chunk.explicit_y_buffer = Some(bind_group);
            let layout = pipeline.uniform_height_layout.clone();

            let buffer = render_device.create_buffer_with_data(&BufferInitDescriptor {
                label: "height buffer".into(),
                contents: &grass.height.to_ne_bytes(),
                usage: BufferUsages::VERTEX | BufferUsages::COPY_DST | BufferUsages::UNIFORM,
            });
            let bind_group_descriptor = BindGroupDescriptor {
                label: Some("grass height bind group"),
                layout: &layout,
                entries: &[BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::Buffer(BufferBinding {
                        buffer: &buffer,
                        offset: 0,
                        size: NonZeroU64::new(4),
                    }),
                }],
            };
            let bind_group = render_device.create_bind_group(&bind_group_descriptor);
            chunk.height_buffer = Some(bind_group);
        } else {
            warn!(
                "Tried to prepare a entity buffer for a grass chunk which wasn't registered before"
            );
        }
    }
}

pub(crate) fn prepare_height_buffer(
    mut cache: ResMut<GrassCache>,
    pipeline: Res<GrassPipeline>,
    fallback_img: Res<FallbackImage>,
    images: Res<RenderAssets<Image>>,

    render_device: Res<RenderDevice>,
    inserted_grass: Query<(Entity, &WarblerHeight)>,
    mut local_height_map_storage: Local<Vec<(Entity, Handle<Image>)>>,
) {
    let mut has_loaded = Vec::new();
    for (entity, heights_texture) in local_height_map_storage.iter() {
        if let Some(chunk) = cache.get_mut(entity) {
            let layout = pipeline.heights_texture_layout.clone();
            if let Some(tex) = images.get(&heights_texture) {
                has_loaded.push(*entity);
                let bind_group_descriptor = BindGroupDescriptor {
                    label: Some("grass height map bind group"),
                    layout: &layout,
                    entries: &[BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::TextureView(&tex.texture_view),
                    }],
                };

                let bind_group = render_device.create_bind_group(&bind_group_descriptor);
                chunk.blade_height_texture = Some(bind_group);
            }
        } else {
            warn!("Tried to prepare a buffer for a grass chunk which wasn't registered before");
        }
    }
    local_height_map_storage.retain(|(e,_)| !has_loaded.contains(e));

    for (entity, height) in inserted_grass.iter() {
        if let Some(chunk) = cache.get_mut(&entity) {
            match height.clone() {
                WarblerHeight::Uniform(height) => {
                    let layout = pipeline.uniform_height_layout.clone();

                    let buffer = render_device.create_buffer_with_data(&BufferInitDescriptor {
                        label: "grass blade height buffer".into(),
                        contents: &height.to_ne_bytes(),
                        usage: BufferUsages::VERTEX
                            | BufferUsages::COPY_DST
                            | BufferUsages::UNIFORM,
                    });
                    let bind_group_descriptor = BindGroupDescriptor {
                        label: Some("grass blade height bind group"),
                        layout: &layout,
                        entries: &[BindGroupEntry {
                            binding: 0,
                            resource: BindingResource::Buffer(BufferBinding {
                                buffer: &buffer,
                                offset: 0,
                                size: NonZeroU64::new(4),
                            }),
                        }],
                    };
                    let bind_group = render_device.create_bind_group(&bind_group_descriptor);
                    chunk.height_buffer = Some(bind_group);
                }
                WarblerHeight::Texture(heights_texture) => {
                    let layout = pipeline.heights_texture_layout.clone();

                    let tex = if let Some(tex) = images.get(&heights_texture) {
                        &tex.texture_view
                    } else {
                        // if the texture is not loaded, we will push it locally and try next frame again
                        local_height_map_storage
                            .push((entity, heights_texture.clone()));

                        &fallback_img.texture_view
                    };

                    let bind_group_descriptor = BindGroupDescriptor {
                        label: Some("grass height map bind group"),
                        layout: &layout,
                        entries: &[BindGroupEntry {
                            binding: 0,
                            resource: BindingResource::TextureView(tex),
                        }],
                    };

                    let bind_group = render_device.create_bind_group(&bind_group_descriptor);
                    chunk.blade_height_texture = Some(bind_group);
                }
            };
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
    inserted_grass: Query<(Entity, &HeightMap,&Aabb)>,
    mut local_height_map_storage: Local<Vec<(Entity, Handle<Image>, Aabb)>>,
) {
    let mut has_loaded = Vec::new();
    let layout = pipeline.height_map_layout.clone();

    for (e, handle, aabb) in local_height_map_storage.iter() {
        if let Some(tex) = images.get(handle) {
            has_loaded.push(*e);
            let height_map_texture = &tex.texture_view;
            let aabb_buffer = render_device.create_buffer_with_data(&BufferInitDescriptor {
                label: Some("aabb buffer"),
                contents: bytemuck::bytes_of(&Vec3::from(aabb.half_extents.mul(2.))),
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            });
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
            if let Some(chunk) = cache.get_mut(e) {
                chunk.height_map = Some(bind_group);
            } else {
                warn!("Tried to prepare a buffer for a grass chunk which wasn't registered before");
            }
        }
    }
    local_height_map_storage.retain(|(e,_,_)| !has_loaded.contains(e));

    for (entity, height_map, aabb) in inserted_grass.iter() {
        let height_map_texture = if let Some(tex) = images.get(&height_map.height_map) {
            &tex.texture_view
        } else {
            // if the texture is not loaded, we will push it locally and try next frame again
            local_height_map_storage.push((
                entity,
                height_map.height_map.clone(),
                *aabb,
            ));
            &fallback_img.texture_view
        };

        let aabb_buffer = render_device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("aabb buffer"),
            contents: bytemuck::bytes_of(&Vec3::from(aabb.half_extents.mul(2.))),
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        });

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
        if let Some(chunk) = cache.get_mut(&entity) {
            chunk.height_map = Some(bind_group);
        } else {
            warn!("Tried to prepare a buffer for a grass chunk which wasn't registered before");
        }
    }
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_uniform_buffers(
    pipeline: Res<GrassPipeline>,
    mut cache: ResMut<GrassCache>,
    region_config: Res<GrassConfiguration>,
    noise_config: Res<GrassNoiseTexture>,
    fallback_img: Res<FallbackImage>,
    render_device: Res<RenderDevice>,
    images: Res<RenderAssets<Image>>,
    mut last_texture_id: Local<Option<TextureViewId>>,
) {
    let texture = &images
        .get(&noise_config.0)
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

#[derive(Debug, Clone, Copy, Pod, Zeroable)]
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

fn prepare_texture_from_data<T: Default + Clone + bytemuck::Pod>(
    data: &mut Vec<T>,
    render_device: &RenderDevice,
    render_queue: &RenderQueue,
    format: TextureFormat,
) -> TextureView {
    let device = render_device.wgpu_device();

    // the dimensions of the texture are choosen to be nxn for the tiniest n which can contain the data
    let sqrt = (data.len() as f32).sqrt() as u32 + 1;
    let fill_data = vec![T::default(); (sqrt * sqrt) as usize - data.len()];
    data.extend(fill_data);
    let texture_size = Extent3d {
        width: sqrt,
        height: sqrt,
        depth_or_array_layers: 1,
    };
    // wgpu expects a byte array
    let data_slice = bytemuck::cast_slice(data.as_slice());
    // the texture is empty per default
    let texture = device.create_texture(&TextureDescriptor {
        size: texture_size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
        label: None,
        view_formats: &[],
    });
    let t_size = mem::size_of::<T>();

    // write data to texture
    render_queue.write_texture(
        ImageCopyTexture {
            texture: &texture,
            mip_level: 0,
            origin: Origin3d::ZERO,
            aspect: TextureAspect::All,
        },
        data_slice,
        ImageDataLayout {
            offset: 0,
            bytes_per_row: NonZeroU32::new(t_size as u32 * texture_size.width),
            rows_per_image: NonZeroU32::new(texture_size.height),
        },
        texture_size,
    );
    texture
        .create_view(&TextureViewDescriptor {
            label: None,
            format: Some(format),
            dimension: Some(TextureViewDimension::D2),
            aspect: TextureAspect::All,
            base_mip_level: 0,
            mip_level_count: NonZeroU32::new(1),
            base_array_layer: 0,
            array_layer_count: NonZeroU32::new(1),
        })
        .into()
}
