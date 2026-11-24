//! `wasm32-unknown-unknown` browser entrypoint attaching `window.cs16Web` without `#[wasm_bindgen]`
//! proc-macros so the entire crate builds under `#![forbid(unsafe_code)]`.

use std::cell::RefCell;
use std::rc::Rc;

use cs_assets::bsp::BspMap;
use cs_assets::scheme::{SkyboxTextures, VguiColors};
use cs_client::render::lighting::{GpuLightsUniform, TextureFilterMode};
use cs_client::render::mdl_renderer::{StudioInstance, viewmodel_transform};
use cs_client::render::{RenderSettings, Renderer, SceneDynamicData};
use cs_client::ui::hud::HudSprites;
use cs_client::ui::vgui_assets::VguiAssetBank;
use cs_client::ui::vgui_theme::{VguiPalette, apply_vgui2_theme};
use cs_platform::{Clock, WebClock, WebLocalStorage};
use glam::Vec3;
use js_sys::{Array, Float32Array, Object, Reflect, Uint8Array};
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::future_to_promise;
use web_sys::HtmlCanvasElement;

use crate::state::{MapSyncAction, WebAudioEvent, WebClientState};

struct WebRuntime {
    state: WebClientState,
    renderer: Option<Renderer>,
    egui_ctx: egui::Context,
    hud_sprites: Option<HudSprites>,
    vgui_bank: Option<VguiAssetBank>,
    vgui_palette: VguiPalette,
    clock: WebClock,
    last_ms: Option<f64>,
}

impl WebRuntime {
    fn new() -> Self {
        let egui_ctx = egui::Context::default();
        let vgui_palette = VguiPalette::from(VguiColors::default());
        apply_vgui2_theme(&egui_ctx, vgui_palette);
        Self {
            state: WebClientState::new(Box::new(WebLocalStorage)),
            renderer: None,
            egui_ctx,
            hud_sprites: None,
            vgui_bank: None,
            vgui_palette,
            clock: WebClock::new(),
            last_ms: None,
        }
    }
}

