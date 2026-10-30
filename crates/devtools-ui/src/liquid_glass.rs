//! 栏的玻璃效果，参照 PasteNow：内容滚到栏下面时，整块栏都像一层磨砂玻璃，
//! 下面的图标、文字变成柔和的、更鲜亮的色块，像光映在玻璃上；只在分界处有很短的一段由清晰过渡到模糊。
//!
//! 系统的 `NSGlassEffectView` 只能放在 egui 画面的下面，透不出 egui 画的卡片，所以这里自己做：
//! 1. 每帧画完列表后，借 eframe 的 egui 渲染器把列表（连同窗口底色）另外渲染到一张离屏纹理；
//! 2. 把它缩到一半大小，做两轮横、纵分开的高斯模糊，得到一张平滑的模糊图；
//! 3. 栏的背景是一个绘制回调：读模糊图，提高饱和度与亮度，再压一层薄薄的底色，
//!    栏里的控件画在它上面。
//!
//! 只在真正的窗口里可用（需要 eframe 的 wgpu 渲染器）；离屏测试里退回 `glass` 的模拟效果。

use std::num::NonZeroU64;

use eframe::egui_wgpu::{self, wgpu, CallbackResources, CallbackTrait, ScreenDescriptor};
use egui::epaint::ClippedShape;
use egui::{Color32, Context, Rect, Rgba, Shape};

use crate::glass::{Edge, Source};

/// 分界处由清晰过渡到模糊的距离（点）。
const RAMP: f32 = 8.0;
/// 模糊图相对窗口的缩小倍数，以及高斯模糊的轮数（每轮横、纵各一次）。
/// 模糊得不重（约 4 点）：PasteNow 的玻璃下还能看出文字笔画的光。
const DOWNSCALE: u32 = 2;
const BLUR_ROUNDS: usize = 2;

/// 液态玻璃效果：持有 eframe 的 wgpu 渲染状态，管线和纹理放在渲染器的回调资源里。
pub struct LiquidGlass {
    state: egui_wgpu::RenderState,
}

/// 离屏纹理：窗口大小的清晰图，缩小的模糊图与中间结果。
struct Targets {
    size: [u32; 2],
    crisp: wgpu::TextureView,
    blurred: wgpu::TextureView,
    /// 模糊的各步：降采样（清晰 → 模糊图），然后每轮横向（模糊图 → 中间）、纵向（中间 → 模糊图）。
    down: wgpu::BindGroup,
    horizontal: wgpu::BindGroup,
    vertical: wgpu::BindGroup,
    temp: wgpu::TextureView,
}

/// 放在渲染器的 `callback_resources` 里：管线、离屏纹理与每个栏的参数缓冲。
struct Resources {
    glass: wgpu::RenderPipeline,
    glass_layout: wgpu::BindGroupLayout,
    down: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
    blur_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// 降采样、横向、纵向三步的参数。
    blur_uniforms: [wgpu::Buffer; 3],
    targets: Option<Targets>,
    /// 每个栏一份参数缓冲与绑定组（栏用 `slot` 区分）。
    slots: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
}

/// 栏的着色器参数，布局与 WGSL 里的 `Glass` 一致。
#[repr(C)]
#[derive(Clone, Copy)]
struct Uniforms {
    /// 栏的范围（物理像素）：min.x, min.y, max.x, max.y。
    rect: [f32; 4],
    /// 边（1 = 顶部，-1 = 底部）、窗口圆角、过渡距离（物理像素）、保留。
    params: [f32; 4],
    /// 压在模糊内容上的底色（预乘，与帧缓冲同一色彩空间）。
    tint: [f32; 4],
    /// 屏幕宽高（物理像素）、保留。
    screen: [f32; 4],
}

/// 栏参数缓冲的大小。
const UNIFORM_SIZE: u64 = std::mem::size_of::<Uniforms>() as u64;
/// 模糊步骤的参数：方向（目标纹理的像素）与源、目标纹理的大小。
const BLUR_UNIFORM_SIZE: u64 = 32;
/// 同一帧里最多几个栏（面板有顶部栏、底部栏，设置页有标题栏）。
const SLOTS: usize = 4;

/// 把参数结构按内存布局转成字节，写进 uniform 缓冲。
fn bytes<T: Copy>(value: &T) -> &[u8] {
    // SAFETY: 只用于由 f32 组成、没有填充字节的 repr(C) 结构与数组。
    unsafe {
        std::slice::from_raw_parts((value as *const T).cast::<u8>(), std::mem::size_of::<T>())
    }
}

/// 绑定组布局中的纹理、uniform 缓冲与采样器项。
fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn uniform_entry(binding: u32, size: u64) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: NonZeroU64::new(size),
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

