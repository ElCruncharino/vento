// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! A picture the hardware decoded, drawn from the memory it was decoded
//! into: the VAAPI surface imported as DMA-BUFs and converted on the GPU,
//! with nothing read back to the processor and nothing converted there.
//!
//! On Lunar Lake the readback was most of what a hardware picture cost —
//! iHD's map of a 1080p surface is 4 to 7 ms, and the conversion after it
//! another 3 — so the hardware path was slower than software's. Left on the
//! GPU (`common::video::GpuPicture`), a picture costs the decode, the wait
//! for it, and a pass of this module's.
//!
//! **Converted once, then drawn like any other.** Each new picture is
//! converted by one render pass (`zerocopy.wgsl`) into a single `R, G, B,
//! A` texture of the picture's size, registered with egui-wgpu as a texture
//! of its own, which the desktop draws in place of its tiles (`desktop.rs`).
//! Everything about where and how the desktop is drawn — the view's scale
//! and pan, nearest at 1:1 and linear elsewhere, every pop-out window, the
//! cursor over it — is then the same code as for a picture read back, and
//! the pixels on screen are the same bytes through the same sampler. Drawing
//! the two planes straight into each window instead would save one write of
//! the picture on the GPU (a fraction of a millisecond) and cost a second
//! way of drawing the desktop, converted again in every window every frame.
//!
//! **The arithmetic is the processor's.** The pass is
//! `common::video::convert`'s integer expression, with its coefficients, and
//! P010 narrowed to eight bits as `platform_linux`'s `narrow` narrows it, so
//! a pixel is the one a readback would have made; the tests below hold it to
//! that, for every pair of chroma samples.
//!
//! **Imported once per surface.** The decoder decodes into a pool of
//! surfaces it reuses, so what an import makes of one (two textures over
//! its memory and a bind group) is kept by the surface's key and used again
//! the next time a picture is in it; a key of a new pool (a new decoder, a
//! new size) drops them all. A kept import holds the surface's memory even
//! once the decoder has let go of it, so they are dropped with the session.
//!
//! **What waits for what.** The decoder waits for each surface before
//! handing it out (`vaSyncSurface`), so the decode is in memory before
//! this module sees it; no fence of the decoder's is waited on here, and
//! none is needed. The picture is kept until the GPU has finished the pass
//! that reads it (`on_submitted_work_done`), and only then is the surface
//! the decoder's again. An imported texture starts undefined and is made
//! readable on its first use; after that wgpu takes it to be as it left
//! it, and on Intel's i915 and Xe each batch starts with the GPU's caches
//! invalidated, so a later picture in the same surface is read fresh.
//!
//! **Only on Vulkan, and only where it works.** The import is wgpu-hal's
//! `texture_from_dmabuf_fd`, Vulkan's, with the device asked for
//! `VULKAN_EXTERNAL_MEMORY_DMA_BUF` (and 16-bit textures for P010) where it
//! has them (`mod.rs`). Anywhere else — GL, a driver without the
//! extensions, an import or a pass that fails, a picture past the device's
//! largest texture — the window says why once and the session goes on with
//! its pictures read back, as without this.

use common::video::convert::coefficients;
use common::video::{Dmabuf, GpuPicture, fourcc};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;
use std::time::Instant;

/// How many surfaces' imports are kept at most: past any pool a decoder
/// makes, so that only a leak of keys would reach it.
const KEPT: usize = 64;

/// The conversion's state, made with the device and grown on first use.
pub struct Zerocopy {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// The largest texture the device makes.
    max_side: u32,
    /// Whether 16-bit textures (P010's) can be made.
    deep: bool,
    /// Made at the first picture, so a session that never has one makes
    /// nothing.
    pass: Option<Pass>,
    imports: Imports<Import>,
    target: Option<Target>,
    /// Nearest rather than linear, as the desktop's tiles would be.
    nearest: bool,
    /// What a picture costs here on the processor (import where it is one,
    /// the pass recorded and submitted), in milliseconds, smoothed.
    cost: Option<f32>,
}