/// Registers the `window.cs16Web` API for `web/app.js`.
pub fn install() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let rt = Rc::new(RefCell::new(WebRuntime::new()));
    let api = Object::new();

    // init(canvas) -> Promise<void>
    {
        let rt_c = Rc::clone(&rt);
        let closure: Closure<dyn FnMut(JsValue) -> JsValue> = Closure::new(move |canvas_val: JsValue| {
            let rt_inner = Rc::clone(&rt_c);
            let promise = future_to_promise(async move {
                let canvas: HtmlCanvasElement = canvas_val
                    .dyn_into()
                    .map_err(|_| JsValue::from_str("expected HTMLCanvasElement"))?;
                let width = canvas.width().max(1);
                let height = canvas.height().max(1);

                let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
                desc.backends = wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL;
                let instance = wgpu::Instance::new(desc);
                let surface = instance
                    .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
                    .map_err(|e| JsValue::from_str(&format!("create_surface: {e}")))?;

                let renderer = Renderer::new_from_surface(&instance, surface, width, height)
                    .await
                    .map_err(|e| JsValue::from_str(&e))?;
                rt_inner.borrow_mut().renderer = Some(renderer);
                Ok(JsValue::UNDEFINED)
            });
            promise.into()
        });
        let _ = Reflect::set(&api, &JsValue::from_str("init"), &closure.into_js_value());
    }

    // rendererInfo() -> String
    {
        let rt_c = Rc::clone(&rt);
        let closure: Closure<dyn FnMut() -> String> = Closure::new(move || {
            let g = rt_c.borrow();
            match &g.renderer {
                Some(r) => format!("{} ({})", r.backend_name, r.adapter_name),
                None => "uninitialized".to_string(),
            }
        });
        let _ = Reflect::set(&api, &JsValue::from_str("rendererInfo"), &closure.into_js_value());
    }

    // mountFile(path, bytes)
    {
        let rt_c = Rc::clone(&rt);
        let closure: Closure<dyn FnMut(String, Uint8Array)> =
            Closure::new(move |path: String, bytes: Uint8Array| {
                rt_c.borrow_mut().state.mount_file(path, bytes.to_vec());
            });
        let _ = Reflect::set(&api, &JsValue::from_str("mountFile"), &closure.into_js_value());
    }

    // finishMount()
    {
        let rt_c = Rc::clone(&rt);
        let closure: Closure<dyn FnMut()> = Closure::new(move || {
            let mut g = rt_c.borrow_mut();
            g.state.finish_mount();
            let vfs = g.state.vfs.clone();
            if let Some(ref v) = vfs {
                let colors = VguiColors::load(v);
                g.vgui_palette = VguiPalette::from(colors);
                apply_vgui2_theme(&g.egui_ctx, g.vgui_palette);
                g.hud_sprites = Some(HudSprites::load(&g.egui_ctx, v));
                g.vgui_bank = Some(VguiAssetBank::load(&g.egui_ctx, Some(v)));
            }
        });
        let _ = Reflect::set(&api, &JsValue::from_str("finishMount"), &closure.into_js_value());
    }

    // listMaps() -> Array<String>
    {
        let rt_c = Rc::clone(&rt);
        let closure: Closure<dyn FnMut() -> Array> = Closure::new(move || {
            let maps = rt_c.borrow().state.list_maps();
            let arr = Array::new();
            for m in maps {
                arr.push(&JsValue::from_str(&m));
            }
            arr
        });
        let _ = Reflect::set(&api, &JsValue::from_str("listMaps"), &closure.into_js_value());
    }

    // readFile(path) -> Uint8Array | undefined
    {
        let rt_c = Rc::clone(&rt);
        let closure: Closure<dyn FnMut(String) -> JsValue> = Closure::new(move |path: String| {
            match rt_c.borrow().state.read_file(&path) {
                Some(bytes) => Uint8Array::from(bytes.as_slice()).into(),
                None => JsValue::UNDEFINED,
            }
        });
        let _ = Reflect::set(&api, &JsValue::from_str("readFile"), &closure.into_js_value());
    }

    // startLocal(map, bots)
    {
        let rt_c = Rc::clone(&rt);
        let closure: Closure<dyn FnMut(String, u32)> = Closure::new(move |map: String, bots: u32| {
            if let Err(err) = rt_c.borrow_mut().state.start_local(&map, bots) {
                web_sys::console::error_1(&JsValue::from_str(&format!("startLocal failed: {err}")));
            }
        });
        let _ = Reflect::set(&api, &JsValue::from_str("startLocal"), &closure.into_js_value());
    }

    // connectRemote()
    {
        let rt_c = Rc::clone(&rt);
        let closure: Closure<dyn FnMut()> = Closure::new(move || {
            let _ = rt_c.borrow_mut().state.connect_remote();
        });
        let _ = Reflect::set(&api, &JsValue::from_str("connectRemote"), &closure.into_js_value());
    }

    // disconnect()
    {
        let rt_c = Rc::clone(&rt);
        let closure: Closure<dyn FnMut()> = Closure::new(move || {
            rt_c.borrow_mut().state.disconnect();
        });
        let _ = Reflect::set(&api, &JsValue::from_str("disconnect"), &closure.into_js_value());
    }

    // pushNetPacket(Uint8Array)
    {
        let rt_c = Rc::clone(&rt);
        let closure: Closure<dyn FnMut(Uint8Array)> = Closure::new(move |bytes: Uint8Array| {
            rt_c.borrow_mut().state.push_net_packet(bytes.to_vec());
        });
        let _ = Reflect::set(&api, &JsValue::from_str("pushNetPacket"), &closure.into_js_value());
    }

    // takeNetPackets() -> Array<Uint8Array>
    {
        let rt_c = Rc::clone(&rt);
        let closure: Closure<dyn FnMut() -> Array> = Closure::new(move || {
            let pkts = rt_c.borrow_mut().state.take_net_packets();
            let arr = Array::new();
            for p in pkts {
                arr.push(&Uint8Array::from(p.as_slice()));
            }
            arr
        });
        let _ = Reflect::set(&api, &JsValue::from_str("takeNetPackets"), &closure.into_js_value());
    }

    // takeAudioEvents() -> Array<Object>
    {
        let rt_c = Rc::clone(&rt);
        let closure: Closure<dyn FnMut() -> Array> = Closure::new(move || {
            let evs = rt_c.borrow_mut().state.take_audio_events();
            let arr = Array::new();
            for ev in evs {
                arr.push(&audio_event_to_js(ev));
            }
            arr
        });
        let _ = Reflect::set(&api, &JsValue::from_str("takeAudioEvents"), &closure.into_js_value());
    }

    // unlockAudio()
    {
        let rt_c = Rc::clone(&rt);
        let closure: Closure<dyn FnMut()> = Closure::new(move || {
            rt_c.borrow_mut().state.unlock_audio();
        });
        let _ = Reflect::set(&api, &JsValue::from_str("unlockAudio"), &closure.into_js_value());
    }

    // pushVoiceMicPcm(samples: Float32Array, sampleRate: number)
    {
        let rt_c = Rc::clone(&rt);
        let closure: Closure<dyn FnMut(Float32Array, f64)> =
            Closure::new(move |samples: Float32Array, sample_rate: f64| {
                let pcm = samples.to_vec();
                let rate = sample_rate.round().clamp(8_000.0, 96_000.0) as u32;
                rt_c.borrow_mut().state.push_voice_mic_pcm(&pcm, rate);
            });
        let _ = Reflect::set(
            &api,
            &JsValue::from_str("pushVoiceMicPcm"),
            &closure.into_js_value(),
        );
    }

    // onKey(code, pressed)
    {
        let rt_c = Rc::clone(&rt);
        let closure: Closure<dyn FnMut(String, bool)> = Closure::new(move |code: String, pressed: bool| {
            rt_c.borrow_mut().state.input.on_key(&code, pressed);
        });
        let _ = Reflect::set(&api, &JsValue::from_str("onKey"), &closure.into_js_value());
    }

    // onMouseMove(dx, dy)
    {
        let rt_c = Rc::clone(&rt);
        let closure: Closure<dyn FnMut(f64, f64)> = Closure::new(move |dx: f64, dy: f64| {
            let mut g = rt_c.borrow_mut();
            let sens = g.state.cvars.get_f32("sensitivity").clamp(0.1, 20.0);
            let mut cam = g.state.camera.clone();
            g.state.input.on_mouse_move(&mut cam, dx as f32, dy as f32, sens);
            g.state.camera = cam;
        });
        let _ = Reflect::set(&api, &JsValue::from_str("onMouseMove"), &closure.into_js_value());
    }

    // onMouseButton(button, pressed)
    {
        let rt_c = Rc::clone(&rt);
        let closure: Closure<dyn FnMut(u32, bool)> = Closure::new(move |button: u32, pressed: bool| {
            rt_c.borrow_mut().state.input.on_mouse_button(button, pressed);
        });
        let _ = Reflect::set(&api, &JsValue::from_str("onMouseButton"), &closure.into_js_value());
    }

    // onGamepad(lx, ly, rx, ry, mask)
    {
        let rt_c = Rc::clone(&rt);
        let closure: Closure<dyn FnMut(f64, f64, f64, f64, u32)> =
            Closure::new(move |lx: f64, ly: f64, rx: f64, ry: f64, mask: u32| {
                rt_c.borrow_mut()
                    .state
                    .input
                    .on_gamepad(lx as f32, ly as f32, rx as f32, ry as f32, mask);
            });
        let _ = Reflect::set(&api, &JsValue::from_str("onGamepad"), &closure.into_js_value());
    }

    // onTouch(lx, ly, dx, dy, mask)
    {
        let rt_c = Rc::clone(&rt);
        let closure: Closure<dyn FnMut(f64, f64, f64, f64, u32)> =
            Closure::new(move |lx: f64, ly: f64, dx: f64, dy: f64, mask: u32| {
                let mut g = rt_c.borrow_mut();
                let sens = g.state.cvars.get_f32("sensitivity").clamp(0.1, 20.0);
                let mut cam = g.state.camera.clone();
                g.state.input.on_touch(
                    &mut cam,
                    lx as f32,
                    ly as f32,
                    dx as f32,
                    dy as f32,
                    mask,
                    sens,
                );
                g.state.camera = cam;
            });
        let _ = Reflect::set(&api, &JsValue::from_str("onTouch"), &closure.into_js_value());
    }

    // resize(w, h)
    {
        let rt_c = Rc::clone(&rt);
        let closure: Closure<dyn FnMut(u32, u32)> = Closure::new(move |w: u32, h: u32| {
            if let Some(renderer) = &mut rt_c.borrow_mut().renderer {
                renderer.resize(w, h);
            }
        });
        let _ = Reflect::set(&api, &JsValue::from_str("resize"), &closure.into_js_value());
    }

    // execConsole(line)
    {
        let rt_c = Rc::clone(&rt);
        let closure: Closure<dyn FnMut(String)> = Closure::new(move |line: String| {
            rt_c.borrow_mut().state.exec_console(&line);
        });
        let _ = Reflect::set(&api, &JsValue::from_str("execConsole"), &closure.into_js_value());
    }

    // stepAndRender(now_ms)
    {
        let rt_c = Rc::clone(&rt);
        let closure: Closure<dyn FnMut(f64)> = Closure::new(move |now_ms: f64| {
            step_and_render(&rt_c, now_ms);
        });
        let _ = Reflect::set(&api, &JsValue::from_str("stepAndRender"), &closure.into_js_value());
    }

    let _ = Reflect::set(&window, &JsValue::from_str("cs16Web"), &api);
}

