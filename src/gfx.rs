//! wgpu renderer (ARCHITECTURE.md §7): the scene stays on the CPU, the GPU
//! only rasterizes.
//!
//! One pipeline of instanced quads. Each [`Prim`] is a signed-distance
//! primitive (rounded box or round-capped segment) evaluated per fragment,
//! which gives analytic antialiasing and soft shadows without MSAA or any
//! tessellation. Instances are blended in submission order.
//!
//! An image is that same box with a texture in it: the frame's instances
//! are cut into [`scene::runs`] wherever the texture changes, and each run
//! is one draw over the same buffer — so the document's paint order
//! survives without a second pipeline or an atlas.

use std::sync::Arc;

use anyhow::Context as _;
use wgpu::util::DeviceExt as _;

use crate::bitmap::Bitmap;
use crate::scene::{self, ImageSlots, Prim, Rgba};

const SHADER: &str = r#"
struct Globals {
    viewport: vec2<f32>,
    _pad: vec2<f32>,
};
@group(0) @binding(0) var<uniform> globals: Globals;

struct Inst {
    @location(0) geom: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) radius: f32,
    @location(3) feather: f32,
    @location(4) kind: u32,
    @location(5) angle: f32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) px: vec2<f32>,
    @location(1) @interpolate(flat) geom: vec4<f32>,
    @location(2) @interpolate(flat) color: vec4<f32>,
    @location(3) @interpolate(flat) params: vec2<f32>,
    @location(4) @interpolate(flat) kind: u32,
    @location(5) @interpolate(flat) angle: f32,
};

const KIND_SEGMENT: u32 = 1u;
const KIND_IMAGE: u32 = 2u;

@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Inst) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    // Rasterize the primitive's bounds plus room for the edge ramp.
    let margin = max(inst.feather, 1.0) * 0.5 + 1.0;
    var lo: vec2<f32>;
    var hi: vec2<f32>;
    if (inst.kind == KIND_SEGMENT) {
        lo = min(inst.geom.xy, inst.geom.zw) - vec2<f32>(inst.radius + margin);
        hi = max(inst.geom.xy, inst.geom.zw) + vec2<f32>(inst.radius + margin);
    } else {
        // The turned box's extents, projected on the axes.
        let half = inst.geom.zw * 0.5;
        let c = abs(cos(inst.angle));
        let s = abs(sin(inst.angle));
        let extent = vec2<f32>(c * half.x + s * half.y, s * half.x + c * half.y);
        let center = inst.geom.xy + half;
        lo = center - extent - vec2<f32>(margin);
        hi = center + extent + vec2<f32>(margin);
    }
    let px = mix(lo, hi, corners[vi]);
    // Screen px (y down) -> NDC (y up).
    let ndc = vec2<f32>(
        px.x / globals.viewport.x * 2.0 - 1.0,
        1.0 - px.y / globals.viewport.y * 2.0,
    );
    var out: VsOut;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.px = px;
    out.geom = inst.geom;
    out.color = inst.color;
    out.params = vec2<f32>(inst.radius, inst.feather);
    out.kind = inst.kind;
    out.angle = inst.angle;
    return out;
}

fn sd_box(p: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - half + vec2<f32>(r);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

fn sd_segment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
    return length(pa - ba * h);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var d: f32;
    var rgba = in.color;
    if (in.kind == KIND_SEGMENT) {
        d = sd_segment(in.px, in.geom.xy, in.geom.zw) - in.params.x;
    } else {
        let half = in.geom.zw * 0.5;
        // A radius beyond the half extents would invert the field.
        let r = min(in.params.x, min(half.x, half.y));
        // Undo the turn: sample the field in the box's own axes.
        let p = in.px - (in.geom.xy + half);
        let c = cos(in.angle);
        let s = sin(in.angle);
        let local = vec2<f32>(c * p.x + s * p.y, c * p.y - s * p.x);
        d = sd_box(local, half, r);
        if (in.kind == KIND_IMAGE) {
            // The box's own axes are already the texture's: the corner at
            // -half is (0, 0). There is no mip chain to pick from, and
            // asking for level 0 keeps the sample out of the derivative
            // rules that a branch like this one would otherwise break.
            let uv = (local + half) / max(in.geom.zw, vec2<f32>(1e-6));
            rgba = textureSampleLevel(tex, samp, uv, 0.0) * in.color;
        }
    }
    let ramp = max(in.params.y, 1.0);
    let coverage = clamp(0.5 - d / ramp, 0.0, 1.0);
    return vec4<f32>(rgba.rgb, rgba.a * coverage);
}
"#;

pub struct Gfx {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    globals_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    /// Layout every texture bind group is built with.
    tex_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// One bind group per slot; slot 0 is the 1x1 white stand-in bound by
    /// the runs that draw no image, so every draw has group 1.
    textures: Vec<wgpu::BindGroup>,
    slots: ImageSlots,
}

