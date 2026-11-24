//! End-to-end test for the Linux layer: presents ~60 frames per second to a
//! headless swap chain through the real Vulkan loader for `secs` seconds.
//! With the layer installed, `uo-probe` must show ≈60 FPS for this process.
//!
//!   cargo run --example present_test -- [secs]

fn main() {
    use ash::{ext, khr, vk};
    use std::time::{Duration, Instant};

    let secs: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    unsafe {
        let entry = ash::Entry::load().expect("libvulkan");
        let exts = [
            khr::surface::NAME.as_ptr(),
            ext::headless_surface::NAME.as_ptr(),
        ];
        let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_1);
        let instance = entry
            .create_instance(
                &vk::InstanceCreateInfo::default()
                    .application_info(&app)
                    .enabled_extension_names(&exts),
                None,
            )
            .expect("instance");
        let headless = ext::headless_surface::Instance::new(&entry, &instance);
        let surface = headless
            .create_headless_surface(&vk::HeadlessSurfaceCreateInfoEXT::default(), None)
            .expect("surface");
        let surf = khr::surface::Instance::new(&entry, &instance);

        // First device + queue family that can present to the surface.
        let (pd, qf) = instance
            .enumerate_physical_devices()
            .unwrap()
            .into_iter()
            .find_map(|pd| {
                let n = instance
                    .get_physical_device_queue_family_properties(pd)
                    .len() as u32;
                (0..n)
                    .find(|&i| {
                        surf.get_physical_device_surface_support(pd, i, surface)
                            .unwrap_or(false)
                    })
                    .map(|i| (pd, i))
            })
            .expect("no device can present to a headless surface");
        let name = instance.get_physical_device_properties(pd);
        println!(
            "device: {:?}",
            std::ffi::CStr::from_ptr(name.device_name.as_ptr())
        );

        let prio = [1.0];
        let qci = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(qf)
            .queue_priorities(&prio)];
        let dext = [khr::swapchain::NAME.as_ptr()];
        let device = instance
            .create_device(
                pd,
                &vk::DeviceCreateInfo::default()
                    .queue_create_infos(&qci)
                    .enabled_extension_names(&dext),
                None,
            )
            .expect("device");
        let queue = device.get_device_queue(qf, 0);

        let caps = surf
            .get_physical_device_surface_capabilities(pd, surface)
            .unwrap();
        let fmt = surf
            .get_physical_device_surface_formats(pd, surface)
            .unwrap()[0];
        let extent = if caps.current_extent.width == u32::MAX {
            vk::Extent2D {
                width: 64,
                height: 64,
            }
        } else {
            caps.current_extent
        };
        let sc = khr::swapchain::Device::new(&instance, &device);
        let swapchain = sc
            .create_swapchain(
                &vk::SwapchainCreateInfoKHR::default()
                    .surface(surface)
                    .min_image_count(caps.min_image_count.max(2))
                    .image_format(fmt.format)
                    .image_color_space(fmt.color_space)
                    .image_extent(extent)
                    .image_array_layers(1)
                    .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
                    .pre_transform(caps.current_transform)
                    .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
                    .present_mode(vk::PresentModeKHR::FIFO)
                    .clipped(true),
                None,
            )
            .expect("swapchain");
        let sem = device
            .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
            .unwrap();

        println!("pid {} presenting ~60 FPS for {secs} s", std::process::id());
        let start = Instant::now();
        let mut next = start;
        let mut frames = 0u64;
        while start.elapsed() < Duration::from_secs(secs) {
            let (idx, _) = sc
                .acquire_next_image(swapchain, u64::MAX, sem, vk::Fence::null())
                .expect("acquire");
            let waits = [sem];
            let chains = [swapchain];
            let idxs = [idx];
            let _ = sc.queue_present(
                queue,
                &vk::PresentInfoKHR::default()
                    .wait_semaphores(&waits)
                    .swapchains(&chains)
                    .image_indices(&idxs),
            );
            device.queue_wait_idle(queue).ok();
            frames += 1;
            next += Duration::from_micros(16_667);
            if let Some(d) = next.checked_duration_since(Instant::now()) {
                std::thread::sleep(d);
            }
        }
        println!("presented {frames} frames");
        device.destroy_semaphore(sem, None);
        sc.destroy_swapchain(swapchain, None);
        device.destroy_device(None);
        surf.destroy_surface(surface, None);
        instance.destroy_instance(None);
    }
}