/// The pipeline and what every pass binds besides the picture: the
/// picture's numbers, in a buffer written only when they change, which is
/// about once a stream.
struct Pass {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    params: wgpu::Buffer,
    written: std::cell::Cell<Option<[i32; 8]>>,
}

/// One surface imported: its two planes' textures, kept alive with the bind
/// group over them, and whether its samples are sixteen bits.
struct Import {
    _textures: [wgpu::Texture; 2],
    bind_group: wgpu::BindGroup,
    deep: bool,
}

/// The texture pictures are converted into, and egui's name for it once
/// it has been registered.
struct Target {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    id: Option<egui::TextureId>,
}

/// Imports kept by surface, all of one pool: a key of another drops them.
struct Imports<T> {
    pool: u64,
    kept: HashMap<u32, T>,
}

impl<T> Default for Imports<T> {
    fn default() -> Imports<T> {
        Imports {
            pool: 0,
            kept: HashMap::new(),
        }
    }
}

impl<T> Imports<T> {
    /// The import kept for `key`, or `make`'s, kept, where there is none:
    /// every other pool's let go of first.
    fn get<E>(&mut self, key: (u64, u32), make: impl FnOnce() -> Result<T, E>) -> Result<&T, E> {
        if key.0 != self.pool || self.kept.len() >= KEPT && !self.kept.contains_key(&key.1) {
            self.kept.clear();
            self.pool = key.0;
        }
        match self.kept.entry(key.1) {
            Entry::Occupied(kept) => Ok(kept.into_mut()),
            Entry::Vacant(slot) => Ok(slot.insert(make()?)),
        }
    }

    fn clear(&mut self) {
        self.kept.clear();
        self.pool = 0;
    }
}

