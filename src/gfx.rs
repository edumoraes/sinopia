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
use crate::scene::{self, Blend, Frame, ImageSlots, Onto, Prim, Rgba, Viewport};

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
    @location(6) uv: vec4<f32>,
    @location(7) clip: vec4<f32>,
    @location(8) falloff: f32,
    @location(9) paper: vec4<f32>,
    @location(10) weave: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) px: vec2<f32>,
    @location(1) @interpolate(flat) geom: vec4<f32>,
    @location(2) @interpolate(flat) color: vec4<f32>,
    @location(3) @interpolate(flat) params: vec2<f32>,
    @location(4) @interpolate(flat) kind: u32,
    @location(5) @interpolate(flat) angle: f32,
    @location(6) @interpolate(flat) uv: vec4<f32>,
    @location(7) @interpolate(flat) cut: vec4<f32>,
    @location(8) @interpolate(flat) falloff: f32,
    @location(9) @interpolate(flat) paper: vec4<f32>,
    @location(10) @interpolate(flat) weave: vec4<f32>,
};

const KIND_SEGMENT: u32 = 1u;
const KIND_IMAGE: u32 = 2u;
const KIND_GRAIN: u32 = 3u;

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
    // Nothing outside the clip is ever drawn, so do not rasterize it.
    if (inst.clip.z > 0.0 && inst.clip.w > 0.0) {
        lo = max(lo, inst.clip.xy - vec2<f32>(1.0));
        hi = min(hi, inst.clip.xy + inst.clip.zw + vec2<f32>(1.0));
        hi = max(hi, lo);
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
    out.uv = inst.uv;
    out.cut = inst.clip;
    out.falloff = inst.falloff;
    out.paper = inst.paper;
    out.weave = inst.weave;
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

// Straight alpha: what the window is blended with.
@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return shade(in);
}

// Premultiplied: what the scratch texture holds, so that a group's
// union — a max of every channel — is the union of its coverage.
@fragment
fn fs_premul(in: VsOut) -> @location(0) vec4<f32> {
    let c = shade(in);
    return vec4<f32>(c.rgb * c.a, c.a);
}

fn shade(in: VsOut) -> vec4<f32> {
    var d: f32;
    var rgba = in.color;
    // The cut is one more box in the same field: the far side of it is
    // outside the prim, and the edge ramps like any other.
    var cut = -1e9;
    if (in.cut.z > 0.0 && in.cut.w > 0.0) {
        let half = in.cut.zw * 0.5;
        cut = sd_box(in.px - (in.cut.xy + half), half, 0.0);
    }
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
        if (in.kind == KIND_IMAGE || in.kind == KIND_GRAIN) {
            // The box's own axes are already the texture's: the corner at
            // -half is (0, 0). An image maps onto the whole sheet, a
            // glyph onto its cell of the atlas. There is no mip chain to
            // pick from, and asking for level 0 keeps the sample out of
            // the derivative rules that a branch like this one would
            // otherwise break.
            let t = (local + half) / max(in.geom.zw, vec2<f32>(1e-6));
            let uv = mix(in.uv.xy, in.uv.zw, t);
            let texel = textureSampleLevel(tex, samp, uv, 0.0);
            if (in.kind == KIND_GRAIN) {
                // A grain is worn, not stamped: the box keeps its own
                // edge and its own ramp, and the grain eats into what
                // that edge covers.
                rgba = vec4<f32>(in.color.rgb, in.color.a * texel.a);
            } else {
                rgba = texel * in.color;
            }
        }
    }
    // The paper the nib is dragged over, which is the canvas's and not
    // the nib's: it is sampled by where the pixel falls on the board,
    // not by where it falls on the dab, so it stands still while the
    // nib turns over it and two strokes crossing one place meet the
    // same fibres. `w` is how deep it bites, and zero is no paper.
    if (in.weave.w > 0.0) {
        // One tile of it, wrapped: the cell is drawn in half a texel on
        // every side, so the seam samples this paper and not the one
        // beside it on the sheet.
        let tile = fract(in.px * in.weave.x + in.weave.yz);
        let tooth = textureSampleLevel(tex, samp, mix(in.paper.xy, in.paper.zw, tile), 0.0);
        rgba.a = rgba.a * mix(1.0, tooth.a, in.weave.w);
    }
    let ramp = max(in.params.y, 1.0);
    // The prim ramps over its own feather, bent by the nib's profile —
    // an exponent of 1 is the plain ramp everything else is drawn with.
    // The cut always ramps over one pixel and is never bent, so a soft
    // shadow is still cut by a hard edge.
    let edge = pow(clamp(0.5 - d / ramp, 0.0, 1.0), in.falloff);
    let coverage = edge * clamp(0.5 - cut, 0.0, 1.0);
    return vec4<f32>(rgba.rgb, rgba.a * coverage);
}
"#;