/// 全屏三角形的渲染管线，片元着色器是 `entry`。
fn pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::BindGroupLayout,
    entry: &str,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("liquid_glass"),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("liquid_glass"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(entry),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// 清空并绘制到 `view` 的渲染通道。
fn color_pass<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    view: &'a wgpu::TextureView,
) -> wgpu::RenderPass<'a> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("liquid_glass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
            depth_slice: None,
        })],
        depth_stencil_attachment: None,
        occlusion_query_set: None,
        timestamp_writes: None,
        multiview_mask: None,
    })
}

impl LiquidGlass {
    /// 编译着色器、创建管线，放进渲染器的回调资源。
    pub fn new(state: &egui_wgpu::RenderState) -> Self {
        let device = &state.device;
        let format = state.target_format;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("liquid_glass"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let glass_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("liquid_glass"),
            entries: &[
                uniform_entry(0, UNIFORM_SIZE),
                texture_entry(1),
                texture_entry(2),
                sampler_entry(3),
            ],
        });
        let blur_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("liquid_glass_blur"),
            entries: &[
                uniform_entry(0, BLUR_UNIFORM_SIZE),
                texture_entry(1),
                sampler_entry(2),
            ],
        });
        let glass = pipeline(
            device,
            &module,
            &glass_layout,
            "fs_glass",
            format,
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        );
        let down = pipeline(device, &module, &blur_layout, "fs_down", format, None);
        let blur = pipeline(device, &module, &blur_layout, "fs_blur", format, None);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("liquid_glass"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let blur_uniforms = std::array::from_fn(|_| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("liquid_glass_blur"),
                size: BLUR_UNIFORM_SIZE,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        state.renderer.write().callback_resources.insert(Resources {
            glass,
            glass_layout,
            down,
            blur,
            blur_layout,
            sampler,
            blur_uniforms,
            targets: None,
            slots: Vec::new(),
        });
        Self {
            state: state.clone(),
        }
    }

    /// 把 `source`（列表画下的图形）连同窗口底色 `background` 渲染到离屏纹理并做好模糊图，
    /// 供这一帧的栏读取。列表画完、栏画之前调用。
    pub fn capture(
        &self,
        ctx: &Context,
        source: Option<&Source>,
        window: Rect,
        background: Color32,
    ) {
        let pixels_per_point = ctx.pixels_per_point();
        let screen = ctx.viewport_rect();
        let size = [
            (screen.width() * pixels_per_point).round().max(1.0) as u32,
            (screen.height() * pixels_per_point).round().max(1.0) as u32,
        ];
        // 列表在栏下面的部分被裁掉了，这里换成整个窗口的裁剪区域重新画。
        let mut shapes = vec![ClippedShape {
            clip_rect: screen,
            shape: Shape::rect_filled(window, 0, background),
        }];
        ctx.graphics(|graphics| {
            if let Some((list, source)) =
                source.and_then(|source| Some((graphics.get(source.layer)?, source)))
            {
                shapes.extend(
                    list.all_entries()
                        .skip(source.shapes.start)
                        .take(source.shapes.len())
                        .filter(|clipped| !matches!(clipped.shape, Shape::Callback(_)))
                        .map(|clipped| ClippedShape {
                            clip_rect: screen,
                            shape: clipped.shape.clone(),
                        }),
                );
            }
        });
        let primitives = ctx.tessellate(shapes, pixels_per_point);
        let descriptor = ScreenDescriptor {
            size_in_pixels: size,
            pixels_per_point,
        };

        let device = &self.state.device;
        let queue = &self.state.queue;
        let mut renderer = self.state.renderer.write();
        // 刚启动的几帧里字体等纹理还没上传到渲染器（eframe 在这一帧画完后才上传），
        // 用到它们的图形这一帧先不画，免得渲染器报找不到纹理。
        let primitives: Vec<_> = primitives
            .into_iter()
            .filter(|primitive| match &primitive.primitive {
                egui::epaint::Primitive::Mesh(mesh) => renderer.texture(&mesh.texture_id).is_some(),
                egui::epaint::Primitive::Callback(_) => false,
            })
            .collect();
        let small = size.map(|side| side.div_ceil(DOWNSCALE));
        {
            let resources: &mut Resources = renderer.callback_resources.get_mut().unwrap();
            if resources.targets.as_ref().is_none_or(|t| t.size != size) {
                resources.targets = Some(self.targets(resources, size, small));
                // 纹理换了，栏的绑定组都要重建。
                resources.slots.clear();
            }
            // 降采样，然后是横向、纵向的模糊。
            // 与 WGSL 的 `Blur` 一致：方向、填充、源大小、目标大小。
            let (big, small) = (size.map(|v| v as f32), small.map(|v| v as f32));
            let blur_params = [
                [0.0, 0.0, 0.0, 0.0, big[0], big[1], small[0], small[1]],
                [1.0, 0.0, 0.0, 0.0, small[0], small[1], small[0], small[1]],
                [0.0, 1.0, 0.0, 0.0, small[0], small[1], small[0], small[1]],
            ];
            for (buffer, params) in resources.blur_uniforms.iter().zip(&blur_params) {
                queue.write_buffer(buffer, 0, bytes(params));
            }
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("liquid_glass_backdrop"),
        });
        // 与 eframe 共用渲染器的顶点缓冲：这里先提交，eframe 稍后写入自己的数据时，
        // wgpu 保证这次提交读到的是这里写入的内容。
        let extra = renderer.update_buffers(device, queue, &mut encoder, &primitives, &descriptor);
        let resources: &Resources = renderer.callback_resources.get().unwrap();
        let targets = resources.targets.as_ref().unwrap();
        {
            let mut pass = color_pass(&mut encoder, &targets.crisp).forget_lifetime();
            renderer.render(&mut pass, &primitives, &descriptor);
        }
        let mut step = |pipeline: &wgpu::RenderPipeline,
                        bind_group: &wgpu::BindGroup,
                        target: &wgpu::TextureView| {
            let mut pass = color_pass(&mut encoder, target);
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind_group, &[]);
            pass.draw(0..3, 0..1);
        };
        step(&resources.down, &targets.down, &targets.blurred);
        for _ in 0..BLUR_ROUNDS {
            step(&resources.blur, &targets.horizontal, &targets.temp);
            step(&resources.blur, &targets.vertical, &targets.blurred);
        }
        queue.submit(extra.into_iter().chain(std::iter::once(encoder.finish())));
    }

    /// 按窗口大小创建离屏纹理与模糊各步的绑定组（窗口大小变化时重建）。
    fn targets(&self, resources: &Resources, size: [u32; 2], small: [u32; 2]) -> Targets {
        let device = &self.state.device;
        let view = |size: [u32; 2]| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("liquid_glass_backdrop"),
                    size: wgpu::Extent3d {
                        width: size[0],
                        height: size[1],
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: self.state.target_format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let crisp = view(size);
        let blurred = view(small);
        let temp = view(small);
        let bind = |uniforms: &wgpu::Buffer, input: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("liquid_glass_blur"),
                layout: &resources.blur_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniforms.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(input),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&resources.sampler),
                    },
                ],
            })
        };
        Targets {
            size,
            down: bind(&resources.blur_uniforms[0], &crisp),
            horizontal: bind(&resources.blur_uniforms[1], &blurred),
            vertical: bind(&resources.blur_uniforms[2], &temp),
            crisp,
            blurred,
            temp,
        }
    }

    /// 栏的玻璃背景：一个绘制回调，画在栏的控件下面。`slot` 区分同一帧里的不同栏，
    /// `tint` 是压在模糊内容上的底色。
    pub fn shape(
        &self,
        ctx: &Context,
        slot: usize,
        rect: Rect,
        edge: Edge,
        corner_radius: f32,
        tint: Color32,
    ) -> Shape {
        let ppp = ctx.pixels_per_point();
        let screen = ctx.viewport_rect();
        let tint = if self.state.target_format.is_srgb() {
            let linear = Rgba::from(tint);
            [linear.r(), linear.g(), linear.b(), linear.a()]
        } else {
            tint.to_normalized_gamma_f32()
        };
        let ramp = RAMP * ppp;
        let uniforms = Uniforms {
            rect: [
                rect.min.x * ppp,
                rect.min.y * ppp,
                rect.max.x * ppp,
                rect.max.y * ppp,
            ],
            params: [
                if edge == Edge::Top { 1.0 } else { -1.0 },
                corner_radius * ppp,
                ramp,
                0.0,
            ],
            tint,
            screen: [screen.width() * ppp, screen.height() * ppp, 0.0, 0.0],
        };
        Shape::Callback(egui_wgpu::Callback::new_paint_callback(
            rect,
            GlassCallback {
                slot: slot.min(SLOTS - 1),
                uniforms,
            },
        ))
    }
}

