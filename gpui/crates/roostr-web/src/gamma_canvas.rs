//! Gamma-space blending on Rune's WebGL2 fallback, so a browser without
//! WebGPU shows the same colours.
//!
//! Rune blends in sRGB (gamma) space, as CSS does (`App::srgb_blend`, on by
//! default): it encodes premultiplied colours itself and draws them into a
//! non-sRGB view of the canvas. WebGL2 cannot view a texture in another
//! format, and wgpu lists the canvas's `Rgba8UnormSrgb` first, so Rune
//! (8e3fc8e, `platform/web.rs`) keeps that format there and turns its
//! renderer's gamma-space blending off, but leaves the runtime's on. Colours
//! the runtime encoded are then encoded again by the sRGB target: every fill
//! and glyph washes out while images, which the renderer encodes, stay right.
//!
//! On that path [`GammaCanvas`] draws the frame as on WebGPU, gamma-space
//! blended into a non-sRGB texture, then copies it to the canvas decoded to
//! linear light, which the sRGB target encodes back to the same bytes. With
//! WebGPU the target is already the non-sRGB view and it draws Rune's
//! default frame.

use rune::compose::DeviceKey;
use rune::{Compositor, Frame};

/// Where the frame draws before the copy: plain 8-bit, holding the
/// sRGB-encoded values a WebGPU canvas would get.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// One full-screen triangle writing each texel of `frame`, decoded to linear
/// light, to the same pixel of the target.
const SHADER: &str = r"
@group(0) @binding(0) var frame: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureLoad(frame, vec2<i32>(pos.xy), 0);
    let v = c.rgb;
    let rgb = select(pow((v + 0.055) / 1.055, vec3<f32>(2.4)), v / 12.92, v <= vec3<f32>(0.04045));
    return vec4<f32>(rgb, c.a);
}
";

/// The compositor: Rune's default frame, or on WebGL2 the gamma-space frame
/// and its copy to the canvas.
#[derive(Default)]
pub struct GammaCanvas {
    blit: Option<Blit>,
}

/// The copy's GPU objects, for one device and target format.
struct Blit {
    device: DeviceKey,
    target: wgpu::TextureFormat,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    frame: Option<Target>,
}

/// The texture the frame draws into, at the canvas's size.
struct Target {
    size: (u32, u32),
    view: wgpu::TextureView,
    bind: wgpu::BindGroup,
}

impl Blit {
    fn new(device: &wgpu::Device, key: DeviceKey, target: wgpu::TextureFormat) -> Blit {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gamma canvas"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gamma canvas"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gamma canvas"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gamma canvas"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Blit {
            device: key,
            target,
            layout,
            pipeline,
            frame: None,
        }
    }
}

impl Target {
    /// The frame texture in `slot` at `size`, made again when the canvas
    /// resized.
    fn at<'a>(
        slot: &'a mut Option<Target>,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        size: (u32, u32),
    ) -> &'a Target {
        if slot.as_ref().is_some_and(|f| f.size == size) {
            return slot.as_ref().expect("checked above");
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gamma canvas frame"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gamma canvas frame"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        slot.insert(Target { size, view, bind })
    }
}

impl<S: 'static> Compositor<S> for GammaCanvas {
    fn render(&mut self, f: &mut Frame<'_, S>) {
        // WebGPU, or a runtime blending in linear light: the target matches.
        if !(f.format.is_srgb() && f.rt.srgb_blend()) {
            f.draw_ui();
            return;
        }
        let (key, target) = (f.device_key(), f.format);
        if self
            .blit
            .as_ref()
            .is_none_or(|b| b.device != key || b.target != target)
        {
            self.blit = Some(Blit::new(f.device, key, target));
        }
        let blit = self.blit.as_mut().expect("made above");
        let frame = Target::at(&mut blit.frame, f.device, &blit.layout, f.size);
        // The renderer blends in gamma space for a non-sRGB target. Rune
        // makes the next frame's canvas view in the renderer's format, so it
        // gets the canvas's back once this frame is recorded.
        f.ui.set_format(f.device, FORMAT);
        let clear = f.rt.clear_color();
        f.ui.prepare(
            f.device,
            f.queue,
            &mut f.rt.text.atlas,
            &f.rt.instances,
            f.size,
        );
        f.ui.draw(f.encoder, &frame.view, clear);
        f.ui.set_format(f.device, target);
        let mut pass = f.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gamma canvas copy"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: f.target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&blit.pipeline);
        pass.set_bind_group(0, &frame.bind, &[]);
        pass.draw(0..3, 0..1);
    }
}