pub struct Gfx {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    /// The six ways a prim reaches a target, all from the one shader:
    /// `direct` onto the window (straight alpha over); `composite`
    /// lays an offscreen surface's premultiplied pixels on whatever is
    /// under them; `erase` lays them the other way round, taking their
    /// coverage out of what is there, which is what an eraser does to
    /// its own layer's sheet; `union` onto the scratch, every channel a
    /// max, so a swept stroke's spans cover without adding up; `build`
    /// one over the next, so a stamped stroke's dabs pile up toward its
    /// opacity and a stroke lands on the sheet it is painting; `wipe`
    /// with no blending at all, which is how a box clears a surface.
    direct: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    erase: wgpu::RenderPipeline,
    union: wgpu::RenderPipeline,
    build: wgpu::RenderPipeline,
    wipe: wgpu::RenderPipeline,
    globals_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    /// Layout every texture bind group is built with.
    tex_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// One bind group per slot; slot 0 is the 1x1 white stand-in bound by
    /// the runs that draw no image, so every draw has group 1.
    textures: Vec<wgpu::BindGroup>,
    slots: ImageSlots,
    /// The glyph atlas, once it has been uploaded. It is not a blob, so it
    /// stays out of `slots`: that map is sha256 to texture (§9.3), and a
    /// name that is not a bare hash has no business in it.
    atlas: Option<u32>,
    /// The sheet of brush icons, once it has been uploaded. Built into
    /// the binary and the same at every scale, so unlike the glyph atlas
    /// it is uploaded once and never replaced.
    icons: Option<u32>,
    /// The six illustrated dock tools, on their own small RGBA sheet.
    dock_icons: Option<u32>,
    /// The sheet of nib shapes, on the same terms as the icons.
    shapes: Option<u32>,
    /// The agents' makers' marks, one cell each, for the export dialog.
    agent_logos: Option<u32>,
    /// The export dialog's picture of what is leaving: a slot of its own,
    /// replaced in place each time the picture is taken again.
    picture: Option<u32>,
    /// The window-sized texture a group is composited in, once a frame
    /// has needed one. Rebuilt when the window changes size; like the
    /// atlas, a slot of its own and never an entry in `slots`.
    scratch: Option<Surface>,
    /// The second one, on the same terms: what a raster layer's paint
    /// is built on when one of its strokes rubs the others out.
    sheet: Option<Surface>,
}

/// A window-sized texture a frame composites in.
struct Surface {
    slot: u32,
    view: wgpu::TextureView,
    size: (u32, u32),
}