fn step_and_render(rt_cell: &Rc<RefCell<WebRuntime>>, now_ms: f64) {
    let mut g = rt_cell.borrow_mut();
    let sample_ms = if now_ms.is_finite() && now_ms > 0.0 {
        now_ms
    } else {
        g.clock.now() * 1000.0
    };
    let dt = match g.last_ms {
        Some(prev) => ((sample_ms - prev) / 1000.0).clamp(0.0, 0.1),
        None => 0.016,
    };
    g.last_ms = Some(sample_ms);

    let out = g.state.step(dt);

    match out.map_sync {
        MapSyncAction::None => {}
        MapSyncAction::Unload => {
            if let Some(r) = &mut g.renderer {
                r.unload_map();
                r.clear_model_caches();
            }
        }
        MapSyncAction::Load(map_name) => {
            g.state.mark_map_loaded(&map_name);
            let vfs_opt = g.state.vfs.clone();
            if let Some(vfs) = vfs_opt
                && let Ok(bsp) = BspMap::load(&vfs, &map_name)
            {
                let sky = SkyboxTextures::load(&vfs, &bsp.skyname);
                let sprites = HudSprites::load(&g.egui_ctx, &vfs);
                g.hud_sprites = Some(sprites);
                if let Some(r) = &mut g.renderer {
                    r.clear_model_caches();
                    r.load_map(&bsp, &sky);
                }
            }
        }
    }

    if let (Some(reg), Some(r)) = (out.new_registry, &mut g.renderer) {
        r.set_model_registry(reg);
    }

    let width = g.renderer.as_ref().map_or(1280, |r| r.config.width.max(1)) as f32;
    let height = g.renderer.as_ref().map_or(720, |r| r.config.height.max(1)) as f32;
    let mut raw_input = egui::RawInput::default();
    raw_input.screen_rect = Some(egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::Vec2::new(width, height),
    ));
    g.egui_ctx.begin_pass(raw_input);
    let view_ref = out.view.as_ref();
    let client_pal = g
        .vgui_bank
        .as_ref()
        .map_or_else(
            || cs_client::ui::vgui_assets::ClientPalette::from(cs_assets::scheme::ClientVguiColors::default()),
            |b| b.client_palette,
        );
    g.state
        .draw_hud_ui(&g.egui_ctx, g.hud_sprites.as_ref(), client_pal, g.vgui_palette, view_ref);
    let full_output = g.egui_ctx.end_pass();
    let paint_jobs = g
        .egui_ctx
        .tessellate(full_output.shapes, full_output.pixels_per_point);

    let mut cam = g.state.camera.clone();
    let mut fov = g.state.cvars.get_f32("default_fov").clamp(60.0, 120.0);
    let mut viewmodel_inst: Option<(String, glam::Mat4, i32, i32, f32, bool)> = None;
    let mut beams = Vec::new();
    let mut points = Vec::new();
    let mut decals = Vec::new();
    let mut custom_decals = std::collections::HashMap::new();
    let mut lightstyles = [1.0f32; 64];
    let mut lights = GpuLightsUniform::default();
    let mut elights = Vec::new();
    let mut fog = None;
    let mut scene = None;

    if let Some(v) = out.view {
        cam.position.z += v.bob + v.shake_z;
        cam.pitch_deg = (cam.pitch_deg + v.punch.x + v.shake_angles.x).clamp(-89.0, 89.0);
        cam.yaw_deg = (cam.yaw_deg + v.punch.y + v.shake_angles.y).rem_euclid(360.0);
        cam.roll_deg = (v.punch.z + v.roll_deg + v.shake_angles.z).clamp(-90.0, 90.0);
        fov = (v.fov + v.underwater_fov_delta).clamp(10.0, 120.0);
        if let Some(vm) = v.viewmodel {
            let bob_off = Vec3::new(0.0, 0.0, v.bob * 0.4) + v.viewmodel_lag;
            let xf = viewmodel_transform(
                v.eye,
                cam.yaw_deg,
                cam.pitch_deg,
                bob_off,
                vm.right_hand,
            );
            viewmodel_inst = Some((vm.path, xf, vm.sequence, vm.body, vm.animtime, vm.right_hand));
        }
        beams = v.beams;
        points = v.points;
        decals = v.decals;
        custom_decals = v.custom_decals;
        lightstyles = v.lightstyles;
        lights = v.lights;
        elights = v.elights;
        fog = v.fog;
        scene = Some(v.scene);
    }

    let gamma = g.state.cvars.get_f32("gamma").clamp(1.8, 3.0);
    let texgamma = g.state.cvars.get_f32("texgamma").clamp(1.8, 3.0);
    let lightgamma = g.state.cvars.get_f32("lightgamma").clamp(1.8, 3.0);
    let brightness = g.state.cvars.get_f32("brightness").clamp(0.0, 3.0);
    let debug_mode = if g.state.cvars.get_bool("r_lightmap")
        || g.state.cvars.get_bool("r_lightmap_only")
    {
        1.0
    } else if g.state.cvars.get_bool("r_fullbright") {
        2.0
    } else {
        0.0
    };
    let time_sec = (sample_ms / 1000.0) as f32;
    let settings = RenderSettings {
        gamma,
        texgamma,
        lightgamma,
        brightness,
        gl_overbright: g.state.cvars.get_bool("gl_overbright"),
        direct: g.state.cvars.get_f32("direct").clamp(0.75, 1.0),
        gl_picmip: g.state.cvars.get_i32("gl_picmip").clamp(0, 4),
        gl_max_size: g.state.cvars.get_i32("gl_max_size").clamp(64, 4096) as u32,
        gl_round_down: g.state.cvars.get_i32("gl_round_down").clamp(0, 3),
        fov_deg: fov,
        viewmodel_fov: g.state.cvars.get_f32("viewmodel_fov").clamp(50.0, 120.0),
        aspect_mode: "widescreen",
        texture_mode: TextureFilterMode::parse(g.state.cvars.get_str("gl_texturemode")),
        anisotropy: g.state.cvars.get_i32("gl_ansio").clamp(1, 16) as u16,
        msaa_samples: if g.state.cvars.get_i32("gl_msaa") >= 4 { 4 } else { 1 },
        software_mode: g.state.cvars.get_bool("r_software_mode"),
        r_vis: g.state.cvars.get_bool("r_vis"),
        r_decals: g.state.cvars.get_i32("r_decals").max(0) as usize,
        wave_height: 0.0,
        debug_mode,
        time_sec,
        fog,
    };

    let WebRuntime {
        state, renderer, ..
    } = &mut *g;
    if let Some(r) = renderer {
        r.set_entity_scene(scene);
        let vm_ref = viewmodel_inst
            .as_ref()
            .map(|(path, xf, seq, body, animtime, right_hand)| StudioInstance {
                model_path: path.as_str(),
                transform: *xf,
                sequence: *seq,
                frame: 0.0,
                animtime: *animtime,
                framerate: 1.0,
                now: time_sec,
                body: *body,
                light_rgb: Vec3::splat(0.95),
                flip_winding: *right_hand,
                attached_model: None,
                ..StudioInstance::default()
            });
        let dynamic = SceneDynamicData {
            vfs: state.vfs.as_ref(),
            viewmodel: vm_ref,
            beams: &beams,
            points: &points,
            decals: &decals,
            custom_decals: Some(&custom_decals),
            lightstyles: &lightstyles,
            lights,
            elights: &elights,
        };
        let _ = r.render_frame(
            &cam,
            &settings,
            &dynamic,
            full_output.textures_delta,
            &paint_jobs,
            full_output.pixels_per_point,
            None,
        );
    }
}