/// 栏的绘制回调：写入这一栏的参数，然后在栏的范围内画玻璃。
struct GlassCallback {
    slot: usize,
    uniforms: Uniforms,
}

impl CallbackTrait for GlassCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(resources) = resources.get_mut::<Resources>() else {
            return Vec::new();
        };
        let Some(targets) = resources.targets.as_ref() else {
            return Vec::new();
        };
        while resources.slots.len() <= self.slot {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("liquid_glass"),
                size: UNIFORM_SIZE,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("liquid_glass"),
                layout: &resources.glass_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&targets.crisp),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&targets.blurred),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&resources.sampler),
                    },
                ],
            });
            resources.slots.push((buffer, bind_group));
        }
        queue.write_buffer(&resources.slots[self.slot].0, 0, bytes(&self.uniforms));
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let Some(resources) = resources.get::<Resources>() else {
            return;
        };
        let Some((_, bind_group)) = resources.slots.get(self.slot) else {
            return;
        };
        render_pass.set_pipeline(&resources.glass);
        render_pass.set_bind_group(0, bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }
}

const SHADER: &str = r#"
// 覆盖整个视口的三角形（栏的回调里视口是栏的范围，模糊步骤里是整张目标纹理）。
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

// ---------------------------------------------------------------- 模糊

