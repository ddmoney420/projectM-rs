//! GPU context: owns the wgpu instance, adapter, device and queue.
//!
//! The C++ renderer relies on a globally-current OpenGL context. wgpu has no
//! global state, so we carry the device/queue explicitly through the renderer.

use std::fmt;

#[derive(Debug)]
pub enum GpuError {
    NoAdapter(String),
    NoDevice(String),
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GpuError::NoAdapter(e) => write!(f, "no suitable GPU adapter: {e}"),
            GpuError::NoDevice(e) => write!(f, "failed to create GPU device: {e}"),
        }
    }
}

impl std::error::Error for GpuError {}

/// Owns the wgpu device/queue used for all rendering.
pub struct GpuContext {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl GpuContext {
    /// Create a headless context (no surface/window). Tries a hardware adapter
    /// first, then falls back to a software adapter (WARP/lavapipe) so it works
    /// in environments without a real GPU — e.g. CI and offscreen rendering.
    pub fn headless() -> Result<Self, GpuError> {
        pollster::block_on(Self::headless_async())
    }

    async fn headless_async() -> Result<Self, GpuError> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());

        let adapter = match instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            })
            .await
        {
            Ok(a) => a,
            Err(_) => instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    force_fallback_adapter: true,
                    compatible_surface: None,
                })
                .await
                .map_err(|e| GpuError::NoAdapter(e.to_string()))?,
        };

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("pm-render device"),
                required_features: wgpu::Features::empty(),
                // Broadly-supported limits so the same code runs on software,
                // mobile-class, and desktop adapters.
                required_limits: wgpu::Limits::downlevel_defaults(),
                ..Default::default()
            })
            .await
            .map_err(|e| GpuError::NoDevice(e.to_string()))?;

        Ok(GpuContext { instance, adapter, device, queue })
    }

    /// Human-readable adapter description (backend + device name).
    pub fn adapter_info(&self) -> String {
        let info = self.adapter.get_info();
        format!("{} ({:?})", info.name, info.backend)
    }
}