impl Gfx {
    pub fn new(window: Arc<winit::window::Window>) -> anyhow::Result<Gfx> {
        // No display handle: only the GL backend needs one; Vulkan (our
        // target, §10.1) ignores it. Switch to winit's OwnedDisplayHandle if
        // GL ever comes in.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(window.clone())
            .context("creating the Wayland surface")?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            // A whiteboard does not justify waking a discrete GPU.
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .context("no compatible GPU adapter")?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("omawhite"),
            ..Default::default()
        }))
        .context("creating the wgpu device")?;

        let size = window.inner_size();
        let config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .context("surface has no default configuration")?;
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("omawhite-prims"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buf.as_entire_binding(),
            }],
        });

        let tex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("image"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("omawhite"),
            bind_group_layouts: &[Some(&bgl), Some(&tex_bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("prims"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Prim>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x4, // geom
                        1 => Float32x4, // color
                        2 => Float32,   // radius
                        3 => Float32,   // feather
                        4 => Uint32,    // kind
                        5 => Float32,   // angle
                        // `slot` stays on the CPU: it picks the bind group.
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        // Slot 0: what the runs with no image bind. White and opaque, so
        // the shader path is the same whatever it lands on.
        let blank = upload(
            &device,
            &queue,
            &tex_bgl,
            &sampler,
            texture_format(config.format),
            &Bitmap {
                w: 1,
                h: 1,
                rgba: vec![255, 255, 255, 255],
            },
        )?;

        Ok(Gfx {
            surface,
            device,
            queue,
            config,
            pipeline,
            globals_buf,
            bind_group,
            tex_bgl,
            sampler,
            textures: vec![blank],
            slots: ImageSlots::new(),
        })
    }

    /// Which blob is in which texture slot — what `scene` needs to decide
    /// between a textured box and a placeholder.
    pub fn image_slots(&self) -> &ImageSlots {
        &self.slots
    }

    /// Uploads `bmp` as the texture for `blob`, and answers its slot.
    /// Uploading the same blob twice keeps the first texture.
    pub fn upload_image(&mut self, blob: &str, bmp: &Bitmap) -> anyhow::Result<u32> {
        if let Some(&slot) = self.slots.get(blob) {
            return Ok(slot);
        }
        let max = self.device.limits().max_texture_dimension_2d;
        anyhow::ensure!(
            bmp.w <= max && bmp.h <= max,
            "image is {}x{} px, over this GPU's {max} px texture limit",
            bmp.w,
            bmp.h
        );
        let group = upload(
            &self.device,
            &self.queue,
            &self.tex_bgl,
            &self.sampler,
            texture_format(self.config.format),
            bmp,
        )?;
        let slot = self.textures.len() as u32;
        self.textures.push(group);
        self.slots.insert(blob.to_owned(), slot);
        Ok(slot)
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        self.config.width = w.max(1);
        self.config.height = h.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    /// Renders one frame: clear to `background`, then `prims` in order,
    /// one draw per texture run.
    /// `Ok(false)` = frame skipped (surface occluded or temporarily lost);
    /// the caller may try again later.
    pub fn render(&mut self, background: Rgba, prims: &[Prim]) -> anyhow::Result<bool> {
        let texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return Ok(false);
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(false);
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                anyhow::bail!("validation error acquiring the surface frame");
            }
        };
        let view = texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let globals: [f32; 4] = [
            self.config.width as f32,
            self.config.height as f32,
            0.0,
            0.0,
        ];
        self.queue
            .write_buffer(&self.globals_buf, 0, bytemuck::cast_slice(&globals));

        // A buffer per frame is the simplest thing that works; reuse and
        // per-element caching come with real profiling (§3).
        let instance_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("prims"),
                contents: bytemuck::cast_slice(prims),
                usage: wgpu::BufferUsages::VERTEX,
            });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        {
            let [r, g, b, a] = background;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("canvas"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: f64::from(r),
                            g: f64::from(g),
                            b: f64::from(b),
                            a: f64::from(a),
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if !prims.is_empty() {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, instance_buf.slice(..));
                for run in scene::runs(prims) {
                    let group = self
                        .textures
                        .get(run.slot as usize)
                        .unwrap_or(&self.textures[0]);
                    pass.set_bind_group(1, group, &[]);
                    pass.draw(0..6, run.start..run.end);
                }
            }
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(texture);
        Ok(true)
    }
}

/// The texture format that matches the surface: the pipeline writes its
/// colors straight through, so an image has to go in the same space the
/// surface reads out.
fn texture_format(surface: wgpu::TextureFormat) -> wgpu::TextureFormat {
    if surface.is_srgb() {
        wgpu::TextureFormat::Rgba8UnormSrgb
    } else {
        wgpu::TextureFormat::Rgba8Unorm
    }
}

/// One texture from RGBA8 texels, with its bind group.
fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    format: wgpu::TextureFormat,
    bmp: &Bitmap,
) -> anyhow::Result<wgpu::BindGroup> {
    anyhow::ensure!(
        bmp.rgba.len() as u64 == 4 * u64::from(bmp.w) * u64::from(bmp.h),
        "{}x{} px needs {} bytes, got {}",
        bmp.w,
        bmp.h,
        4 * u64::from(bmp.w) * u64::from(bmp.h),
        bmp.rgba.len()
    );
    let size = wgpu::Extent3d {
        width: bmp.w.max(1),
        height: bmp.h.max(1),
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("image"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &bmp.rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * bmp.w),
            rows_per_image: Some(bmp.h),
        },
        size,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Ok(device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("image"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    }))
}