struct Blur {
    // 方向（目标纹理的像素）。
    dir: vec2<f32>,
    _pad0: vec2<f32>,
    // 源、目标纹理的大小。
    src: vec2<f32>,
    dst: vec2<f32>,
};

@group(0) @binding(0) var<uniform> blur: Blur;
@group(0) @binding(1) var blur_input: texture_2d<f32>;
@group(0) @binding(2) var blur_sampler: sampler;

// 降采样：每个小像素取周围几个大像素的平均（四次双线性采样）。
@fragment
fn fs_down(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = position.xy / blur.dst;
    let texel = 1.0 / blur.src;
    var color = vec4<f32>(0.0);
    color += textureSampleLevel(blur_input, blur_sampler, uv + vec2<f32>(-1.0, -1.0) * texel, 0.0);
    color += textureSampleLevel(blur_input, blur_sampler, uv + vec2<f32>(1.0, -1.0) * texel, 0.0);
    color += textureSampleLevel(blur_input, blur_sampler, uv + vec2<f32>(-1.0, 1.0) * texel, 0.0);
    color += textureSampleLevel(blur_input, blur_sampler, uv + vec2<f32>(1.0, 1.0) * texel, 0.0);
    return color * 0.25;
}

// 一维高斯模糊（σ = 3 个小像素，13 个采样点）。
@fragment
fn fs_blur(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = position.xy / blur.dst;
    let step = blur.dir / blur.src;
    var color = vec4<f32>(0.0);
    var total = 0.0;
    for (var i = -6; i <= 6; i = i + 1) {
        let x = f32(i);
        let weight = exp(-x * x / 18.0);
        color += textureSampleLevel(blur_input, blur_sampler, uv + step * x, 0.0) * weight;
        total += weight;
    }
    return color / total;
}

// ---------------------------------------------------------------- 栏

struct Glass {
    rect: vec4<f32>,
    params: vec4<f32>,
    tint: vec4<f32>,
    screen: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Glass;
@group(0) @binding(1) var crisp: texture_2d<f32>;
@group(0) @binding(2) var blurred: texture_2d<f32>;
@group(0) @binding(3) var glass_sampler: sampler;

@fragment
fn fs_glass(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let p = position.xy;
    let r = g.rect;
    let top = g.params.x > 0.0;
    let radius = g.params.y;
    let ramp = g.params.z;

    // 到朝向内容那条边（分界）的距离，越往栏里（越靠窗口边）越大。
    var d: f32;
    if top { d = r.w - p.y; } else { d = p.y - r.y; }
    let progress = smoothstep(0.0, ramp, max(d, 0.0));

    let uv = p / g.screen.xy;
    let sharp = textureSampleLevel(crisp, glass_sampler, uv, 0.0);
    let soft = textureSampleLevel(blurred, glass_sampler, uv, 0.0);
    // 磨砂玻璃下的颜色更鲜亮：提高饱和度和一点亮度，像光透过玻璃。
    let alpha = max(soft.a, 0.0001);
    var rgb = soft.rgb / alpha;
    let luma = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    rgb = clamp(mix(vec3<f32>(luma), rgb, 1.7) * 1.3, vec3<f32>(0.0), vec3<f32>(1.0));
    var frosted = vec4<f32>(rgb * soft.a, soft.a);
    // 薄薄的底色，栏里的控件看得清。
    frosted = g.tint + frosted * (1.0 - g.tint.a);
    var color = mix(sharp, frosted, progress);

    // 窗口的圆角：贴着窗口边的两个角外面不画。
    var mask = 1.0;
    let cx = clamp(p.x, r.x + radius, r.z - radius);
    var cy: f32;
    if top { cy = r.y + radius; } else { cy = r.w - radius; }
    if (top && p.y < cy) || (!top && p.y > cy) {
        mask = clamp(radius - length(p - vec2<f32>(cx, cy)) + 0.5, 0.0, 1.0);
    }
    return color * mask;
}
"#;
