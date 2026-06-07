//! Headless rendering tests. These need a GPU adapter (hardware or a software
//! fallback like WARP/lavapipe). If none is available, the test prints a notice
//! and returns rather than failing, so the suite still runs on GPU-less CI.

use pm_render::{
    clear, read_rgba8, Framebuffer, FullscreenShader, GpuContext, RenderContext, Texture,
    TARGET_FORMAT,
};

fn ctx_or_skip() -> Option<GpuContext> {
    match GpuContext::headless() {
        Ok(c) => {
            eprintln!("using adapter: {}", c.adapter_info());
            Some(c)
        }
        Err(e) => {
            eprintln!("skipping GPU test, no adapter: {e}");
            None
        }
    }
}

#[test]
fn clear_to_red_reads_back_red() {
    let Some(ctx) = ctx_or_skip() else { return };

    let target = Texture::new_render_target(&ctx.device, "main", 8, 8, TARGET_FORMAT);
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    clear(&mut enc, &target.view, wgpu::Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 });
    ctx.queue.submit(Some(enc.finish()));

    let pixels = read_rgba8(&ctx, &target);
    assert_eq!(pixels.len(), 8 * 8 * 4);
    // Every pixel must be opaque red.
    for px in pixels.chunks_exact(4) {
        assert_eq!(px, [255, 0, 0, 255], "expected red");
    }
}

#[test]
fn fullscreen_shader_fills_green() {
    let Some(ctx) = ctx_or_skip() else { return };

    let frag = r#"
        @fragment
        fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
            return vec4<f32>(0.0, 1.0, 0.0, 1.0);
        }
    "#;
    let shader = FullscreenShader::new(&ctx.device, TARGET_FORMAT, frag);
    let target = Texture::new_render_target(&ctx.device, "main", 16, 16, TARGET_FORMAT);

    let mut enc = ctx.device.create_command_encoder(&Default::default());
    shader.render(&mut enc, &target.view, Some(wgpu::Color::BLACK));
    ctx.queue.submit(Some(enc.finish()));

    let pixels = read_rgba8(&ctx, &target);
    for px in pixels.chunks_exact(4) {
        assert_eq!(px, [0, 255, 0, 255], "expected green");
    }
}

#[test]
fn fullscreen_shader_uv_gradient() {
    // A horizontal red gradient: left edge ~0, right edge ~255. Verifies the
    // built-in fullscreen vertex stage produces a sane UV.
    let Some(ctx) = ctx_or_skip() else { return };

    let frag = r#"
        @fragment
        fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
            return vec4<f32>(uv.x, 0.0, 0.0, 1.0);
        }
    "#;
    let shader = FullscreenShader::new(&ctx.device, TARGET_FORMAT, frag);
    let w = 64u32;
    let target = Texture::new_render_target(&ctx.device, "main", w, 4, TARGET_FORMAT);

    let mut enc = ctx.device.create_command_encoder(&Default::default());
    shader.render(&mut enc, &target.view, None);
    ctx.queue.submit(Some(enc.finish()));

    let pixels = read_rgba8(&ctx, &target);
    // Read the red channel of the first row, left vs right.
    let left = pixels[0];
    let right = pixels[((w - 1) * 4) as usize];
    assert!(left < 16, "left edge should be near 0, got {left}");
    assert!(right > 239, "right edge should be near 255, got {right}");
    assert!(right > left);
}

#[test]
fn framebuffer_targets_are_independent() {
    let Some(ctx) = ctx_or_skip() else { return };

    let fb = Framebuffer::new(&ctx.device, 2, 8, 8);
    assert_eq!(fb.count(), 2);

    // Clear buffer 0 red, buffer 1 blue; they must not bleed into each other.
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    clear(&mut enc, fb.view(0), wgpu::Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 });
    clear(&mut enc, fb.view(1), wgpu::Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 });
    ctx.queue.submit(Some(enc.finish()));

    let b0 = read_rgba8(&ctx, fb.texture(0));
    let b1 = read_rgba8(&ctx, fb.texture(1));
    assert_eq!(&b0[0..4], &[255, 0, 0, 255]);
    assert_eq!(&b1[0..4], &[0, 0, 255, 255]);
}

#[test]
fn framebuffer_resize() {
    let Some(ctx) = ctx_or_skip() else { return };

    let mut fb = Framebuffer::new(&ctx.device, 1, 16, 16);
    assert_eq!((fb.width(), fb.height()), (16, 16));
    // Same size -> no change.
    assert!(!fb.set_size(&ctx.device, 16, 16));
    // New size -> changed.
    assert!(fb.set_size(&ctx.device, 32, 24));
    assert_eq!((fb.width(), fb.height()), (32, 24));
    // Degenerate -> no change.
    assert!(!fb.set_size(&ctx.device, 0, 10));
}

#[test]
fn render_context_aspect_ratio() {
    // Pure-CPU test, no GPU needed.
    let mut rc = RenderContext::default();
    rc.set_viewport(1920, 1080);
    assert!((rc.aspect_x - 1920.0 / 1080.0).abs() < 1e-5);
    assert_eq!(rc.aspect_y, 1.0);
    assert!((rc.inv_aspect_x - 1080.0 / 1920.0).abs() < 1e-5);

    rc.set_viewport(1080, 1920);
    assert_eq!(rc.aspect_x, 1.0);
    assert!((rc.aspect_y - 1920.0 / 1080.0).abs() < 1e-5);
}
