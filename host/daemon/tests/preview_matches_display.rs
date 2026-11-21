//! `DisplayPreview` output is compared pixel for pixel with the display binary for image-backed
//! configurations. Additional tests exercise video frames, preview HTTP endpoints, and worker cleanup.
//! The worker tests run small Python 3 stand-ins and need `python3` on `PATH`.

use openfluxd::api::build_router;
use openfluxd::api::state::{ApiState, NoopDeviceSink};
use openfluxd::preview::DisplayPreview;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Opens a preview barrier when dropped, so a failing assertion never leaves a render blocked.
struct OpenOnDrop(Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>);

impl Drop for OpenOnDrop {
    fn drop(&mut self) {
        let (m, cv) = &*self.0;
        *m.lock().unwrap_or_else(|e| e.into_inner()) = true;
        cv.notify_all();
    }
}

#[cfg(unix)]
fn build_display_binary() -> PathBuf {
    static BUILT_BIN: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BUILT_BIN
        .get_or_init(|| {
            let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .canonicalize()
                .unwrap();
            let target_root = std::env::var("CARGO_TARGET_DIR")
                .ok()
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| repo.join("target"));
            let status = Command::new(env!("CARGO"))
                .current_dir(&repo)
                .args([
                    "build",
                    "-p",
                    "openflux-display",
                    "--bin",
                    "openflux-display",
                ])
                .status()
                .expect("build openflux-display");
            assert!(status.success(), "failed to build openflux-display");
            target_root.join("debug/openflux-display")
        })
        .clone()
}

