//! Render wgpu (ARCHITECTURE.md §7): cena retida na CPU, GPU só rasteriza.
//!
//! MVP do scaffold: um pipeline de quads instanciados — cada `RectInstance`
//! (pos/size em px de tela, cor linear) vira dois triângulos no shader.
//! Sem atlas, sem texto, sem tesselação: isso entra com as ferramentas.

use std::sync::Arc;

use anyhow::Context as _;
use wgpu::util::DeviceExt as _;

use crate::scene::RectInstance;

const SHADER: &str = r#"
struct Globals {
    viewport: vec2<f32>,
    _pad: vec2<f32>,
};
@group(0) @binding(0) var<uniform> globals: Globals;

struct Inst {
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Inst) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let px = inst.pos + corners[vi] * inst.size;
    // px de tela (y para baixo) -> NDC (y para cima).
    let ndc = vec2<f32>(
        px.x / globals.viewport.x * 2.0 - 1.0,
        1.0 - px.y / globals.viewport.y * 2.0,
    );
    var out: VsOut;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.color = inst.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
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
    /// Cor de fundo em RGBA linear; trocada pelo `op: theme`.
    pub background: [f32; 4],
}

impl Gfx {
    pub fn new(window: Arc<winit::window::Window>) -> anyhow::Result<Gfx> {
        // Sem display handle: só o backend GL o usa; Vulkan (nosso alvo,
        // §10.1) ignora. Trocar por OwnedDisplayHandle do winit se GL entrar.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(window.clone())
            .context("criando surface Wayland")?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            // Whiteboard não justifica acordar dGPU.
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .context("nenhum adapter de GPU compatível")?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("omawhite"),
            ..Default::default()
        }))
        .context("criando device wgpu")?;

        let size = window.inner_size();
        let config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .context("surface sem configuração default")?;
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("omawhite-rects"),
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

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("omawhite"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rects"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<RectInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x2, // pos
                        1 => Float32x2, // size
                        2 => Float32x4, // color
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

        Ok(Gfx {
            surface,
            device,
            queue,
            config,
            pipeline,
            globals_buf,
            bind_group,
            background: crate::scene::parse_color("#1a1a1a"),
        })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        self.config.width = w.max(1);
        self.config.height = h.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    /// Renderiza um frame. `Ok(false)` = frame pulado (surface ocluída ou
    /// perdida temporariamente); o chamador pode tentar de novo depois.
    pub fn render(&mut self, instances: &[RectInstance]) -> anyhow::Result<bool> {
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
                anyhow::bail!("erro de validação ao adquirir frame da surface");
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

        // Cena de scaffold é pequena; buffer por frame é o mais simples que
        // funciona. Dirty-rect e reuso entram com a profiling de verdade (§3).
        let instance_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("rect-instances"),
                contents: bytemuck::cast_slice(instances),
                usage: wgpu::BufferUsages::VERTEX,
            });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        {
            let [r, g, b, a] = self.background;
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
            if !instances.is_empty() {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, instance_buf.slice(..));
                pass.draw(0..6, 0..instances.len() as u32);
            }
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(texture);
        Ok(true)
    }
}
