//! Every graphics adapter this computer offers, what the Graphics driver setting makes of each, and
//! whether it really draws: a device is opened and a pixel cleared and read back. Without a window,
//! so OpenGL may list fewer adapters than the game sees.
//!
//! `cargo run -p horizon-client-render --example gpu_adapters`
use horizon_client_render::{
    gfx::{is_mesa, is_software, pick_adapter},
    graphics::GraphicsDriver,
};

const GREEN: wgpu::Color = wgpu::Color::GREEN;

/// Clears one pixel green on `adapter` and reads it back.
fn draws(adapter: &wgpu::Adapter) -> Result<[u8; 4], String> {
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))
        .map_err(|e| e.to_string())?;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    let view = texture.create_view(&Default::default());
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(GREEN),
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(1),
            },
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |_| {});
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| e.to_string())?;
    let pixel = buffer.get_mapped_range(..4).map_err(|e| e.to_string())?;
    Ok([pixel[0], pixel[1], pixel[2], pixel[3]])
}

fn main() {
    for (name, backends) in [
        ("Vulkan", wgpu::Backends::VULKAN),
        ("OpenGL", wgpu::Backends::GL),
    ] {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = backends;
        let instance = wgpu::Instance::new(descriptor);
        let adapters = pollster::block_on(instance.enumerate_adapters(backends));
        let infos: Vec<wgpu::AdapterInfo> = adapters.iter().map(wgpu::Adapter::get_info).collect();
        println!("{name}: {} adapter(s)", adapters.len());
        for (adapter, info) in adapters.iter().zip(&infos) {
            let family = if is_software(info) {
                "software"
            } else if is_mesa(info) {
                "Mesa"
            } else {
                "vendor"
            };
            let drew = match draws(adapter) {
                Ok([0, 255, 0, 255]) => "draws".to_owned(),
                Ok(pixel) => format!("WRONG PIXEL {pixel:?}"),
                Err(e) => format!("FAILED: {e}"),
            };
            println!(
                "  {} [{family}, {:?}] {} {}: {drew}",
                info.name, info.device_type, info.driver, info.driver_info
            );
        }
        for driver in [GraphicsDriver::Mesa, GraphicsDriver::Software] {
            let pick = pick_adapter(driver, &infos).map(|i| infos[i].name.as_str());
            println!("  {} picks {}", driver.label(), pick.unwrap_or("nothing"));
        }
    }
}
