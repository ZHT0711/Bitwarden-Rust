use super::*;
use windows::{
    Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_POPUP},
    core::w,
};

struct TestWindow(HWND);

impl TestWindow {
    fn new() -> Self {
        Self(unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("Bytewarden isolated renderer regression"),
                WS_POPUP,
                0,
                0,
                640,
                480,
                None,
                None,
                None,
                None,
            )
            .unwrap()
        })
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.0).unwrap() };
    }
}

fn path_scene() -> Scene {
    let mut builder = PathBuilder::fill();
    builder.move_to(point(px(10.), px(10.)));
    builder.line_to(point(px(80.), px(10.)));
    builder.line_to(point(px(80.), px(80.)));
    builder.line_to(point(px(10.), px(80.)));
    builder.close();
    let mut path = builder.build().unwrap();
    path.color = rgb(0xff0000).into();
    path.content_mask = ContentMask {
        bounds: Bounds::new(point(px(0.), px(0.)), size(px(640.), px(480.))),
    };
    let mut scene = Scene::default();
    scene.insert_primitive(path.scale(1.));
    scene.finish();
    assert_eq!(scene.paths.len(), 1);
    scene
}

fn assert_red_pixel(renderer: &mut DirectXRenderer, scene: &Scene) {
    // Present flips to the next back buffer. Read the production path passes
    // before Present so the sample comes from the freshly painted target.
    renderer.pre_draw().unwrap();
    renderer.draw_paths_to_intermediate(&scene.paths).unwrap();
    renderer.draw_paths_from_intermediate(&scene.paths).unwrap();
    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        renderer.resources.render_target.GetDesc(&mut desc);
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        let mut staging = None;
        renderer
            .devices
            .device
            .CreateTexture2D(&desc, None, Some(&mut staging))
            .unwrap();
        let staging = staging.unwrap();
        let context = &renderer.devices.device_context;
        context.CopyResource(&staging, &*renderer.resources.render_target);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .unwrap();
        let pixel = std::slice::from_raw_parts(
            mapped
                .pData
                .cast::<u8>()
                .add(20 * mapped.RowPitch as usize + 20 * 4),
            4,
        );
        let bgra = [pixel[0], pixel[1], pixel[2], pixel[3]];
        context.Unmap(&staging, 0);
        assert!(
            bgra[2] > 240 && bgra[1] < 10 && bgra[0] < 10 && bgra[3] > 240,
            "{bgra:?}"
        );
    }
}

#[::core::prelude::v1::test]
fn paths_allocate_on_first_use_release_and_recover_after_resize() {
    let window = TestWindow::new();
    let devices = DirectXDevices::new().unwrap();
    let mut renderer = DirectXRenderer::new(window.0, &devices, true).unwrap();
    assert!(renderer.resources.path_resources.is_none());
    renderer
        .resize(size(DevicePixels(200), DevicePixels(100)))
        .unwrap();
    renderer.draw(&Scene::default()).unwrap();
    assert!(renderer.resources.path_resources.is_none());

    let scene = path_scene();
    renderer.draw(&scene).unwrap();
    assert!(renderer.resources.path_resources.is_some());
    assert_red_pixel(&mut renderer, &scene);
    let texture = renderer
        .resources
        .path_resources
        .as_ref()
        .unwrap()
        .texture
        .clone();
    renderer.draw(&scene).unwrap();
    assert_eq!(
        renderer.resources.path_resources.as_ref().unwrap().texture,
        texture
    );
    drop(texture);

    renderer
        .resize(size(DevicePixels(420), DevicePixels(260)))
        .unwrap();
    assert!(renderer.resources.path_resources.is_none());
    renderer.draw(&scene).unwrap();
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe {
        renderer
            .resources
            .path_resources
            .as_ref()
            .unwrap()
            .texture
            .GetDesc(&mut desc)
    };
    assert_eq!((desc.Width, desc.Height), (420, 260));
    assert_red_pixel(&mut renderer, &scene);

    renderer.draw(&Scene::default()).unwrap();
    assert!(renderer.resources.path_resources.is_none());
    renderer.draw(&scene).unwrap();
    assert_red_pixel(&mut renderer, &scene);
    renderer.handle_device_lost_impl(&devices).unwrap();
    assert!(renderer.resources.path_resources.is_none());
    renderer.draw(&scene).unwrap();
    assert_red_pixel(&mut renderer, &scene);
    renderer.draw(&Scene::default()).unwrap();
    assert!(renderer.resources.path_resources.is_none());
}