/// Which of the two a call is about.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Which {
    Scratch,
    Sheet,
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
        let pipeline = |label, fragment, blend| {
            pipeline(&device, &layout, &shader, config.format, label, fragment, blend)
        };
        let direct = pipeline("direct", "fs_main", Some(wgpu::BlendState::ALPHA_BLENDING));
        let composite = pipeline(
            "composite",
            "fs_main",
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        );
        let max = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Max,
        };
        let union = pipeline(
            "union",
            "fs_premul",
            Some(wgpu::BlendState {
                color: max,
                alpha: max,
            }),
        );
        // The scratch holds premultiplied color, so a dab lands on the
        // one under it the way the composite lands on the window.
        let build = pipeline(
            "build",
            "fs_premul",
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        );
        // Coverage out of the destination instead of into it: what is
        // left of the sheet is what the eraser did not cover.
        let out = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let erase = pipeline(
            "erase",
            "fs_main",
            Some(wgpu::BlendState {
                color: out,
                alpha: out,
            }),
        );
        let wipe = pipeline("wipe", "fs_premul", None);

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
            direct,
            composite,
            erase,
            union,
            build,
            wipe,
            globals_buf,
            bind_group,
            tex_bgl,
            sampler,
            textures: vec![blank],
            slots: ImageSlots::new(),
            icons: None,
            dock_icons: None,
            shapes: None,
            agent_logos: None,
            picture: None,
            atlas: None,
            scratch: None,
            sheet: None,
        })
    }

    /// One of the two offscreen surfaces at `size`, made or remade as
    /// needed, and its slot. The window asks for its own size; an export
    /// asks for the picture's, and puts them back afterwards.
    fn ensure_surface(&mut self, which: Which, size: (u32, u32)) -> u32 {
        let held = match which {
            Which::Scratch => self.scratch.as_ref(),
            Which::Sheet => self.sheet.as_ref(),
        };
        // A surface of the right size is already there; one of the
        // wrong size keeps its slot and gives up its texture.
        let held = match held {
            Some(s) if s.size == size => return s.slot,
            Some(s) => Some(s.slot),
            None => None,
        };
        let label = match which {
            Which::Scratch => "scratch",
            Which::Sheet => "sheet",
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let group = bind_group(&self.device, &self.tex_bgl, &self.sampler, &view);
        let slot = match held {
            Some(slot) => {
                self.textures[slot as usize] = group;
                slot
            }
            None => {
                self.textures.push(group);
                (self.textures.len() - 1) as u32
            }
        };
        let surface = Some(Surface { slot, view, size });
        match which {
            Which::Scratch => self.scratch = surface,
            Which::Sheet => self.sheet = surface,
        }
        slot
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

    /// Uploads the glyph atlas and answers its slot. Called again when
    /// the scale factor changes the size the chrome asks for: the new
    /// sheet replaces the old one in place, so moving a window between
    /// displays does not leak a texture per move.
    pub fn upload_atlas(&mut self, bmp: &Bitmap) -> anyhow::Result<u32> {
        let group = upload(
            &self.device,
            &self.queue,
            &self.tex_bgl,
            &self.sampler,
            texture_format(self.config.format),
            bmp,
        )?;
        let slot = match self.atlas {
            Some(slot) => {
                self.textures[slot as usize] = group;
                slot
            }
            None => {
                self.textures.push(group);
                (self.textures.len() - 1) as u32
            }
        };
        self.atlas = Some(slot);
        Ok(slot)
    }

    /// Uploads the brush icon sheet and answers its slot, or the slot it
    /// already has. Like the glyph atlas it is not a blob and stays out
    /// of `slots`.
    pub fn upload_icons(&mut self, bmp: &Bitmap) -> anyhow::Result<u32> {
        if let Some(slot) = self.icons {
            return Ok(slot);
        }
        let slot = self.upload_sheet(bmp)?;
        self.icons = Some(slot);
        Ok(slot)
    }

    /// Uploads the dock's illustrated tool sheet and answers its stable
    /// slot. It stays separate from the generated brush sheet: rebuilding
    /// the imported library must not erase the application's own art.
    pub fn upload_dock_icons(&mut self, bmp: &Bitmap) -> anyhow::Result<u32> {
        if let Some(slot) = self.dock_icons {
            return Ok(slot);
        }
        let slot = self.upload_sheet(bmp)?;
        self.dock_icons = Some(slot);
        Ok(slot)
    }

    /// Uploads the export dialog's picture and answers its slot. A new
    /// picture replaces the last in place, as a rebuilt atlas does, so
    /// opening the dialog over and over does not leak a texture a time.
    pub fn upload_picture(&mut self, bmp: &Bitmap) -> anyhow::Result<u32> {
        let group = upload(
            &self.device,
            &self.queue,
            &self.tex_bgl,
            &self.sampler,
            texture_format(self.config.format),
            bmp,
        )?;
        let slot = match self.picture {
            Some(slot) => {
                self.textures[slot as usize] = group;
                slot
            }
            None => {
                self.textures.push(group);
                (self.textures.len() - 1) as u32
            }
        };
        self.picture = Some(slot);
        Ok(slot)
    }

    /// Uploads the sheet of agent logos and answers its stable slot.
    pub fn upload_agent_logos(&mut self, bmp: &Bitmap) -> anyhow::Result<u32> {
        if let Some(slot) = self.agent_logos {
            return Ok(slot);
        }
        let slot = self.upload_sheet(bmp)?;
        self.agent_logos = Some(slot);
        Ok(slot)
    }

    pub fn upload_shapes(&mut self, bmp: &Bitmap) -> anyhow::Result<u32> {
        if let Some(slot) = self.shapes {
            return Ok(slot);
        }
        let slot = self.upload_sheet(bmp)?;
        self.shapes = Some(slot);
        Ok(slot)
    }

    /// A sheet the binary ships: a slot of its own, uploaded once and
    /// never replaced, unlike the glyph atlas which is rebuilt whenever
    /// the scale factor changes.
    fn upload_sheet(&mut self, bmp: &Bitmap) -> anyhow::Result<u32> {
        let group = upload(
            &self.device,
            &self.queue,
            &self.tex_bgl,
            &self.sampler,
            texture_format(self.config.format),
            bmp,
        )?;
        self.textures.push(group);
        Ok((self.textures.len() - 1) as u32)
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        self.config.width = w.max(1);
        self.config.height = h.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    /// The largest texture this device will make, which is the ceiling
    /// on an export's size.
    pub fn max_dimension(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    /// Renders `frame` into a texture of its own and answers the pixels,
    /// tight RGBA8, `w * h * 4` bytes. The copy out is padded to wgpu's
    /// 256-byte row alignment and unpadded here, so the caller gets rows
    /// it can hand straight to an encoder.
    ///
    /// The texture is the *surface's* format, not RGBA: a pipeline is
    /// built against one colour format and a pass onto any other is a
    /// validation error, so an export drawn through the window's own
    /// pipelines has to be drawn onto what they were made for. The
    /// channels are put in RGBA order here instead, which is where a
    /// PNG wants them.
    pub fn render_offscreen(
        &mut self,
        w: u32,
        h: u32,
        background: Rgba,
        frame: &Frame,
    ) -> anyhow::Result<Vec<u8>> {
        let (w, h) = (w.max(1), h.max(1));
        let size = wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("export"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let unpadded = w * 4;
        let padded = unpadded.div_ceil(align) * align;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("export readback"),
            size: u64::from(padded) * u64::from(h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.encode(&view, Viewport { w, h }, background, frame);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(h),
                },
            },
            size,
        );
        self.queue.submit(Some(encoder.finish()));
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        rx.recv()
            .map_err(|_| anyhow::anyhow!("the export readback never answered"))?
            .map_err(|e| anyhow::anyhow!("mapping the export readback: {e}"))?;
        let padded_bytes = slice
            .get_mapped_range()
            .map_err(|e| anyhow::anyhow!("reading the export readback: {e}"))?;
        let mut out = Vec::with_capacity((unpadded * h) as usize);
        for row in 0..h as usize {
            let at = row * padded as usize;
            out.extend_from_slice(&padded_bytes[at..at + unpadded as usize]);
        }
        drop(padded_bytes);
        buffer.unmap();
        // Every surface this runs on so far is BGRA; RGBA needs nothing.
        if matches!(
            self.config.format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        ) {
            for px in out.as_chunks_mut::<4>().0 {
                px.swap(0, 2);
            }
        }
        // The window's surface is the size it was; the scratch and the
        // sheet have just been resized to the export and must go back,
        // or the next frame composites through a texture of the wrong
        // size.
        let window = (self.config.width, self.config.height);
        self.ensure_surface(Which::Scratch, window);
        self.ensure_surface(Which::Sheet, window);
        Ok(out)
    }

    /// Renders one frame: clear to `background`, then the frame's passes
    /// as [`scene::passes`] plans them — the prims in order, one draw per
    /// texture run, with each group composited through the scratch.
    /// `Ok(false)` = frame skipped (surface occluded or temporarily lost);
    /// the caller may try again later.
    pub fn render(&mut self, background: Rgba, frame: &Frame) -> anyhow::Result<bool> {
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

        let viewport = Viewport {
            w: self.config.width,
            h: self.config.height,
        };
        let encoder = self.encode(&view, viewport, background, frame);
        self.queue.submit([encoder.finish()]);
        self.queue.present(texture);
        Ok(true)
    }

    /// Encodes one frame's passes onto `target`. The only thing the
    /// window and an export do differently is what they draw onto and
    /// how big it is, so this is the whole of the drawing and both
    /// callers give it a view.
    fn encode(
        &mut self,
        target: &wgpu::TextureView,
        viewport: Viewport,
        background: Rgba,
        frame: &Frame,
    ) -> wgpu::CommandEncoder {
        let scratch = self.ensure_surface(Which::Scratch, (viewport.w, viewport.h));
        let sheet = self.ensure_surface(Which::Sheet, (viewport.w, viewport.h));
        let globals: [f32; 4] = [viewport.w as f32, viewport.h as f32, 0.0, 0.0];
        self.queue
            .write_buffer(&self.globals_buf, 0, bytemuck::cast_slice(&globals));

        let (prims, passes) = scene::passes(frame, viewport, scratch, sheet);
        // A buffer per frame is the simplest thing that works; reuse and
        // per-element caching come with real profiling (§3).
        let instance_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("prims"),
                contents: bytemuck::cast_slice(&prims),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let scratch_view = &self.scratch.as_ref().expect("scratch was ensured").view;
        let sheet_view = &self.sheet.as_ref().expect("sheet was ensured").view;
        let [r, g, b, a] = background;
        let clear = wgpu::LoadOp::Clear(wgpu::Color {
            r: f64::from(r),
            g: f64::from(g),
            b: f64::from(b),
            a: f64::from(a),
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        // The window is cleared by the first pass onto it; everything
        // else loads what is already there, since a wipe box is what
        // opens an offscreen surface over the part that is being used.
        let mut cleared = false;
        for pass in passes {
            let (label, target, load) = match pass.onto {
                Onto::Window => {
                    let load = if cleared { wgpu::LoadOp::Load } else { clear };
                    cleared = true;
                    ("window", target, load)
                }
                Onto::Sheet => ("sheet", sheet_view, wgpu::LoadOp::Load),
                Onto::Scratch => ("scratch", scratch_view, wgpu::LoadOp::Load),
            };
            let pipeline = match (pass.onto, pass.blend) {
                // The window takes straight alpha; an offscreen surface
                // holds premultiplied color, so a prim drawn onto one
                // lands the way a composite does.
                (Onto::Window, _) => &self.direct,
                (Onto::Scratch, Blend::Union) => &self.union,
                (Onto::Sheet | Onto::Scratch, _) => &self.build,
            };
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if prims.is_empty() {
                continue;
            }
            rp.set_bind_group(0, &self.bind_group, &[]);
            rp.set_vertex_buffer(0, instance_buf.slice(..));
            if let Some(wipe) = pass.wipe {
                rp.set_pipeline(&self.wipe);
                rp.set_bind_group(1, &self.textures[0], &[]);
                rp.draw(0..6, wipe..wipe + 1);
            }
            if let Some(lay) = pass.lay {
                rp.set_pipeline(match lay.blend {
                    Blend::Erase => &self.erase,
                    _ => &self.composite,
                });
                // The box says which surface it samples.
                let from = prims[lay.prim as usize].slot as usize;
                rp.set_bind_group(1, self.textures.get(from).unwrap_or(&self.textures[0]), &[]);
                rp.draw(0..6, lay.prim..lay.prim + 1);
            }
            let range = pass.start..pass.end;
            if range.is_empty() {
                continue;
            }
            rp.set_pipeline(pipeline);
            for run in scene::runs(&prims[range.start as usize..range.end as usize]) {
                let group = self
                    .textures
                    .get(run.slot as usize)
                    .unwrap_or(&self.textures[0]);
                rp.set_bind_group(1, group, &[]);
                rp.draw(0..6, range.start + run.start..range.start + run.end);
            }
        }
        encoder
    }
}

/// One pipeline over the instanced-quad vertex stage: `fragment` names
/// the entry point, `blend` how its output meets the target (`None` is
/// a plain write).
fn pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    label: &str,
    fragment: &str,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
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
                    6 => Float32x4, // uv
                    7 => Float32x4, // clip
                    8 => Float32,   // falloff
                    9 => Float32x4, // paper
                    10 => Float32x4, // weave
                    // `slot` stays on the CPU: it picks the bind group.
                ],
            })],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
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
    Ok(bind_group(device, layout, sampler, &view))
}

/// The group 1 that samples `view`.
fn bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    view: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("image"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}