impl Zerocopy {
    /// The conversion on `device`, or why there can be none: not Vulkan, or
    /// a device without the DMA-BUF import.
    pub fn new(adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Zerocopy, String> {
        let backend = adapter.get_info().backend;
        if backend != wgpu::Backend::Vulkan {
            return Err(format!("wgpu is on {backend:?}, and only Vulkan imports DMA-BUFs here"));
        }
        if !device.features().contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF) {
            return Err("the Vulkan device cannot import DMA-BUFs".into());
        }
        Ok(Zerocopy {
            device: device.clone(),
            queue: queue.clone(),
            max_side: device.limits().max_texture_dimension_2d,
            deep: device.features().contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM),
            pass: None,
            imports: Imports::default(),
            target: None,
            nearest: false,
            cost: None,
        })
    }

    /// What a picture has been costing here, in milliseconds.
    pub fn cost(&self) -> Option<f32> {
        self.cost
    }

    /// `picture` converted into the desktop's texture, which comes back by
    /// egui's name for it (the same one every time), or why it could not be.
    pub fn show(&mut self, picture: &Arc<dyn GpuPicture>, renderer: &mut egui_wgpu::Renderer) -> Result<egui::TextureId, String> {
        let started = Instant::now();
        let fresh = self.convert(picture)?;
        let target = self.target.as_mut().expect("made by convert");
        let filter = filter(self.nearest);
        let id = match target.id {
            None => *target.id.insert(renderer.register_native_texture(&self.device, &target.view, filter)),
            Some(id) => {
                if fresh {
                    renderer.update_egui_texture_from_wgpu_texture(&self.device, &target.view, filter, id);
                }
                id
            }
        };
        let ms = started.elapsed().as_secs_f32() * 1000.0;
        self.cost = Some(self.cost.map_or(ms, |m| m + (ms - m) / 16.0));
        Ok(id)
    }

    /// Nearest where every window draws the desktop one to one, as the
    /// tiles are filtered (see `Desktop::set_exact`).
    pub fn set_nearest(&mut self, nearest: bool, renderer: &mut egui_wgpu::Renderer) {
        if nearest == self.nearest {
            return;
        }
        self.nearest = nearest;
        if let Some(Target { view, id: Some(id), .. }) = &self.target {
            renderer.update_egui_texture_from_wgpu_texture(&self.device, view, filter(nearest), *id);
        }
    }

    /// Everything a session left here let go of: the imports, which hold
    /// its surfaces' memory, and the texture.
    pub fn forget(&mut self, renderer: &mut egui_wgpu::Renderer) {
        self.imports.clear();
        if let Some(Target { id: Some(id), .. }) = self.target.take() {
            renderer.free_texture(&id);
        }
        self.cost = None;
    }

    /// `picture` converted into the target, which is made anew where it is
    /// not the picture's size (true then). The picture is held until the
    /// GPU has read it.
    fn convert(&mut self, picture: &Arc<dyn GpuPicture>) -> Result<bool, String> {
        let (width, height) = (picture.width() as u32, picture.height() as u32);
        if width == 0 || height == 0 || width > self.max_side || height > self.max_side {
            return Err(format!(
                "a {width}×{height} picture is past this device's {}-pixel textures",
                self.max_side
            ));
        }
        if self.pass.is_none() {
            self.pass = Some(scoped(&self.device, "the conversion's pipeline", || make_pass(&self.device))?);
        }
        let (device, deep_ok) = (&self.device, self.deep);
        let pass = self.pass.as_ref().expect("made above");
        let import = self.imports.get(picture.surface(), || {
            let dmabuf = picture.export()?;
            scoped(device, "the import", || import(device, &pass.layout, &pass.params, dmabuf, deep_ok)).and_then(|r| r)
        })?;
        let (bt709, full_range) = picture.colour();
        let params = params(bt709, full_range, import.deep);

        let fresh = self
            .target
            .as_ref()
            .is_none_or(|t| (t.texture.width(), t.texture.height()) != (width, height));
        if fresh {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("vento desktop (GPU picture)"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let id = self.target.take().and_then(|t| t.id);
            self.target = Some(Target { texture, view, id });
        }
        let target = self.target.as_ref().expect("made above");
        scoped(&self.device, "the conversion", || {
            run(&self.device, &self.queue, pass, &import.bind_group, params, &target.view)
        })?;
        // The surface is the decoder's again once the pass has read it.
        let held = Arc::clone(picture);
        self.queue.on_submitted_work_done(move || drop(held));
        Ok(fresh)
    }

    /// The target, for a test to read back.
    #[cfg(test)]
    fn target(&self) -> Option<&wgpu::Texture> {
        self.target.as_ref().map(|t| &t.texture)
    }
}

/// What the pass is told of a picture: `convert`'s coefficients for its
/// colour, and whether its samples are sixteen bits.
fn params(bt709: bool, full_range: bool, deep: bool) -> [i32; 8] {
    let c = coefficients(bt709, full_range);
    [c.y, c.rv, c.gu, c.gv, c.bu, c.y_offset, i32::from(deep), 0]
}

/// One pass over `target`, reading the planes `bind_group` binds, submitted.
fn run(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pass: &Pass,
    bind_group: &wgpu::BindGroup,
    params: [i32; 8],
    target: &wgpu::TextureView,
) {
    // Staged and written before the submission below, so each pass has its
    // own picture's numbers though the buffer is the one.
    if pass.written.get() != Some(params) {
        queue.write_buffer(&pass.params, 0, bytemuck::cast_slice(&params));
        pass.written.set(Some(params));
    }
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("vento GPU picture") });
    {
        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("vento GPU picture"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Every pixel is written.
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        rp.set_pipeline(&pass.pipeline);
        rp.set_bind_group(0, bind_group, &[]);
        rp.draw(0..3, 0..1);
    }
    queue.submit(std::iter::once(encoder.finish()));
}

fn filter(nearest: bool) -> wgpu::FilterMode {
    if nearest {
        wgpu::FilterMode::Nearest
    } else {
        wgpu::FilterMode::Linear
    }
}