#[cfg(unix)]
#[test]
fn host_preview_matches_the_display_binary_pixel_for_pixel() {
    let tmp = std::env::temp_dir().join(format!("openfluxd_preview_match_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let cfg_dir = tmp.join("config");
    let media_dir = tmp.join("media");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::create_dir_all(&media_dir).unwrap();

    let display_bin = build_display_binary();

    // A fixed PPM background, a stylemap with several layers and a Mosaic configuration.
    let mut bg_ppm = b"P6\n32 18\n255\n".to_vec();
    for _ in 0..(32 * 18) {
        bg_ppm.extend_from_slice(&[30, 90, 180]);
    }
    std::fs::write(media_dir.join("bg.ppm"), &bg_ppm).unwrap();

    let app_json = serde_json::json!({
        "selectedId": "default",
        "defaultTheme": {
            "id": "default",
            "name": "PreviewTheme",
            "backgroundImagePath": "bg.ppm",
            "backgroundImageBlurAmount": 1.0,
            "backgroundImageDimming": 15.0,
            "moduleBackgroundDimming": 45.0,
            "selectedKeyEffect": "ripple",
            "ringGlowColor": 0xFF66CCFFu32,
            "legendColor": 0xFFFFFFFFu32,
            "stylemaps": {
                "1": { "styles": { "1": { "style": 1, "label": "FN_F1", "legendColor": 0xFF00FFAAu32 } } }
            }
        },
        "customThemes": []
    }).to_string();

    let settings_json = serde_json::json!({
        "parallaxActive": true,
        "eyeHeight": 425.0,
        "eyeDistanceFromFront": 225.0,
        "footStandMode": "low",
        "displayOffsetX": 5.0,
        "displayOffsetY": -3.0
    })
    .to_string();

    let keymap_json = serde_json::json!({
        "selectedFrameLayoutType": "ansi",
        "keymaps": {
            "ansi": {
                "defaultLayer": 0,
                "layers": [
                    { "name": "Base", "keyActions": [{ "keyCode": "KC_ESCAPE", "type": 0 }, { "keyCode": "KC_F1", "type": 0 }] },
                    { "name": "FN", "keyActions": [{ "keyCode": "KC_TRANSPARENT", "type": 0 }, { "keyCode": "KC_BRIGHTNESS_DOWN", "type": 0 }] }
                ]
            }
        }
    }).to_string();

    let mosaic_batches: [[u32; 4]; 5] = [
        [0, 2, 3, 4],
        [5, 6, 7, 8],
        [9, 10, 11, 12],
        [13, 14, 15, 16],
        [17, 18, 12, 15],
    ];

    let live_lines = [
        serde_json::json!({"type":"key_state","seq":7,"active_layer_bitfield":2,"key_bitmap":"0300000000000000000000000000000000000000000000000000000000000000"}).to_string(),
        serde_json::json!({"hardwareData":{"cpu":55,"gpuUsage":30,"memory":12,"totalMemory":32}}).to_string(),
        serde_json::json!({"weather":{"city":"Tokyo","temp":21,"condition":"Rain","ready":true}}).to_string(),
        serde_json::json!({"mediaFile":{"title":"Track","artists":"Artist","length":100,"played":40,"playing":true}}).to_string(),
        serde_json::json!({"customGraphData":{"index":1,"type":0,"title":"FPS","value":"60","progress":0.6}}).to_string(),
    ].join("\n") + "\n";

    std::fs::write(cfg_dir.join("appearance.json"), &app_json).unwrap();
    std::fs::write(cfg_dir.join("settings.json"), &settings_json).unwrap();
    std::fs::write(cfg_dir.join("keymap.json"), &keymap_json).unwrap();
    let live_file = tmp.join("live.jsonl");
    std::fs::write(&live_file, &live_lines).unwrap();

    for (bidx, batch) in mosaic_batches.iter().enumerate() {
        let mosaic_json = serde_json::json!({
            "tileTypes": batch,
            "tileConfigs": [
                { "city": "Tokyo", "utcOffsetMinutes": 540, "metric": "cpu" },
                { "title": "BUILD", "value": "OK", "subtitle": "MAIN", "progress": 0.8 },
                { "petName": "Mochi", "celcius": true },
                { "main": "15", "shortBreak": "5", "longBreak": "15" }
            ]
        })
        .to_string();
        std::fs::write(cfg_dir.join("mosaic.json"), &mosaic_json).unwrap();

        let display_ppm_path = tmp.join(format!("display_{bidx}.ppm"));
        let status = Command::new(&display_bin)
            .args([
                "--config-dir",
                cfg_dir.to_str().unwrap(),
                "--media-dir",
                media_dir.to_str().unwrap(),
                "--live-json",
                live_file.to_str().unwrap(),
                "--render-ppm",
                display_ppm_path.to_str().unwrap(),
            ])
            .env("OPENFLUX_FIXED_TIME_SEC", "1763414400")
            .status()
            .unwrap();
        assert!(status.success());

        let mut preview = DisplayPreview::new(1920, 1080).unwrap();
        assert_eq!(preview.get_key_rects().len(), 85);
        preview
            .set_config_json(
                Some(&app_json),
                Some(&settings_json),
                Some(&keymap_json),
                Some(&mosaic_json),
                Some(media_dir.to_str().unwrap()),
            )
            .unwrap();
        for ln in live_lines.lines() {
            preview.apply_live_json(ln, 0).unwrap();
        }
        let host_ppm = preview.render_ppm_bytes(1_763_414_400, 0, false).unwrap();
        let display_ppm = std::fs::read(&display_ppm_path).unwrap();
        if host_ppm != display_ppm {
            let hdr_end = host_ppm
                .iter()
                .take(32)
                .enumerate()
                .filter(|&(_, &b)| b == b'\n')
                .nth(2)
                .map(|(i, _)| i + 1)
                .unwrap_or(0);
            for (pi, (a, b)) in host_ppm[hdr_end..]
                .chunks_exact(3)
                .zip(display_ppm[hdr_end..].chunks_exact(3))
                .enumerate()
            {
                if a != b {
                    let x = pi % 1920;
                    let y = pi / 1920;
                    panic!(
                        "Batch {bidx} ({batch:?}) mismatch at ({x},{y}) host={a:?} display={b:?}"
                    );
                }
            }
        }
        assert_eq!(
            host_ppm, display_ppm,
            "Batch {bidx} ({batch:?}) host preview differs from the display binary"
        );
    }

    // Video frames submitted to the preview are drawn as the background.
    let mut vid_prev = DisplayPreview::new(16, 8).unwrap();
    let v_app = serde_json::json!({
        "selectedId": "default",
        "defaultTheme": { "id": "default", "name": "V", "backgroundImagePath": "loop.mp4", "backgroundImageDimming": 0.0 }
    }).to_string();
    vid_prev
        .set_config_json(Some(&v_app), None, Some(&keymap_json), None, None)
        .unwrap();
    let green_rgb: Vec<u8> = [20u8, 220, 40].repeat(16 * 8);
    vid_prev
        .submit_video_frame_rgb24(&green_rgb, 16, 8, 0)
        .unwrap();
    let rgba = vid_prev.render_rgba(1_763_414_400, 50, false).unwrap();
    assert_eq!(&rgba[0..4], &[20, 220, 40, 255]);

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn preview_renders_representative_mosaic_tiles_deterministically() {
    let mut prev = DisplayPreview::new(1920, 1080).unwrap();
    assert_eq!(prev.get_key_rects().len(), 85);
    let mos = serde_json::json!({ "tileTypes": [12, 8, 15, 13] }).to_string();
    prev.set_config_json(None, None, None, Some(&mos), None)
        .unwrap();
    prev.apply_live_json(
        r#"{"weather":{"city":"Tokyo","temp":21,"condition":"Rain","ready":true}}"#,
        0,
    )
    .unwrap();
    let ppm1 = prev.render_ppm_bytes(1_763_414_400, 0, false).unwrap();
    let ppm2 = prev.render_ppm_bytes(1_763_414_400, 0, false).unwrap();
    assert_eq!(ppm1, ppm2);
    assert!(ppm1.starts_with(b"P6\n1920 1080\n255\n"));
}

#[tokio::test]
async fn preview_http_endpoints_render_and_unload_cleanly() {
    let tmp = std::env::temp_dir().join(format!("openfluxd_preview_http_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let state = ApiState::with_dirs(
        Arc::new(NoopDeviceSink),
        Some("prev_tok".into()),
        Duration::from_secs(20),
        tmp.join("assets/key_icons"),
    );
    let app = build_router(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    async fn req_bytes(
        addr: std::net::SocketAddr,
        method: &str,
        path: &str,
        body: &str,
    ) -> (u16, Vec<u8>) {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let raw = if body.is_empty() {
            format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer prev_tok\r\nConnection: close\r\n\r\n")
        } else {
            format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer prev_tok\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
        };
        stream.write_all(raw.as_bytes()).await.unwrap();
        let mut resp = Vec::new();
        let _ = stream.read_to_end(&mut resp).await;
        let Some(pos) = resp.windows(4).position(|w| w == b"\r\n\r\n") else {
            return (0, resp);
        };
        let hdr = String::from_utf8_lossy(&resp[..pos]);
        let code: u16 = hdr
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        (code, resp[pos + 4..].to_vec())
    }

    // Before any render, an ISO layout's rects are its 87 keys, and switching to ANSI gives 85
    // straight away.
    let mut iso_km: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/default_keymap.json")).unwrap();
    iso_km["selectedFrameLayoutType"] = serde_json::json!("iso");
    assert_eq!(
        req_bytes(
            addr,
            "PUT",
            "/v1/config",
            &serde_json::json!({"keymap": iso_km}).to_string()
        )
        .await
        .0,
        200
    );
    let (c_iso, b_iso) = req_bytes(addr, "GET", "/v1/preview/rects", "").await;
    assert_eq!(c_iso, 200);
    let r_iso: serde_json::Value = serde_json::from_slice(&b_iso).unwrap();
    assert_eq!(r_iso["rects"].as_array().unwrap().len(), 87);

    let mut ansi_km = iso_km.clone();
    ansi_km["selectedFrameLayoutType"] = serde_json::json!("ansi");
    assert_eq!(
        req_bytes(
            addr,
            "PUT",
            "/v1/config",
            &serde_json::json!({"keymap": ansi_km}).to_string()
        )
        .await
        .0,
        200
    );
    let (c_rects, b_rects) = req_bytes(addr, "GET", "/v1/preview/rects", "").await;
    assert_eq!(c_rects, 200);
    let rv: serde_json::Value = serde_json::from_slice(&b_rects).unwrap();
    assert_eq!(rv["rects"].as_array().unwrap().len(), 85);

    let (c_render, b_ppm) = req_bytes(
        addr,
        "POST",
        "/v1/preview/render",
        r#"{"format":"ppm","nowSec":1763414400}"#,
    )
    .await;
    assert_eq!(c_render, 200);
    assert!(b_ppm.starts_with(b"P6\n1920 1080\n255\n"));
    assert_eq!(b_ppm.len(), b"P6\n1920 1080\n255\n".len() + 1920 * 1080 * 3);
    assert!(state.preview.lock().unwrap().is_some());

    // A render in progress runs off the reactor, so unrelated requests are answered meanwhile.
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    *state.preview_barrier.lock().unwrap() = Some((entered.clone(), release.clone()));
    let open = OpenOnDrop(release.clone());
    let entered_fut = entered.notified();
    let heavy_task = tokio::spawn(async move {
        req_bytes(
            addr,
            "POST",
            "/v1/preview/render",
            r#"{"format":"rgba","nowSec":1763414400}"#,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), entered_fut)
        .await
        .expect("the render starts");
    let (c_chk, _) = tokio::time::timeout(
        Duration::from_millis(250),
        req_bytes(addr, "GET", "/v1/authentication/check", ""),
    )
    .await
    .expect("answered while the render is held");
    assert_eq!(c_chk, 200);
    drop(open);
    assert_eq!(heavy_task.await.unwrap().0, 200);
    *state.preview_barrier.lock().unwrap() = None;

    let (c_cls, _) = req_bytes(addr, "POST", "/v1/preview/close", "{}").await;
    assert_eq!(c_cls, 200);
    assert!(state.preview.lock().unwrap().is_none());

    // An icon imported into a non-default asset store and given to a key shows in the preview.
    let icon_src = tmp.join("bright_key.bmp");
    let mut ibmp = vec![0u8; 54 + 4 * 4 * 3];
    ibmp[0] = b'B';
    ibmp[1] = b'M';
    ibmp[2] = (ibmp.len() as u32).to_le_bytes()[0];
    ibmp[10] = 54;
    ibmp[14] = 40;
    ibmp[18] = 4;
    ibmp[22] = 4;
    ibmp[26] = 1;
    ibmp[28] = 24;
    for px in ibmp[54..].chunks_exact_mut(3) {
        px.copy_from_slice(&[0, 255, 255]);
    } // Bright yellow, in BGR order.
    std::fs::write(&icon_src, &ibmp).unwrap();

    // A render before the key has the icon.
    let (c_pre, b_pre) = req_bytes(
        addr,
        "POST",
        "/v1/preview/render",
        r#"{"format":"rgba","nowSec":1763414400}"#,
    )
    .await;
    assert_eq!(c_pre, 200);
    // Import the icon into tmp/assets/key_icons and give it to key 0 (row 0, index 0).
    let (c_ai, b_ai) = req_bytes(
        addr,
        "POST",
        "/v1/config/addIcon",
        &serde_json::json!({"icons":[{"file": icon_src.to_string_lossy()}]}).to_string(),
    )
    .await;
    assert_eq!(c_ai, 200);
    let icon_id = serde_json::from_slice::<serde_json::Value>(&b_ai).unwrap()["iconPaths"][0]
        .as_str()
        .unwrap()
        .to_string();
    let ks_body = serde_json::json!({
        "styles":[{"profileName":"Default","layer":0,"model":{"width":1,"frameLayoutType":"ansi","row":0,"index":0},"newStyle":{"style":2,"label":"X","iconAssetPath": icon_id}}]
    });
    assert_eq!(
        {
            let mut s = TcpStream::connect(addr).await.unwrap();
            let b = ks_body.to_string();
            s.write_all(format!("PUT /v1/config/addKeystyleToAppearance HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer prev_tok\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}", b.len()).as_bytes()).await.unwrap();
            let mut r = Vec::new();
            s.read_to_end(&mut r).await.unwrap();
            String::from_utf8_lossy(&r)
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .parse::<u16>()
                .unwrap()
        },
        200
    );
    let (c_post, b_post) = req_bytes(
        addr,
        "POST",
        "/v1/preview/render",
        r#"{"format":"rgba","nowSec":1763414400}"#,
    )
    .await;
    assert_eq!(c_post, 200);
    assert_ne!(
        b_pre, b_post,
        "Imported icon from non-default asset root did not affect preview render"
    );

    // The daemon's latest hardware readings and weather show in the preview even when the render
    // request doesn't include them.
    state.inner.lock().unwrap().cached_live_data_json = Some(
        serde_json::json!({
            "hardwareData": {"cpu": 77, "gpuUsage": 44, "memory": 32.0, "totalMemory": 64, "netDown": "1.2 MB/s", "netUp": "0.3 MB/s"},
            "weather": {"city": "Denver", "temp": 68.0, "high": 74.0, "low": 51.0, "condition": "Clear", "isDay": true, "celsius": false, "ready": true}
        }).to_string()
    );
    let (c_tel, b_tel) = req_bytes(
        addr,
        "POST",
        "/v1/preview/render",
        r#"{"format":"rgba","nowSec":1763414400}"#,
    )
    .await;
    assert_eq!(c_tel, 200);
    assert_ne!(
        b_post, b_tel,
        "cached_live_data_json hardware+weather did not affect preview render"
    );
    state.inner.lock().unwrap().cached_live_data_json = None;

    // Video through a real `openfluxd --preview-worker` subprocess: frames advance with nowMs while
    // nowSec stays fixed, and a 60-second jump catches up promptly.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = req_bytes(addr, "POST", "/v1/preview/close", "{}").await;
        let fake_mp4 = tmp.join("synth_loop.mp4");
        std::fs::write(&fake_mp4, b"synthetic-mp4").unwrap();
        let dec_script = tmp.join("test_vid_dec.py");
        std::fs::write(
            &dec_script,
            "#!/usr/bin/env python3\nimport sys, time\nw, h = int(sys.argv[2]), int(sys.argv[3])\ncolors = [(220,20,20), (20,220,20), (20,80,240), (230,210,30), (200,40,210), (30,210,210)]\nframes = [bytes([r, g, b]) * (w * h) for r, g, b in colors]\nfor i in range(40):\n    sys.stdout.buffer.write(frames[i % len(frames)])\n    sys.stdout.buffer.flush()\n    time.sleep(0.04)\n",
        ).unwrap();
        let mut perms = std::fs::metadata(&dec_script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dec_script, perms).unwrap();

        let prev_worker_env = std::env::var_os("OPENFLUX_PREVIEW_WORKER_BIN");
        let prev_dec_env = std::env::var_os("OPENFLUX_VIDEO_DECODER");
        std::env::set_var(
            "OPENFLUX_PREVIEW_WORKER_BIN",
            env!("CARGO_BIN_EXE_openfluxd"),
        );
        std::env::set_var("OPENFLUX_VIDEO_DECODER", &dec_script);

        let mut vapp = state.inner.lock().unwrap().appearance.clone();
        vapp["defaultTheme"]["backgroundImagePath"] = serde_json::json!(fake_mp4.to_string_lossy());
        vapp["defaultTheme"]["backgroundImageDimming"] = serde_json::json!(0.0);
        vapp["defaultTheme"]["backgroundImageBlurAmount"] = serde_json::json!(0.0);
        assert_eq!(
            req_bytes(
                addr,
                "PUT",
                "/v1/config",
                &serde_json::json!({"appearance": vapp}).to_string()
            )
            .await
            .0,
            200
        );

        let mut cur_ms = 0u64;
        let (_cv0, bv0) = req_bytes(
            addr,
            "POST",
            "/v1/preview/render",
            r#"{"format":"rgba","nowSec":1763414400,"nowMs":0}"#,
        )
        .await;
        let mut f_first = None;
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cur_ms += 100;
            let body = format!(r#"{{"format":"rgba","nowSec":1763414400,"nowMs":{cur_ms}}}"#);
            let (code, bytes) = req_bytes(addr, "POST", "/v1/preview/render", &body).await;
            assert_eq!(code, 200);
            if bytes != bv0 {
                f_first = Some(bytes);
                break;
            }
        }
        let f1 = f_first.expect("expected worker-decoded first video frame after opening MP4");
        let mut f2_opt = None;
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cur_ms += 100;
            let (cv2, bytes) = req_bytes(
                addr,
                "POST",
                "/v1/preview/render",
                &format!(r#"{{"format":"rgba","nowSec":1763414400,"nowMs":{cur_ms}}}"#),
            )
            .await;
            assert_eq!(cv2, 200);
            if bytes != f1 {
                f2_opt = Some(bytes);
                break;
            }
        }
        let f2 =
            f2_opt.expect("expected worker-decoded second video frame to advance at fixed nowSec");
        let mut f3_opt = None;
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cur_ms += 100;
            let (cv3, bytes) = req_bytes(
                addr,
                "POST",
                "/v1/preview/render",
                &format!(r#"{{"format":"rgba","nowSec":1763414400,"nowMs":{cur_ms}}}"#),
            )
            .await;
            assert_eq!(cv3, 200);
            if bytes != f2 && bytes != f1 {
                f3_opt = Some(bytes);
                break;
            }
        }
        assert!(
            f3_opt.is_some(),
            "expected worker-decoded video frames to keep advancing at fixed nowSec"
        );

        // A render after a 60-second gap (a hidden tab coming back) answers promptly.
        let t_jump = std::time::Instant::now();
        let jump_ms = cur_ms + 60_000;
        let (cv_jump, _) = req_bytes(
            addr,
            "POST",
            "/v1/preview/render",
            &format!(r#"{{"format":"rgba","nowSec":1763414400,"nowMs":{jump_ms}}}"#),
        )
        .await;
        assert_eq!(cv_jump, 200);
        assert!(
            t_jump.elapsed() < Duration::from_secs(2),
            "60s resume jump took too long: {:?}",
            t_jump.elapsed()
        );

        let _ = req_bytes(addr, "POST", "/v1/preview/close", "{}").await;
        match prev_worker_env {
            Some(v) => std::env::set_var("OPENFLUX_PREVIEW_WORKER_BIN", v),
            None => std::env::remove_var("OPENFLUX_PREVIEW_WORKER_BIN"),
        }
        match prev_dec_env {
            Some(v) => std::env::set_var("OPENFLUX_VIDEO_DECODER", v),
            None => std::env::remove_var("OPENFLUX_VIDEO_DECODER"),
        }
    }

    // A preview job still running when /preview/close gives up waiting for it (after 500 ms) unloads
    // the preview when it finishes, whether it is a render, a failing render or a rects request.
    for (label, method, path, body) in [
        (
            "render_ok",
            "POST",
            "/v1/preview/render",
            r#"{"format":"rgba","nowSec":1763414400}"#,
        ),
        (
            "render_err",
            "POST",
            "/v1/preview/render",
            r#"{"format":"rgba","liveJson":"not_json"}"#,
        ),
        ("rects", "GET", "/v1/preview/rects", ""),
    ] {
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        *state.preview_barrier.lock().unwrap() = Some((entered.clone(), release.clone()));
        let _open = OpenOnDrop(release.clone());

        let entered_fut = entered.notified();
        let job_task = tokio::spawn(async move { req_bytes(addr, method, path, body).await });
        tokio::time::timeout(Duration::from_secs(5), entered_fut)
            .await
            .unwrap_or_else(|_| panic!("{label}: barrier entry timeout"));

        // Close answers after its 500 ms wait while the job still holds the preview.
        let (c_close, _) = req_bytes(addr, "POST", "/v1/preview/close", "{}").await;
        assert_eq!(c_close, 200, "{label}");

        {
            let (m, cv) = &*release;
            *m.lock().unwrap() = true;
            cv.notify_all();
        }
        let _ = job_task.await.unwrap();
        *state.preview_barrier.lock().unwrap() = None;
        assert!(
            state.preview.lock().unwrap().is_none(),
            "{label}: preview leaked after close"
        );
    }

    // Two jobs from before a close, one running and one waiting its turn: once the running one is
    // released, neither leaves a preview loaded.
    let entered2 = Arc::new(tokio::sync::Notify::new());
    let release2 = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    *state.preview_barrier.lock().unwrap() = Some((entered2.clone(), release2.clone()));
    let _open2 = OpenOnDrop(release2.clone());

    let entered2_fut = entered2.notified();
    let active_job = tokio::spawn(async move {
        req_bytes(
            addr,
            "POST",
            "/v1/preview/render",
            r#"{"format":"rgba","nowSec":1763414400}"#,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), entered2_fut)
        .await
        .unwrap();
    let queued_job = tokio::spawn(async move {
        req_bytes(
            addr,
            "POST",
            "/v1/preview/render",
            r#"{"format":"rgba","nowSec":1763414400}"#,
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let (c_cls2, _) = req_bytes(addr, "POST", "/v1/preview/close", "{}").await;
    assert_eq!(c_cls2, 200);
    {
        let (m, cv) = &*release2;
        *m.lock().unwrap() = true;
        cv.notify_all();
    }
    let _ = active_job.await.unwrap();
    let (c_q, _) = queued_job.await.unwrap();
    assert_eq!(c_q, 409);
    *state.preview_barrier.lock().unwrap() = None;
    assert!(
        state.preview.lock().unwrap().is_none(),
        "queued pre-close job leaked preview"
    );

    // Preview worker subprocesses, with stand-ins for misbehaving workers:
    // 1. /preview/close kills and reaps a hung worker within 1.2 s.
    // 2. A worker that doesn't read, or doesn't answer, times out without a close.
    // 3. Reopening with the real worker streams full 1920x1080 RGBA frames.
    // 4. A client that stops reading a stream doesn't hold the preview after a close.
    // 5. A worker that exits mid-frame cuts the HTTP body short.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let hung_script = tmp.join("hung_worker.sh");
        std::fs::write(
            &hung_script,
            "#!/bin/sh\nprintf '\\000\\004'\nexec sleep 60\n",
        )
        .unwrap();
        std::fs::set_permissions(&hung_script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let prev_env = std::env::var_os("OPENFLUX_PREVIEW_WORKER_BIN");
        let prev_to = std::env::var_os("OPENFLUX_PREVIEW_RPC_TIMEOUT_MS");
        std::env::set_var("OPENFLUX_PREVIEW_WORKER_BIN", &hung_script);
        let hung_task = tokio::spawn(async move {
            req_bytes(addr, "POST", "/v1/preview/render", r#"{"format":"rgba"}"#).await
        });
        for _ in 0..40 {
            if state.preview_child.lock().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let t_all = std::time::Instant::now();
        let (c_hcls, _) = req_bytes(addr, "POST", "/v1/preview/close", "{}").await;
        assert_eq!(c_hcls, 200);
        let (c_hung, _) = tokio::time::timeout(Duration::from_millis(1200), hung_task)
            .await
            .expect("hung_task must unblock quickly after /preview/close")
            .unwrap();
        assert_ne!(c_hung, 200);
        assert!(
            t_all.elapsed() < Duration::from_millis(1500),
            "full close+reap took {:?}",
            t_all.elapsed()
        );
        assert!(state.preview_child.lock().unwrap().is_none());
        assert!(state.preview.lock().unwrap().is_none());

        // 2. A worker that never reads a large (over 256 KB) configuration is killed and reaped at
        //    its deadline, without a close.
        let nonread_script = tmp.join("nonread_worker.sh");
        std::fs::write(&nonread_script, "#!/bin/sh\nexec sleep 60\n").unwrap();
        std::fs::set_permissions(&nonread_script, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("OPENFLUX_PREVIEW_WORKER_BIN", &nonread_script);
        std::env::set_var("OPENFLUX_PREVIEW_RPC_TIMEOUT_MS", "250");
        let big_name = "X".repeat(300_000);
        let _ = req_bytes(
            addr,
            "PUT",
            "/v1/config",
            &serde_json::json!({"appearance": {"selectedId": "default", "defaultTheme": {"id": "default", "name": big_name}}}).to_string(),
        ).await;
        let t_big = std::time::Instant::now();
        let (c_big, _) =
            req_bytes(addr, "POST", "/v1/preview/render", r#"{"format":"rgba"}"#).await;
        assert_ne!(c_big, 200);
        assert!(
            t_big.elapsed() < Duration::from_millis(1500),
            "large non-reading RPC timeout took {:?}",
            t_big.elapsed()
        );
        assert!(state.preview_child.lock().unwrap().is_none());
        assert!(state.preview.lock().unwrap().is_none());

        // Put back a small appearance before rendering with the real worker.
        let _ = req_bytes(
            addr,
            "PUT",
            "/v1/config",
            r#"{"appearance": {"selectedId": "default", "defaultTheme": {"id": "default", "name": "Restored"}}}"#,
        ).await;

        // 3. The real `openfluxd --preview-worker` streams a full RGBA frame, then closes.
        std::env::remove_var("OPENFLUX_PREVIEW_RPC_TIMEOUT_MS");
        std::env::set_var(
            "OPENFLUX_PREVIEW_WORKER_BIN",
            env!("CARGO_BIN_EXE_openfluxd"),
        );
        let (c_ok, b_ok) = req_bytes(
            addr,
            "POST",
            "/v1/preview/render",
            r#"{"format":"rgba","nowSec":1763414400}"#,
        )
        .await;
        assert_eq!(c_ok, 200);
        assert_eq!(b_ok.len(), 1920 * 1080 * 4);
        assert!(state.preview_child.lock().unwrap().is_some());
        assert_eq!(
            req_bytes(addr, "POST", "/v1/preview/close", "{}").await.0,
            200
        );
        assert!(state.preview_child.lock().unwrap().is_none());
        assert!(state.preview.lock().unwrap().is_none());

        // 4. A client that stops reading stalls its stream. /preview/close frees the preview, and a
        //    new preview works while the stalled connection is still open.
        let mut stalled_sock = TcpStream::connect(addr).await.unwrap();
        let body_s = r#"{"format":"rgba"}"#;
        stalled_sock
            .write_all(
                format!(
                    "POST /v1/preview/render HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer prev_tok\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body_s}",
                    body_s.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        // Read the response headers before stopping, so the stream is under way when reading stops.
        let mut hdr_buf = [0u8; 512];
        let mut hdr_len = 0usize;
        while hdr_len < hdr_buf.len() {
            let n = stalled_sock.read(&mut hdr_buf[hdr_len..]).await.unwrap();
            assert!(n > 0, "stalled_sock closed before headers");
            hdr_len += n;
            if hdr_buf[..hdr_len].windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        assert!(String::from_utf8_lossy(&hdr_buf[..hdr_len]).contains("200 OK"));

        // Wait until the stream has a frame with no room to send it.
        let full_fut = state.preview_stream_full.notified();
        tokio::time::timeout(Duration::from_secs(5), full_fut)
            .await
            .expect("expected stream backpressure TrySendError::Full while stalled_sock stopped reading");

        let t_st = std::time::Instant::now();
        assert_eq!(
            req_bytes(addr, "POST", "/v1/preview/close", "{}").await.0,
            200
        );
        assert!(t_st.elapsed() < Duration::from_millis(1200));

        // The preview is free and reopens while the stalled connection is still open.
        for _ in 0..40 {
            if state.preview_sem.available_permits() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(state.preview_sem.available_permits(), 1);
        assert!(state.preview.lock().unwrap().is_none());

        let (c_reopen, b_reopen) = req_bytes(
            addr,
            "POST",
            "/v1/preview/render",
            r#"{"format":"rgba","nowSec":1763414400}"#,
        )
        .await;
        assert_eq!(c_reopen, 200);
        assert_eq!(b_reopen.len(), 1920 * 1080 * 4);
        assert_eq!(
            req_bytes(addr, "POST", "/v1/preview/close", "{}").await.0,
            200
        );
        assert!(state.preview_child.lock().unwrap().is_none());
        drop(stalled_sock);

        // 5. A worker that sends one 64 KiB chunk of a frame and exits: the response has already
        //    started (200, Content-Length: 8294400), so the body is cut short and the client sees it
        //    is incomplete.
        let trunc_script = tmp.join("trunc_worker.py");
        std::fs::write(
            &trunc_script,
            r#"#!/usr/bin/env python3
import struct, sys, time
def read_req():
    hdr = sys.stdin.buffer.read(5)
    if len(hdr) < 5: sys.exit(0)
    cmd = hdr[0]
    n = struct.unpack("<I", hdr[1:5])[0]
    _ = sys.stdin.buffer.read(n)
    return cmd
assert read_req() == 1
sys.stdout.buffer.write(struct.pack("<BI", 0, 0))
sys.stdout.buffer.flush()
assert read_req() == 4
sys.stdout.buffer.write(struct.pack("<BI", 0, 1920 * 1080 * 4))
sys.stdout.buffer.write(b"A" * 65536)
sys.stdout.buffer.flush()
time.sleep(0.15)
sys.exit(1)
"#,
        )
        .unwrap();
        std::fs::set_permissions(&trunc_script, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("OPENFLUX_PREVIEW_WORKER_BIN", &trunc_script);

        let mut tr_sock = TcpStream::connect(addr).await.unwrap();
        let tr_body = r#"{"format":"rgba"}"#;
        tr_sock.write_all(
            format!(
                "POST /v1/preview/render HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer prev_tok\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{tr_body}",
                tr_body.len()
            ).as_bytes(),
        ).await.unwrap();
        let mut tr_resp = Vec::new();
        let _ = tr_sock.read_to_end(&mut tr_resp).await;
        let tr_pos = tr_resp
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("expected HTTP 200 headers before mid-stream abort");
        let tr_hdr = String::from_utf8_lossy(&tr_resp[..tr_pos]);
        assert!(tr_hdr.contains("200 OK"), "{tr_hdr}");
        let cl: usize = tr_hdr
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
            .and_then(|l| l.split(':').nth(1))
            .and_then(|v| v.trim().parse().ok())
            .expect("expected Content-Length header");
        assert_eq!(cl, 1920 * 1080 * 4);
        let received_body = tr_resp.len() - (tr_pos + 4);
        assert!(
            received_body >= 65536 && received_body < cl,
            "received={received_body} cl={cl}"
        );
        assert!(state.preview.lock().unwrap().is_none());

        if let Some(old) = prev_env {
            std::env::set_var("OPENFLUX_PREVIEW_WORKER_BIN", old);
        } else {
            std::env::remove_var("OPENFLUX_PREVIEW_WORKER_BIN");
        }
        if let Some(old_to) = prev_to {
            std::env::set_var("OPENFLUX_PREVIEW_RPC_TIMEOUT_MS", old_to);
        } else {
            std::env::remove_var("OPENFLUX_PREVIEW_RPC_TIMEOUT_MS");
        }
    }

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn runtime_profile_and_live_theme_switch_changes_rendered_pixels() {
    let tmp = std::env::temp_dir().join(format!("openfluxd_prof_theme_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("config")).unwrap();
    std::fs::create_dir_all(tmp.join("media")).unwrap();

    let appearance = serde_json::json!({
        "selectedId": "default",
        "defaultTheme": {
            "id": "default",
            "name": "Default",
            "legendColor": 0xFFFFFFFFu32,
            "ringGlowColor": 0xFF102030u32,
            "selectedKeyEffect": "glowing_rings",
            "backgroundImageDimming": 100.0
        },
        "customThemes": [
            {
                "id": "crimson",
                "name": "Crimson",
                "legendColor": 0xFFFF0000u32,
                "ringGlowColor": 0xFFFF2200u32,
                "selectedKeyEffect": "glowing_rings",
                "backgroundImageDimming": 100.0,
                "stylemaps": {
                    "0": {
                        "styles": {
                            "0": { "label": "OVR", "legendColor": 0xFF00FF00u32 }
                        }
                    }
                }
            },
            {
                "id": "azure",
                "name": "Azure",
                "legendColor": 0xFF00CCFFu32,
                "ringGlowColor": 0xFF0044FFu32,
                "selectedKeyEffect": "solid_highlight",
                "backgroundImageDimming": 100.0
            }
        ]
    });
    let profiles = serde_json::json!([
        {"id": "default", "name": "Default", "theme_id": "default"},
        {"id": "fps", "name": "FPS", "theme_id": "crimson"},
        {"id": "work", "name": "Work", "theme_id": "azure"}
    ]);
    std::fs::write(tmp.join("config/appearance.json"), appearance.to_string()).unwrap();
    std::fs::write(tmp.join("config/profiles.json"), profiles.to_string()).unwrap();

    // A fresh preview of "azure", to compare against previews that switched to it.
    let mut app_azure = appearance.clone();
    app_azure["selectedId"] = serde_json::json!("azure");
    let tmp_fresh_b = tmp.join("fresh_b");
    std::fs::create_dir_all(tmp_fresh_b.join("config")).unwrap();
    std::fs::write(
        tmp_fresh_b.join("config/appearance.json"),
        app_azure.to_string(),
    )
    .unwrap();
    let mut d_fresh_b = openfluxd::preview::DisplayPreview::new(1920, 1080).unwrap();
    d_fresh_b
        .load_config_dir(
            tmp_fresh_b.join("config").to_str().unwrap(),
            tmp.join("media").to_str().unwrap(),
        )
        .unwrap();
    let frame_fresh_b = d_fresh_b.render_rgba(1763414400, 0, false).unwrap();

    let mut d = openfluxd::preview::DisplayPreview::new(1920, 1080).unwrap();
    d.load_config_dir(
        tmp.join("config").to_str().unwrap(),
        tmp.join("media").to_str().unwrap(),
    )
    .unwrap();
    let frame_default = d.render_rgba(1763414400, 0, false).unwrap();

    // The firmware reports profile 1 ("crimson", with a key 0 override).
    d.apply_live_json(r#"{"type":"key_state","active_profile":1}"#, 100)
        .unwrap();
    let frame_prof1 = d.render_rgba(1763414400, 0, false).unwrap();
    assert_ne!(
        frame_default, frame_prof1,
        "key_state active_profile=1 switches the rendered theme through profiles.json"
    );
    assert_ne!(
        frame_prof1, frame_fresh_b,
        "crimson and azure must render distinct colors & key effects"
    );

    // The host's activeThemeId "azure" for slot 1 renders exactly like a fresh load of "azure".
    d.apply_live_json(
        r#"{"activeThemeId":"azure","activeThemeProfileSlot":1}"#,
        200,
    )
    .unwrap();
    let frame_azure = d.render_rgba(1763414400, 0, false).unwrap();
    assert_eq!(
        frame_azure, frame_fresh_b,
        "A -> B switch must not retain A's asymmetric stylemap overrides"
    );

    // The firmware moving back to profile 0 ("default") supersedes the host's activeThemeId for slot 1,
    // and repeated host packets carrying slot-1 activeThemeId="azure" must not overwrite slot 0.
    d.apply_live_json(r#"{"type":"key_state","active_profile":0}"#, 300)
        .unwrap();
    d.apply_live_json(
        r#"{"activeThemeId":"azure","activeThemeProfileSlot":1}"#,
        350,
    )
    .unwrap();
    let frame_back = d.render_rgba(1763414400, 0, false).unwrap();
    assert_eq!(
        frame_back, frame_default,
        "repeated host override for slot 1 must not override observed slot 0"
    );

    // Replacing only profiles.json and reloading gives profile 0 the crimson theme.
    let profiles_updated = serde_json::json!([
        {"id": "default", "name": "Default", "theme_id": "crimson"}
    ]);
    std::fs::write(
        tmp.join("config/profiles.json"),
        profiles_updated.to_string(),
    )
    .unwrap();
    d.load_config_dir(
        tmp.join("config").to_str().unwrap(),
        tmp.join("media").to_str().unwrap(),
    )
    .unwrap();
    let frame_reloaded = d.render_rgba(1763414400, 0, false).unwrap();
    assert_eq!(
        frame_reloaded, frame_prof1,
        "profiles.json-only update must resolve profile 0 -> crimson"
    );

    // A reused preview forgets the live theme when its configuration is replaced: after showing
    // crimson for slot 0, loading a configuration that selects azure renders like a fresh azure.
    let mut d_prev = openfluxd::preview::DisplayPreview::new(1920, 1080).unwrap();
    d_prev
        .set_config_json(
            Some(&appearance.to_string()),
            None,
            None,
            None,
            Some(tmp.join("media").to_str().unwrap()),
        )
        .unwrap();
    d_prev
        .apply_live_json(
            r#"{"activeThemeId":"crimson","activeThemeProfileSlot":0}"#,
            10,
        )
        .unwrap();
    let frame_scoped = d_prev.render_rgba(1763414400, 0, false).unwrap();
    assert_eq!(frame_scoped, frame_prof1);
    d_prev
        .set_config_json(
            Some(&app_azure.to_string()),
            None,
            None,
            None,
            Some(tmp.join("media").to_str().unwrap()),
        )
        .unwrap();
    let frame_draft_b = d_prev.render_rgba(1763414400, 0, false).unwrap();
    assert_eq!(
        frame_draft_b, frame_fresh_b,
        "switching from crimson to azure drops crimson's live theme"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