fn set_prop(obj: &Object, key: &str, val: JsValue) {
    let _ = Reflect::set(obj, &JsValue::from_str(key), &val);
}

fn audio_event_to_js(ev: WebAudioEvent) -> JsValue {
    let obj = Object::new();
    match ev {
        WebAudioEvent::Play2d {
            path,
            volume,
            pitch,
            entity,
            channel,
            loop_start,
            loop_end,
            delay_sec,
        } => {
            set_prop(&obj, "kind", JsValue::from_str("play_2d"));
            set_prop(&obj, "path", JsValue::from_str(&path));
            set_prop(&obj, "volume", JsValue::from_f64(f64::from(volume)));
            set_prop(&obj, "pitch", JsValue::from_f64(f64::from(pitch)));
            set_prop(&obj, "entity", JsValue::from_f64(f64::from(entity)));
            set_prop(&obj, "channel", JsValue::from_f64(f64::from(channel)));
            set_prop(&obj, "delay_sec", JsValue::from_f64(f64::from(delay_sec)));
            if let Some(s) = loop_start {
                set_prop(&obj, "loop_start", JsValue::from_f64(f64::from(s)));
            }
            if let Some(e) = loop_end {
                set_prop(&obj, "loop_end", JsValue::from_f64(f64::from(e)));
            }
        }
        WebAudioEvent::Play3d {
            path,
            volume,
            pitch,
            attenuation,
            pos,
            listener,
            right,
            occluded,
            entity,
            channel,
            loop_start,
            loop_end,
            delay_sec,
        } => {
            set_prop(&obj, "kind", JsValue::from_str("play_3d"));
            set_prop(&obj, "path", JsValue::from_str(&path));
            set_prop(&obj, "volume", JsValue::from_f64(f64::from(volume)));
            set_prop(&obj, "pitch", JsValue::from_f64(f64::from(pitch)));
            set_prop(&obj, "attenuation", JsValue::from_f64(f64::from(attenuation)));
            set_prop(&obj, "pos", vec3_arr(pos).into());
            set_prop(&obj, "listener", vec3_arr(listener).into());
            set_prop(&obj, "right", vec3_arr(right).into());
            set_prop(&obj, "occluded", JsValue::from_bool(occluded));
            set_prop(&obj, "entity", JsValue::from_f64(f64::from(entity)));
            set_prop(&obj, "channel", JsValue::from_f64(f64::from(channel)));
            set_prop(&obj, "delay_sec", JsValue::from_f64(f64::from(delay_sec)));
            if let Some(s) = loop_start {
                set_prop(&obj, "loop_start", JsValue::from_f64(f64::from(s)));
            }
            if let Some(e) = loop_end {
                set_prop(&obj, "loop_end", JsValue::from_f64(f64::from(e)));
            }
        }
        WebAudioEvent::UpdateChannel {
            entity,
            channel,
            volume,
            pitch,
            pos,
        } => {
            set_prop(&obj, "kind", JsValue::from_str("update_channel"));
            set_prop(&obj, "entity", JsValue::from_f64(f64::from(entity)));
            set_prop(&obj, "channel", JsValue::from_f64(f64::from(channel)));
            if let Some(v) = volume {
                set_prop(&obj, "volume", JsValue::from_f64(f64::from(v)));
            }
            if let Some(p) = pitch {
                set_prop(&obj, "pitch", JsValue::from_f64(f64::from(p)));
            }
            if let Some(p) = pos {
                set_prop(&obj, "pos", vec3_arr(p).into());
            }
        }
        WebAudioEvent::StopChannel { entity, channel } => {
            set_prop(&obj, "kind", JsValue::from_str("stop_channel"));
            set_prop(&obj, "entity", JsValue::from_f64(f64::from(entity)));
            set_prop(&obj, "channel", JsValue::from_f64(f64::from(channel)));
        }
        WebAudioEvent::StopAll => {
            set_prop(&obj, "kind", JsValue::from_str("stop_all"));
        }
        WebAudioEvent::SetRoomType(rt) => {
            set_prop(&obj, "kind", JsValue::from_str("set_room_type"));
            set_prop(&obj, "room_type", JsValue::from_f64(f64::from(rt)));
        }
        WebAudioEvent::SetUnderwater(underwater) => {
            set_prop(&obj, "kind", JsValue::from_str("set_underwater"));
            set_prop(&obj, "underwater", JsValue::from_bool(underwater));
        }
        WebAudioEvent::SetFade { gain } => {
            set_prop(&obj, "kind", JsValue::from_str("set_fade"));
            set_prop(&obj, "gain", JsValue::from_f64(f64::from(gain)));
        }
        WebAudioEvent::PlayMusic {
            path,
            looping,
            volume,
        } => {
            set_prop(&obj, "kind", JsValue::from_str("play_music"));
            set_prop(&obj, "path", JsValue::from_str(&path));
            set_prop(&obj, "looping", JsValue::from_bool(looping));
            set_prop(&obj, "volume", JsValue::from_f64(f64::from(volume)));
        }
        WebAudioEvent::SetMusicVolume(vol) => {
            set_prop(&obj, "kind", JsValue::from_str("set_music_volume"));
            set_prop(&obj, "volume", JsValue::from_f64(f64::from(vol)));
        }
        WebAudioEvent::StopMusic => {
            set_prop(&obj, "kind", JsValue::from_str("stop_music"));
        }
        WebAudioEvent::VoicePcm {
            speaker,
            sample_rate,
            samples,
            volume,
        } => {
            set_prop(&obj, "kind", JsValue::from_str("voice_pcm"));
            set_prop(&obj, "speaker", JsValue::from_f64(f64::from(speaker)));
            set_prop(&obj, "sample_rate", JsValue::from_f64(f64::from(sample_rate)));
            set_prop(&obj, "volume", JsValue::from_f64(f64::from(volume)));
            let f32arr = Float32Array::from(samples.as_slice());
            set_prop(&obj, "samples", f32arr.into());
        }
    }
    obj.into()
}

fn vec3_arr(v: [f32; 3]) -> Array {
    let arr = Array::new();
    arr.push(&JsValue::from_f64(f64::from(v[0])));
    arr.push(&JsValue::from_f64(f64::from(v[1])));
    arr.push(&JsValue::from_f64(f64::from(v[2])));
    arr
}