/// `make` with wgpu's validation errors caught rather than its default of
/// panicking on them: an import of a buffer the driver will not take is a
/// reason to read pictures back, not the end of the process.
fn scoped<T>(device: &wgpu::Device, what: &str, make: impl FnOnce() -> T) -> Result<T, String> {
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let made = make();
    match pollster::block_on(scope.pop()) {
        Some(error) => Err(format!("{what} failed: {error}")),
        None => Ok(made),
    }
}

fn make_pass(device: &wgpu::Device) -> Pass {
    let module = device.create_shader_module(wgpu::include_wgsl!("zerocopy.wgsl"));
    let plane = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("vento GPU picture"),
        entries: &[
            plane(0),
            plane(1),
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("vento GPU picture"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("vento GPU picture"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    let params = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("vento GPU picture"),
        size: 32,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    Pass {
        layout,
        pipeline,
        params,
        written: std::cell::Cell::new(None),
    }
}

/// The texture formats of a DMA-BUF's two layers, and whether they are
/// sixteen bits a sample: NV12's and P010's.
fn formats(layers: [u32; 2]) -> Option<([wgpu::TextureFormat; 2], bool)> {
    use wgpu::TextureFormat::*;
    match layers {
        [fourcc::R8, fourcc::GR88] => Some(([R8Unorm, Rg8Unorm], false)),
        [fourcc::R16, fourcc::GR1616] => Some(([R16Unorm, Rg16Unorm], true)),
        _ => None,
    }
}

/// A DRM fourcc as its four characters.
fn fourcc_name(code: u32) -> String {
    code.to_le_bytes().iter().map(|b| *b as char).collect::<String>().trim_end().to_string()
}

/// A surface's two layers imported as textures, with the bind group a pass
/// reads them through.
fn import(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    params: &wgpu::Buffer,
    dmabuf: Dmabuf,
    deep_ok: bool,
) -> Result<Import, String> {
    let codes: Vec<u32> = dmabuf.layers.iter().map(|l| l.fourcc).collect();
    let Some((formats, deep)) = <[u32; 2]>::try_from(codes.as_slice()).ok().and_then(formats) else {
        let names: Vec<String> = codes.iter().map(|c| fourcc_name(*c)).collect();
        return Err(format!("its layers are {}, not NV12's or P010's", names.join(" and ")));
    };
    if deep && !deep_ok {
        return Err("the device has no 16-bit textures for P010".into());
    }
    let mut textures = Vec::with_capacity(2);
    for (layer, format) in dmabuf.layers.into_iter().zip(formats) {
        let size = wgpu::Extent3d {
            width: layer.width,
            height: layer.height,
            depth_or_array_layers: 1,
        };
        let hal_desc = wgpu::hal::TextureDescriptor {
            label: Some("vento DMA-BUF"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUses::RESOURCE,
            memory_flags: wgpu::hal::MemoryFlags::empty(),
            view_formats: Vec::new(),
        };
        let raw = {
            // SAFETY: the hal device is only used for these calls, which ask
            // about a format and make a texture of its own; nothing is
            // destroyed through it.
            let Some(hal) = (unsafe { device.as_hal::<wgpu::hal::api::Vulkan>() }) else {
                return Err("the device is not Vulkan's".into());
            };
            if !takes(&hal, format, layer.modifier) {
                return Err(format!(
                    "the device does not sample {:?} laid out as {:#x}",
                    format, layer.modifier
                ));
            }
            // SAFETY: the descriptor is the export's, a DMA-BUF of this
            // layout (its modifier, pitch and offset), and the plane's size
            // in samples is the surface's; the import takes the descriptor.
            unsafe {
                hal.texture_from_dmabuf_fd(
                    layer.fd,
                    &hal_desc,
                    layer.modifier,
                    u64::from(layer.pitch),
                    u64::from(layer.offset),
                )
            }
            .map_err(|e| {
                format!(
                    "the {} layer (modifier {:#x}) did not import: {e}",
                    fourcc_name(layer.fourcc),
                    layer.modifier
                )
            })?
        };
        // SAFETY: made on this device, as the descriptor says; undefined
        // until its first use, which wgpu makes readable.
        let texture = unsafe {
            device.create_texture_from_hal::<wgpu::hal::api::Vulkan>(
                raw,
                &wgpu::TextureDescriptor {
                    label: Some("vento DMA-BUF"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::TextureUses::UNINITIALIZED,
            )
        };
        textures.push(texture);
    }
    let [y, uv]: [wgpu::Texture; 2] = textures.try_into().map_err(|_| "two layers".to_string())?;
    let bind_group = bind(device, layout, params, &y, &uv);
    Ok(Import {
        _textures: [y, uv],
        bind_group,
        deep,
    })
}

/// Whether the device samples single-plane images of `format` with the
/// layout `modifier`. wgpu-hal's import does not ask, and an image made with
/// a modifier the driver does not have is undefined behaviour rather than an
/// error: lavapipe, which is what wgpu has on an X server without DRI3, has
/// only the linear layout, and a GPU of another vendor than the decoder's
/// none of Intel's tiled ones. Asked, either is a reason to read back.
fn takes(hal: &wgpu::hal::vulkan::Device, format: wgpu::TextureFormat, modifier: u64) -> bool {
    use ash::vk;
    let format = match format {
        wgpu::TextureFormat::R8Unorm => vk::Format::R8_UNORM,
        wgpu::TextureFormat::Rg8Unorm => vk::Format::R8G8_UNORM,
        wgpu::TextureFormat::R16Unorm => vk::Format::R16_UNORM,
        wgpu::TextureFormat::Rg16Unorm => vk::Format::R16G16_UNORM,
        _ => return false,
    };
    let instance = hal.shared_instance().raw_instance();
    let physical = hal.raw_physical_device();
    // Twice, as Vulkan lists anything: how many, then them.
    let mut list = vk::DrmFormatModifierPropertiesListEXT::default();
    let mut properties = vk::FormatProperties2::default().push_next(&mut list);
    // SAFETY: the instance and the physical device are the device's own,
    // and VK_EXT_image_drm_format_modifier is one of the extensions
    // VULKAN_EXTERNAL_MEMORY_DMA_BUF is had for.
    unsafe { instance.get_physical_device_format_properties2(physical, format, &mut properties) };
    let mut modifiers = vec![vk::DrmFormatModifierPropertiesEXT::default(); list.drm_format_modifier_count as usize];
    let mut list = vk::DrmFormatModifierPropertiesListEXT::default().drm_format_modifier_properties(&mut modifiers);
    let mut properties = vk::FormatProperties2::default().push_next(&mut list);
    // SAFETY: as above, with room for as many as it said.
    unsafe { instance.get_physical_device_format_properties2(physical, format, &mut properties) };
    modifiers.iter().any(|m| {
        m.drm_format_modifier == modifier
            && m.drm_format_modifier_plane_count == 1
            && m.drm_format_modifier_tiling_features.contains(vk::FormatFeatureFlags::SAMPLED_IMAGE)
    })
}

fn bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    params: &wgpu::Buffer,
    y: &wgpu::Texture,
    uv: &wgpu::Texture,
) -> wgpu::BindGroup {
    let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("vento GPU picture"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view(y)),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&view(uv)),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: params.as_entire_binding(),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::video::convert::{Frame, Plane};
    use common::video::{Changes, Codec, Decoded, Decoder as _, Hardware, Output};

    #[test]
    fn imports_are_kept_by_surface_and_dropped_with_their_pool() {
        let mut imports = Imports::<u32>::default();
        let mut made = 0;
        let mut get = |imports: &mut Imports<u32>, key: (u64, u32)| {
            *imports
                .get(key, || -> Result<u32, ()> {
                    made += 1;
                    Ok(made)
                })
                .unwrap()
        };
        assert_eq!(get(&mut imports, (1, 5)), 1);
        assert_eq!(get(&mut imports, (1, 5)), 1, "kept");
        assert_eq!(get(&mut imports, (1, 6)), 2);
        assert_eq!(get(&mut imports, (1, 5)), 1, "both kept");
        // Another pool's surface of the same id is other memory.
        assert_eq!(get(&mut imports, (2, 5)), 3);
        assert_eq!(imports.kept.len(), 1, "the old pool's dropped");
        assert_eq!(get(&mut imports, (2, 6)), 4);
        // Past the most kept, all are let go of rather than one grown past.
        for id in 100..100 + KEPT as u32 {
            get(&mut imports, (2, id));
        }
        assert!(imports.kept.len() <= KEPT);
        // An import that fails is not kept.
        assert!(imports.get((3, 1), || Err::<u32, &str>("no")).is_err());
        assert!(imports.kept.is_empty());
        imports.clear();
        assert_eq!(imports.pool, 0);
    }

    #[test]
    fn layers_name_their_formats() {
        assert_eq!(
            formats([fourcc::R8, fourcc::GR88]),
            Some(([wgpu::TextureFormat::R8Unorm, wgpu::TextureFormat::Rg8Unorm], false))
        );
        assert_eq!(
            formats([fourcc::R16, fourcc::GR1616]),
            Some(([wgpu::TextureFormat::R16Unorm, wgpu::TextureFormat::Rg16Unorm], true))
        );
        assert_eq!(formats([fourcc::GR88, fourcc::R8]), None);
        assert_eq!(fourcc_name(fourcc::R8), "R8");
        assert_eq!(fourcc_name(fourcc::GR1616), "GR32");
    }

    /// A device for the tests: the one the window would choose, with what
    /// the window asks of it; `None`, with a note, where there is none.
    fn gpu(test: &str) -> Option<(wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            ..Default::default()
        })) else {
            eprintln!("{test}: skipped, no GPU adapter");
            return None;
        };
        let features = adapter.features()
            & (wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF | wgpu::Features::TEXTURE_FORMAT_16BIT_NORM);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: features,
            ..Default::default()
        }))
        .ok()?;
        Some((adapter, device, queue))
    }

    /// A plane of `w × h` samples of `bytes` each as a texture.
    fn plane(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, w: u32, h: u32, data: &[u8]) -> wgpu::Texture {
        let size = wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let row = data.len() as u32 / h;
        queue.write_texture(
            texture.as_image_copy(),
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: None,
            },
            size,
        );
        texture
    }

    /// An `Rgba8Unorm` texture read back as the framebuffer's words.
    fn read_back(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u32> {
        let (w, h) = (texture.width(), texture.height());
        let row = (w * 4).div_ceil(256) * 256;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(row * h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: None,
                },
            },
            texture.size(),
        );
        queue.submit(std::iter::once(encoder.finish()));
        buffer.map_async(wgpu::MapMode::Read, .., |r| r.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let bytes = buffer.get_mapped_range(..).unwrap();
        let mut words = Vec::with_capacity((w * h) as usize);
        for y in 0..h as usize {
            let line = &bytes[y * row as usize..][..w as usize * 4];
            words.extend(line.chunks_exact(4).map(|p| u32::from_le_bytes(p.try_into().unwrap())));
        }
        words
    }

    /// NV12 converted on the processor, as a decoder's `Output` converts it.
    fn on_the_processor(y: &[u8], uv: &[u8], w: usize, h: usize, bt709: bool, full_range: bool) -> Vec<u32> {
        let at = |offset, col_inc, row_inc, sub| Plane {
            offset,
            col_inc,
            row_inc,
            horiz_subsampling: sub,
            vert_subsampling: sub,
        };
        let frame = Frame {
            bytes: [y, uv, uv],
            planes: [at(0, 1, w, 1), at(0, 2, w, 2), at(1, 2, w, 2)],
            width: w,
            height: h,
            bt709,
            full_range,
        };
        let mut out = vec![0u32; w * h];
        let mut changed = Changes::new();
        changed.reset(h);
        changed.all(w, h);
        Output::new().write(&frame, &mut out, w, h, &mut changed);
        out
    }

    /// The pass against `convert`, on every pair of chroma samples there is
    /// (U across, V down, each at every value) under a luma of every value,
    /// in all four colours: the same words, every one. And sixteen-bit
    /// samples narrowed as eight-bit ones would be, which P010 is.
    #[test]
    fn the_pass_is_converts_arithmetic_exactly() {
        let Some((_, device, queue)) = gpu("the_pass_is_converts_arithmetic_exactly") else {
            return;
        };
        let (w, h) = (512usize, 512usize);
        let y: Vec<u8> = (0..w * h).map(|i| ((i % w) * 7 + (i / w) * 13) as u8).collect();
        let uv: Vec<u8> = (0..w / 2 * h / 2).flat_map(|i| [(i % (w / 2)) as u8, (i / (w / 2)) as u8]).collect();
        let pass = make_pass(&device);
        let luma = plane(&device, &queue, wgpu::TextureFormat::R8Unorm, w as u32, h as u32, &y);
        let chroma = plane(&device, &queue, wgpu::TextureFormat::Rg8Unorm, w as u32 / 2, h as u32 / 2, &uv);
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: w as u32,
                height: h as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let eight = bind(&device, &pass.layout, &pass.params, &luma, &chroma);
        // The same samples sixteen bits wide, with bits below the eight that
        // round them up or not: the top eight, plus one where the rest are
        // at least half.
        let deep = device.features().contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM).then(|| {
            let widen = |s: &[u8], salt: usize| -> (Vec<u8>, Vec<u8>) {
                let wide: Vec<u16> = s.iter().enumerate().map(|(i, v)| u16::from(*v) << 8 | ((i * 37 + salt) & 0xff) as u16).collect();
                let narrow = wide.iter().map(|v| ((u32::from(*v) + 128) >> 8).min(255) as u8).collect();
                (wide.iter().flat_map(|v| v.to_le_bytes()).collect(), narrow)
            };
            let (y16, y8) = widen(&y, 3);
            let (uv16, uv8) = widen(&uv, 11);
            let luma = plane(&device, &queue, wgpu::TextureFormat::R16Unorm, w as u32, h as u32, &y16);
            let chroma = plane(&device, &queue, wgpu::TextureFormat::Rg16Unorm, w as u32 / 2, h as u32 / 2, &uv16);
            (bind(&device, &pass.layout, &pass.params, &luma, &chroma), y8, uv8)
        });
        if deep.is_none() {
            eprintln!("the_pass_is_converts_arithmetic_exactly: no 16-bit textures, P010's narrowing not tried");
        }
        for (bt709, full_range) in [(false, false), (true, false), (false, true), (true, true)] {
            run(&device, &queue, &pass, &eight, params(bt709, full_range, false), &view);
            let want = on_the_processor(&y, &uv, w, h, bt709, full_range);
            let got = read_back(&device, &queue, &target);
            let wrong = want.iter().zip(&got).filter(|(a, b)| a != b).count();
            assert_eq!(wrong, 0, "bt709 {bt709} full {full_range}: {wrong} pixels differ");
            if let Some((group, y8, uv8)) = &deep {
                run(&device, &queue, &pass, group, params(bt709, full_range, true), &view);
                let want = on_the_processor(y8, uv8, w, h, bt709, full_range);
                let got = read_back(&device, &queue, &target);
                let wrong = want.iter().zip(&got).filter(|(a, b)| a != b).count();
                assert_eq!(wrong, 0, "16-bit, bt709 {bt709} full {full_range}: {wrong} pixels differ");
            }
        }
    }

    /// The Linux platform crate's fixtures, cut into frames as its tests cut them.
    fn fixture(name: &str) -> (Codec, Vec<Vec<u8>>) {
        let path = format!("{}/../backends/platform/linux/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let ivf = |b: &[u8]| {
            let mut at = u16::from_le_bytes([b[6], b[7]]) as usize;
            let mut frames = Vec::new();
            while at + 12 <= b.len() {
                let size = u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize;
                frames.push(b[at + 12..at + 12 + size].to_vec());
                at += 12 + size;
            }
            frames
        };
        // Annex B at its access unit delimiters (H.264's NAL type 9, H.265's
        // 35), each frame from its start code.
        let annex_b = |b: &[u8], delimiter: fn(u8) -> bool| {
            let mut starts: Vec<usize> = (0..b.len().saturating_sub(3))
                .filter(|&i| b[i..i + 3] == [0, 0, 1] && delimiter(b[i + 3]))
                .map(|i| if i > 0 && b[i - 1] == 0 { i - 1 } else { i })
                .collect();
            starts.push(b.len());
            starts.windows(2).map(|w| b[w[0]..w[1]].to_vec()).collect::<Vec<_>>()
        };
        match name.rsplit('.').next() {
            Some("ivf") if name.starts_with("vp9") => (Codec::Vp9, ivf(&bytes)),
            Some("ivf") => (Codec::Av1, ivf(&bytes)),
            Some("h264") => (Codec::H264, annex_b(&bytes, |h| h & 0x1f == 9)),
            Some("h265") => (Codec::H265, annex_b(&bytes, |h| (h >> 1) & 0x3f == 35)),
            _ => panic!("{name}"),
        }
    }

    /// A VAAPI picture imported from its DMA-BUFs and converted here is the
    /// picture its readback is, word for word: NV12 off H.264, VP9 and AV1,
    /// and P010 off H.265 Main 10, each surface imported once. Skipped
    /// without a VAAPI device or a Vulkan one that imports DMA-BUFs.
    #[test]
    fn a_vaapi_picture_drawn_from_its_dmabufs_is_its_readback() {
        let test = "a_vaapi_picture_drawn_from_its_dmabufs_is_its_readback";
        let Some((adapter, device, queue)) = gpu(test) else {
            return;
        };
        let mut zerocopy = match Zerocopy::new(&adapter, &device, &queue) {
            Ok(z) => z,
            Err(why) => {
                eprintln!("{test}: skipped, {why}");
                return;
            }
        };
        for name in ["h264.h264", "vp9.ivf", "av1.ivf", "h265-main10.h265"] {
            let (codec, frames) = fixture(name);
            let mut decoder = platform_linux::ffmpeg::Decoder::with_hardware(codec, 128, 128, Hardware::Vaapi).unwrap();
            if decoder.method() != Hardware::Vaapi {
                eprintln!("{test}: skipped, no VAAPI ({:?})", decoder.software_reason());
                return;
            }
            for (i, frame) in frames.iter().cycle().take(frames.len() * 3).enumerate() {
                let mut out = vec![0u32; 128 * 128];
                let mut changed = Changes::new();
                changed.reset(128);
                let Decoded::Gpu(picture) = decoder.decode_picture(frame, &mut out, 128, 128, &mut changed) else {
                    panic!("{name}: picture {i} was not held");
                };
                match zerocopy.convert(&picture) {
                    Ok(_) => {}
                    // lavapipe, say, which has the import and only the
                    // linear layout: read back in the window, as here.
                    Err(why) if why.contains("does not sample") => {
                        eprintln!("{test}: skipped, {why}");
                        return;
                    }
                    Err(why) => panic!("{name}: {why}"),
                }
                let got = read_back(&device, &queue, zerocopy.target().unwrap());
                let mut want = vec![0u32; 128 * 128];
                assert!(picture.read(&mut want, 128, 128));
                let wrong = want.iter().zip(&got).filter(|(a, b)| a != b).count();
                assert_eq!(wrong, 0, "{name}: picture {i}: {wrong} pixels differ from the readback");
            }
            // A pool of a few surfaces, each imported once, however many
            // pictures went through them.
            assert!(zerocopy.imports.kept.len() <= 8, "{name}: {} imports", zerocopy.imports.kept.len());
        }
    }
}
