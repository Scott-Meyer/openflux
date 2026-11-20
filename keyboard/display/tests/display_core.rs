use openflux_display::layout::{build_physical_layout, compute_key_parallax, FrameLayoutType};
use openflux_display::media_bg::{VideoAdapter, VideoStream};
use openflux_display::{DisplayConfig, DisplayPreview};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Instant;

struct FakeUnpacedVideo {
    read_calls: Arc<AtomicUsize>,
}

struct FakeUnpacedStream {
    frame_idx: u64,
    read_calls: Arc<AtomicUsize>,
}

impl VideoAdapter for FakeUnpacedVideo {
    fn open(&self, _path: &str, _w: i32, _h: i32) -> Option<Box<dyn VideoStream>> {
        Some(Box::new(FakeUnpacedStream {
            frame_idx: 0,
            read_calls: self.read_calls.clone(),
        }))
    }
}

impl VideoStream for FakeUnpacedStream {
    fn try_read_frame(&mut self) -> Result<Option<(u64, i32, i32, Vec<u8>)>, ()> {
        let calls = self.read_calls.fetch_add(1, Ordering::SeqCst);
        if calls >= 2000 {
            return Ok(None);
        }
        let val = ((self.frame_idx * 17 + 40) & 0xFF) as u8;
        let pts_ms = self.frame_idx * 33;
        self.frame_idx += 1;
        Ok(Some((pts_ms, 16, 8, vec![val; 16 * 8 * 3])))
    }
}

#[test]
fn layout_geometry_matches_physical_mm_and_mosaic_totals() {
    let ansi = build_physical_layout(FrameLayoutType::Ansi);
    let iso = build_physical_layout(FrameLayoutType::Iso);
    assert_eq!(ansi.keys.len(), 85);
    assert_eq!(iso.keys.len(), 87);

    // Mosaic margins, four tiles, and three gaps span the 1920 px display
    let tile_w = (ansi.mosaic_w - 3.0 * ansi.mosaic_gap) / 4.0;
    let total = ansi.mosaic_x * 2.0 + 4.0 * tile_w + 3.0 * ansi.mosaic_gap;
    assert!((total - 1920.0).abs() < 0.01, "total={total}");
    assert!((tile_w - 408.9).abs() < 0.1, "tile_w={tile_w}");

    // F-row gap 6.75 mm
    let f_gap_px = ansi.keys[16].y - (ansi.keys[0].y + ansi.keys[0].h);
    assert!((f_gap_px - 6.75 * (1920.0 / 309.31)).abs() < 0.25);
}

#[test]
fn parallax_center_key_matches_10_5mm_projection() {
    let cfg = DisplayConfig::default();
    let (dx_c, dy_c) = compute_key_parallax(&cfg.settings, 960.0, 700.0);
    assert!(dx_c.abs() < 0.1);
    assert!(dy_c < -10.0 && dy_c > -28.0);
    // Refraction keeps horizontal edge shifts near 8.3 px
    let (dx_l, _) = compute_key_parallax(&cfg.settings, 75.45, 227.0);
    let (dx_r, _) = compute_key_parallax(&cfg.settings, 1844.55, 227.0);
    assert!(
        (dx_l + 8.31).abs() < 1.2,
        "expected ~-8.3px left edge shift, got {dx_l}"
    );
    assert!(
        (dx_r - 8.31).abs() < 1.2,
        "expected ~+8.3px right edge shift, got {dx_r}"
    );
}

#[test]
fn idle_render_has_no_unwanted_squares_or_circles_and_is_fast() {
    let mut d = DisplayPreview::new(1920, 1080);
    // Custom solid red background so any drawn keycap box/outline would immediately stand out
    d.set_custom_bg_rgba(&[200, 30, 30, 255], 1, 1).unwrap();
    let t0 = Instant::now();
    let frame = d.render_rgba(1763414400, 0, false);
    let elapsed_ms = t0.elapsed().as_millis();
    assert_eq!(frame.len(), 1920 * 1080 * 4);
    // Warm renders must remain within the build-specific timing budget
    let t1 = Instant::now();
    let frame2 = d.render_rgba(1763414400, 100, false);
    let warm_ms = t1.elapsed().as_millis();
    let warm_budget_ms = if cfg!(debug_assertions) { 1200 } else { 30 };
    assert!(
        warm_ms < warm_budget_ms,
        "warm render took {warm_ms}ms (cold={elapsed_ms}ms)"
    );

    // Sample corner of Key #0 away from legend text: without showAlignmentGrid, pixel must stay pure bg
    let k0 = d.get_key_rects()[0];
    let px = (k0.x as usize + 6) + (k0.y as usize + 6) * 1920;
    assert_eq!(
        frame2[px * 4],
        frame2[4],
        "idle key corner must not draw square/box"
    );

    // Enabling showAlignmentGrid must draw keycap outlines
    d.set_config_json(
        None,
        Some(r#"{"showAlignmentGrid": true}"#),
        None,
        None,
        None,
    )
    .unwrap();
    let frame_grid = d.render_rgba(1763414400, 200, false);
    assert_ne!(
        frame2, frame_grid,
        "showAlignmentGrid must render alignment guides"
    );
}

#[test]
fn unpaced_video_resume_60s_jump_is_bounded_and_advances() {
    let tmp = std::env::temp_dir().join(format!("flux_rust_vid_resume_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let mp4 = tmp.join("resume.mp4");
    std::fs::write(&mp4, b"fake_mp4").unwrap();

    let reads = Arc::new(AtomicUsize::new(0));
    let mut d = DisplayPreview::new(16, 8);
    d.renderer
        .bg
        .set_video_adapter(Some(Arc::new(FakeUnpacedVideo {
            read_calls: reads.clone(),
        })));
    let app = serde_json::json!({
        "selectedId": "default",
        "defaultTheme": {
            "id": "default",
            "backgroundImagePath": mp4.to_string_lossy(),
            "backgroundImageDimming": 0,
            "backgroundImageBlurAmount": 0
        }
    });
    d.set_config_json(Some(&app.to_string()), None, None, None, None)
        .unwrap();
    let _ = d.render_rgba(1763414400, 100, false);
    let before_reads = reads.load(Ordering::SeqCst);
    let px_before = d.renderer.bg.pixels[0];

    let _ = d.render_rgba(1763414400, 60100, false);
    let delta_reads = reads.load(Ordering::SeqCst) - before_reads;
    assert!(
        delta_reads > 0 && delta_reads <= 10,
        "delta_reads={delta_reads}"
    );
    assert_ne!(d.renderer.bg.pixels[0], px_before);

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn profile_switch_and_drop_shadow_toggle_change_pixels() {
    let tmp = std::env::temp_dir().join(format!("flux_rust_theme_sw_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("config")).unwrap();
    std::fs::create_dir_all(tmp.join("media")).unwrap();

    let app = serde_json::json!({
        "selectedId": "default",
        "defaultTheme": {
            "id": "default",
            "name": "Default",
            "legendColor": 0xFFFFFFFFu32,
            "ringGlowColor": 0xFF102030u32,
            "blendingModePreset": "dropShadow",
            "backgroundImageDimming": 0
        },
        "customThemes": [{
            "id": "crimson",
            "name": "Crimson",
            "legendColor": 0xFFFF0000u32,
            "ringGlowColor": 0xFFFF2200u32,
            "blendingModePreset": "off",
            "backgroundImageDimming": 0
        }]
    });
    let profs = serde_json::json!([
        {"id": "default", "name": "Default", "theme_id": "default"},
        {"id": "fps", "name": "FPS", "theme_id": "crimson"}
    ]);
    std::fs::write(tmp.join("config/appearance.json"), app.to_string()).unwrap();
    std::fs::write(tmp.join("config/profiles.json"), profs.to_string()).unwrap();
    let km = serde_json::json!({
        "selectedFrameLayoutType": "ansi",
        "keymaps": {
            "ansi": {
                "defaultLayer": 0,
                "layers": [{
                    "name": "Base",
                    "keyActions": [{"type": 0, "keyCode": "KC_A"}]
                }]
            }
        }
    });
    std::fs::write(tmp.join("config/keymap.json"), km.to_string()).unwrap();

    let mut d = DisplayPreview::new(1920, 1080);
    d.set_custom_bg_rgba(&[180, 180, 180, 255], 1, 1).unwrap();
    d.load_config_dir(
        tmp.join("config").to_str().unwrap(),
        tmp.join("media").to_str().unwrap(),
    )
    .unwrap();
    let f_def = d.render_rgba(1763414400, 0, false);

    d.apply_live_json(r#"{"type":"key_state","active_profile":1}"#, 100)
        .unwrap();
    let f_p1 = d.render_rgba(1763414400, 0, false);
    assert_ne!(f_def, f_p1);

    // Physical cycle back to 0 must override any stale host activeThemeId
    d.apply_live_json(
        r#"{"activeThemeId":"crimson","activeThemeProfileSlot":1}"#,
        150,
    )
    .unwrap();
    d.apply_live_json(r#"{"type":"key_state","active_profile":0}"#, 200)
        .unwrap();
    let f_back = d.render_rgba(1763414400, 0, false);
    assert_eq!(f_def, f_back);

    // Toggle only blendingModePreset ("dropShadow" -> "off") with all else unchanged
    let mut app_no_shadow = app.clone();
    app_no_shadow["defaultTheme"]["blendingModePreset"] = serde_json::json!("off");
    d.set_config_json(Some(&app_no_shadow.to_string()), None, None, None, None)
        .unwrap();
    let f_no_shadow = d.render_rgba(1763414400, 0, false);
    assert_ne!(f_def, f_no_shadow);

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn unicode_media_paths_extensionless_and_bundled_weather_svgs_render() {
    // Unicode path safety
    let tmp = std::env::temp_dir().join(format!("flux_rust_uni_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let uni_ppm = tmp.join("猫.ppm");
    std::fs::write(&uni_ppm, b"P6\n1 1\n255\n\xaa\xbb\xcc").unwrap();

    let mut d = DisplayPreview::new(16, 8);
    let app = serde_json::json!({
        "selectedId": "default",
        "defaultTheme": {
            "id": "default",
            "backgroundImagePath": uni_ppm.to_string_lossy(),
            "backgroundImageDimming": 0
        }
    });
    d.set_config_json(Some(&app.to_string()), None, None, None, None)
        .unwrap();
    let frame = d.render_rgba(1763414400, 0, false);
    assert_eq!(frame[0], 0xAA);
    // Extensionless path must not panic either
    let app_extless = serde_json::json!({
        "selectedId": "default",
        "defaultTheme": { "id": "default", "backgroundImagePath": "猫" }
    });
    d.set_config_json(Some(&app_extless.to_string()), None, None, None, None)
        .unwrap();
    let _ = d.render_rgba(1763414400, 0, false);

    // Bundled weather SVG rendering
    let wi_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/weather_icons");
    let wi_root = wi_dir.parent().unwrap().to_string_lossy().to_string();
    let mut cache = openflux_display::assets::AssetCache::new();
    let mut pix = vec![0xFF00_0000u32; 64 * 64];
    for cond in ["Sunny", "Cloudy", "Light Rain"] {
        pix.fill(0xFF00_0000u32);
        assert!(
            cache.draw_weather_svg(
                Some(&wi_root),
                cond,
                &mut pix,
                64,
                64,
                4,
                4,
                48,
                0xFFFF_FFFF
            ),
            "expected bundled weather SVG to load for {cond}"
        );
        assert!(
            pix.contains(&0xFFFF_FFFF),
            "expected non-empty SVG pixels for {cond}"
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn fast_paced_ffmpeg_like_decoder_advances_without_future_freeze() {
    // Simulate a fast decoder producing frames at PTS 0, 40, 80, ... without internal sleep
    let tmp = std::env::temp_dir().join(format!("flux_rust_fast_vid_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let mp4 = tmp.join("fast.mp4");
    std::fs::write(&mp4, b"fast").unwrap();

    // Queue-backed adapter producing twelve frames at 40 ms intervals
    struct FastPacedAdapter;
    struct FastPacedStream {
        frames: std::collections::VecDeque<(u64, i32, i32, Vec<u8>)>,
    }
    impl VideoAdapter for FastPacedAdapter {
        fn open(&self, _path: &str, _w: i32, _h: i32) -> Option<Box<dyn VideoStream>> {
            // A deterministic queue avoids dependence on thread scheduling
            let mut frames = std::collections::VecDeque::new();
            for idx in 0u64..12 {
                let val = ((idx * 20 + 20) & 0xFF) as u8;
                frames.push_back((idx * 40, 8, 8, vec![val; 8 * 8 * 3]));
            }
            Some(Box::new(FastPacedStream { frames }))
        }
    }
    impl VideoStream for FastPacedStream {
        fn poll_ready(&self) -> bool {
            !self.frames.is_empty()
        }
        fn try_read_frame(&mut self) -> Result<Option<(u64, i32, i32, Vec<u8>)>, ()> {
            Ok(self.frames.pop_front())
        }
    }

    let mut d = DisplayPreview::new(8, 8);
    d.renderer
        .bg
        .set_video_adapter(Some(Arc::new(FastPacedAdapter)));
    let app = serde_json::json!({
        "selectedId": "default",
        "defaultTheme": {
            "id": "default",
            "backgroundImagePath": mp4.to_string_lossy(),
            "backgroundImageDimming": 0
        }
    });
    d.set_config_json(Some(&app.to_string()), None, None, None, None)
        .unwrap();
    let _ = d.render_rgba(1763414400, 100, false);
    let px0 = d.renderer.bg.pixels[0];
    // Future PTS=40ms at now_ms=120 (elapsed 20ms) must not claim ready
    assert!(!d.renderer.bg.has_ready_video_frame_at(120));
    assert!(d.renderer.bg.has_ready_video_frame_at(145));
    let _ = d.render_rgba(1763414400, 145, false);
    let px1 = d.renderer.bg.pixels[0];
    let _ = d.render_rgba(1763414400, 185, false);
    let px2 = d.renderer.bg.pixels[0];
    assert_ne!(px0, px1);
    assert_ne!(px1, px2);

    // Multi-overdue jump (elapsed 85ms -> 225ms at now_ms=325): skips PTS 120/160, picks PTS 200 (idx 5 -> val 120),
    // and retains future PTS 240 until 340ms
    let prev_rendered = d.renderer.bg.video_frames_rendered;
    let _ = d.render_rgba(1763414400, 325, false);
    assert_eq!(d.renderer.bg.video_frames_rendered, prev_rendered + 3);
    assert_eq!(d.renderer.bg.pixels[0] & 0xFF, 120);
    assert!(!d.renderer.bg.has_ready_video_frame_at(335));
    assert!(d.renderer.bg.has_ready_video_frame_at(340));

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn ipc_line_buffer_handles_split_utf8_and_video_stream_recovers_after_eof() {
    use openflux_display::ipc::LineStreamBuffer;

    // 1. Split UTF-8 codepoint across nonblocking reads
    let mut lb = LineStreamBuffer::new();
    let msg = b"{\"city\":\"M\xc3\xbcnchen\"}\n{\"ok\":true}\n";
    assert!(lb.push_bytes(&msg[..11]).is_empty());
    let lines = lb.push_bytes(&msg[11..]);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], "{\"city\":\"München\"}");
    assert_eq!(lines[1], "{\"ok\":true}");

    // 2. Real FfmpegVideoAdapter with fake raw-frame decoder script -> wait until EOF/2 frames, then poll_ready() is false
    #[cfg(unix)]
    {
        use openflux_display::FfmpegVideoAdapter;
        use std::os::unix::fs::PermissionsExt;
        let tmp = std::env::temp_dir().join(format!("flux_real_ad_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let mp4 = tmp.join("clip.mp4");
        std::fs::write(&mp4, b"dummy").unwrap();
        let dec = tmp.join("fake_dec.py");
        std::fs::write(
            &dec,
            "#!/usr/bin/env python3\nimport sys\nw, h = int(sys.argv[2]), int(sys.argv[3])\nfor i in range(2):\n    sys.stdout.buffer.write(bytes([(i*50+10)&0xff])*(w*h*3))\n    sys.stdout.buffer.flush()\n",
        ).unwrap();
        std::fs::set_permissions(&dec, std::fs::Permissions::from_mode(0o755)).unwrap();
        let ad = FfmpegVideoAdapter {
            sync_first_frame: true,
            target_scale: Some((8, 8)),
            decoder_bin: Some(dec.to_string_lossy().to_string()),
            ffmpeg_bin: None,
        };
        let mut stream = ad.open(mp4.to_str().unwrap(), 8, 8).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut got = 0;
        while got < 2 && std::time::Instant::now() < deadline {
            match stream.try_read_frame() {
                Ok(Some(_)) => got += 1,
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
                Err(()) => break,
            }
        }
        assert_eq!(got, 2);
        assert!(
            !stream.poll_ready(),
            "poll_ready() must return false once queue is drained"
        );

        // 3. Explicit ffmpeg_bin must be invoked directly without falling back to system/PATH ffmpeg;
        // missing explicit path or unset host ffmpeg_bin must return None cleanly.
        let fake_ff = tmp.join("fake_ffmpeg.py");
        let ff_marker = tmp.join("fake_ffmpeg_args.txt");
        std::fs::write(
            &fake_ff,
            format!(
                "#!/usr/bin/env python3\nimport sys\nwith open({:?}, 'w') as f: f.write(' '.join(sys.argv[1:]))\nsys.stdout.buffer.write(b'\\x11\\x22\\x33' * (8 * 8))\nsys.stdout.buffer.flush()\n",
                ff_marker.to_string_lossy()
            ),
        ).unwrap();
        std::fs::set_permissions(&fake_ff, std::fs::Permissions::from_mode(0o755)).unwrap();
        let ad_ff = FfmpegVideoAdapter {
            sync_first_frame: true,
            target_scale: Some((8, 8)),
            decoder_bin: None,
            ffmpeg_bin: Some(fake_ff.clone()),
        };
        let mut ff_stream = ad_ff.open(mp4.to_str().unwrap(), 8, 8).unwrap();
        let ff_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let frame = loop {
            match ff_stream.try_read_frame() {
                Ok(Some(frame)) => break frame,
                Ok(None) => {
                    assert!(
                        std::time::Instant::now() < ff_deadline,
                        "timed out waiting for fake ffmpeg_bin frame"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(()) => {
                    panic!("fake ffmpeg_bin stream hit unexpected EOF/error before first frame")
                }
            }
        };
        assert_eq!(frame.1, 8);
        assert_eq!(frame.2, 8);
        assert_eq!(frame.3, [0x11u8, 0x22, 0x33].repeat(8 * 8));
        drop(ff_stream);
        let argv_txt = std::fs::read_to_string(&ff_marker).unwrap();
        assert!(
            argv_txt.contains("-pix_fmt rgb24"),
            "unexpected ffmpeg args: {argv_txt}"
        );

        let ad_missing_ff = FfmpegVideoAdapter {
            sync_first_frame: true,
            target_scale: Some((8, 8)),
            decoder_bin: None,
            ffmpeg_bin: Some(tmp.join("does_not_exist_ffmpeg")),
        };
        assert!(ad_missing_ff.open(mp4.to_str().unwrap(), 8, 8).is_none());
        if !cfg!(target_os = "linux") {
            assert_eq!(FfmpegVideoAdapter::default().resolve_ffmpeg_bin(), None);
        } else {
            assert_eq!(
                FfmpegVideoAdapter::default().resolve_ffmpeg_bin(),
                Some(std::path::PathBuf::from(
                    openflux_display::device_paths::DISPLAY_FFMPEG_PATH
                ))
            );
        }

        // 4. Dropping a decoder wrapper must terminate descendants that inherit stdout
        let stall_dec = tmp.join("stall_dec.sh");
        std::fs::write(&stall_dec, "#!/bin/sh\nsleep 30 &\nwait\n").unwrap();
        std::fs::set_permissions(&stall_dec, std::fs::Permissions::from_mode(0o755)).unwrap();
        let ad_stall = FfmpegVideoAdapter {
            sync_first_frame: false,
            target_scale: Some((8, 8)),
            decoder_bin: Some(stall_dec.to_string_lossy().to_string()),
            ffmpeg_bin: None,
        };
        let stall_stream = ad_stall.open(mp4.to_str().unwrap(), 8, 8).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        let t0 = std::time::Instant::now();
        drop(stall_stream);
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(2),
            "dropping wrapper-stalled decoder hung"
        );

        // 5. Drained decoder EOF after displayed frame must surface disconnected state so BgCache recovers & reopens
        let eof_dec = tmp.join("eof_dec.py");
        let count_file = tmp.join("runs.txt");
        std::fs::write(
            &eof_dec,
            format!(
                "#!/usr/bin/env python3\nimport sys, time\nwith open({:?}, 'a') as f: f.write('1\\n')\nw, h = int(sys.argv[2]), int(sys.argv[3])\nsys.stdout.buffer.write(bytes([0, 220, 0])*(w*h))\nsys.stdout.buffer.flush()\ntime.sleep(0.08)\n",
                count_file.to_string_lossy()
            ),
        ).unwrap();
        std::fs::set_permissions(&eof_dec, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut rec_d = DisplayPreview::new(8, 8);
        rec_d
            .renderer
            .bg
            .set_video_adapter(Some(Arc::new(FfmpegVideoAdapter {
                sync_first_frame: true,
                target_scale: Some((8, 8)),
                decoder_bin: Some(eof_dec.to_string_lossy().to_string()),
                ffmpeg_bin: None,
            })));
        let eof_app = serde_json::json!({
            "selectedId": "default",
            "defaultTheme": { "backgroundImagePath": mp4.to_string_lossy(), "backgroundImageDimming": 0 }
        }).to_string();
        rec_d
            .set_config_json(Some(&eof_app), None, None, None, None)
            .unwrap();
        let deadline_first = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while rec_d.renderer.bg.video_frames_rendered < 1
            && std::time::Instant::now() < deadline_first
        {
            let _ = rec_d.render_rgba(1763414400, 100, false);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(rec_d.renderer.bg.video_frames_rendered, 1);
        // Wait until first decoder exits after emitting 1 frame, then step BgCache -> must detect EOF & reopen
        std::thread::sleep(std::time::Duration::from_millis(150));
        let deadline_r = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline_r {
            let _ = rec_d.render_rgba(1763414400, 300, false);
            if std::fs::read_to_string(&count_file)
                .map(|s| s.lines().count())
                .unwrap_or(0)
                >= 2
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        assert!(
            std::fs::read_to_string(&count_file)
                .map(|s| s.lines().count())
                .unwrap_or(0)
                >= 2,
            "BgCache failed to reopen the decoder after EOF"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}

#[test]
fn config_reloader_honors_transaction_markers_and_sparse_mosaic_slots_keep_fixed_columns() {
    use openflux_display::config::ConfigReloader;

    // 1. Config transaction markers
    let tmp = std::env::temp_dir().join(format!("flux_cfg_tx_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(
        tmp.join("appearance.json"),
        r#"{"selectedId":"default","defaultTheme":{"name":"Initial"}}"#,
    )
    .unwrap();
    let mut cfg = DisplayConfig::load_dir(&tmp);
    let mut rel = ConfigReloader::new(&tmp);
    assert_eq!(cfg.appearance.theme_name, "Initial");

    // Write config.updating marker -> edit appearance.json -> poll_reload must not reload mid-transaction
    std::fs::write(tmp.join("config.updating"), b"").unwrap();
    std::fs::write(
        tmp.join("appearance.json"),
        r#"{"selectedId":"default","defaultTheme":{"name":"MidTx"}}"#,
    )
    .unwrap();
    assert!(!rel.poll_reload(&mut cfg));
    assert_eq!(cfg.appearance.theme_name, "Initial");

    // Complete edit & remove marker -> next poll_reload applies completed snapshot
    std::fs::write(
        tmp.join("appearance.json"),
        r#"{"selectedId":"default","defaultTheme":{"name":"Final"}}"#,
    )
    .unwrap();
    std::fs::remove_file(tmp.join("config.updating")).unwrap();
    assert!(rel.poll_reload(&mut cfg));
    assert_eq!(cfg.appearance.theme_name, "Final");

    // 1b. Same-length atomic replacement of appearance.json even when mosaic.json has a strictly future mtime
    std::fs::write(tmp.join("mosaic.json"), r#"{"tileTypes":[12,0,0,0]}"#).unwrap();
    let future = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(tmp.join("mosaic.json"))
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(future))
        .unwrap();
    assert!(rel.poll_reload(&mut cfg));
    let app_tmp = tmp.join("appearance.json.tmp");
    std::fs::write(
        &app_tmp,
        r#"{"selectedId":"default","defaultTheme":{"name":"Atom!"}}"#,
    )
    .unwrap();
    std::fs::rename(&app_tmp, tmp.join("appearance.json")).unwrap();
    assert!(
        std::fs::metadata(tmp.join("appearance.json"))
            .unwrap()
            .modified()
            .unwrap()
            < std::fs::metadata(tmp.join("mosaic.json"))
                .unwrap()
                .modified()
                .unwrap()
    );
    assert!(
        rel.poll_reload(&mut cfg),
        "same-length atomic rename must trigger reload even when mosaic.json mtime is newer"
    );
    assert_eq!(cfg.appearance.theme_name, "Atom!");

    // 2. Sparse Mosaic [Weather, None, Clock, None] keeps Clock in slot 2 (not shifted to slot 1)
    let mut d = DisplayPreview::new(1920, 1080);
    d.set_custom_bg_rgba(&[0, 0, 0, 255], 1, 1).unwrap();
    let sparse_m = r#"{"tileTypes":[15,0,12,0],"tileConfigs":[{"city":"Tokyo"},{},{},{}]}"#;
    let shifted_m = r#"{"tileTypes":[15,12,0,0],"tileConfigs":[{"city":"Tokyo"},{},{},{}]}"#;
    let no_dim_app = r#"{"selectedId":"default","defaultTheme":{"backgroundImageDimming":0,"moduleBackgroundDimming":0}}"#;
    d.set_config_json(Some(no_dim_app), None, None, Some(sparse_m), None)
        .unwrap();
    let f_sparse = d.render_rgba(1763414400, 0, false);
    d.set_config_json(Some(no_dim_app), None, None, Some(shifted_m), None)
        .unwrap();
    let f_shifted = d.render_rgba(1763414400, 0, false);
    assert_ne!(
        f_sparse, f_shifted,
        "slot 2 Clock must stay at slot 2 column when slot 1 is empty"
    );

    // Check slot 1 region (x ~ 550..850, y ~ 40..200) is untouched pure black in f_sparse
    let mut slot1_nonblack = 0;
    for y in 40..200usize {
        for x in 550..850usize {
            let off = (y * 1920 + x) * 4;
            if f_sparse[off] > 0 || f_sparse[off + 1] > 0 || f_sparse[off + 2] > 0 {
                slot1_nonblack += 1;
            }
        }
    }
    assert_eq!(
        slot1_nonblack, 0,
        "empty slot 1 must remain completely transparent/black"
    );

    // 3. WorldClock ignores host TZ; the CLI and DisplayPreview render identical PPMs.
    let wc_m1 = r#"{"tileTypes":[14,0,0,0],"tileConfigs":[{"city":"Tokyo","utcOffsetMinutes":540},{},{},{}]}"#;
    let wc_m2 = r#"{"tileTypes":[14,0,0,0],"tileConfigs":[{"city":"London","utcOffsetMinutes":0},{},{},{}]}"#;
    d.set_config_json(Some(no_dim_app), None, None, Some(wc_m1), None)
        .unwrap();
    let wc_tokyo = d.render_rgba(1763414400, 0, false);
    d.set_config_json(Some(no_dim_app), None, None, Some(wc_m2), None)
        .unwrap();
    let wc_london = d.render_rgba(1763414400, 0, false);
    assert_ne!(wc_tokyo, wc_london);
    let wc_other_day = d.render_rgba(1763414400 + 86400 * 3, 0, false);
    assert_ne!(wc_london, wc_other_day);

    let disp_bin = std::path::PathBuf::from(env!("CARGO_BIN_EXE_openflux-display"));
    assert!(
        disp_bin.is_file(),
        "CARGO_BIN_EXE_openflux-display missing: {}",
        disp_bin.display()
    );
    // Black PPM background with zero dimming isolates Mosaic rendering from gradient background
    std::fs::write(tmp.join("black.ppm"), b"P6\n1 1\n255\n\x00\x00\x00").unwrap();
    let black_app = r#"{"selectedId":"default","defaultTheme":{"backgroundImagePath":"black.ppm","backgroundImageDimming":0,"moduleBackgroundDimming":0}}"#;
    std::fs::write(tmp.join("appearance.json"), black_app).unwrap();
    std::fs::write(tmp.join("mosaic.json"), wc_m1).unwrap();
    let p_la = tmp.join("wc_la.ppm");
    let p_tyo = tmp.join("wc_tyo.ppm");
    assert!(std::process::Command::new(&disp_bin)
        .args([
            "--config-dir",
            tmp.to_str().unwrap(),
            "--media-dir",
            tmp.to_str().unwrap(),
            "--render-ppm",
            p_la.to_str().unwrap()
        ])
        .env("TZ", "America/Los_Angeles")
        .env("OPENFLUX_FIXED_TIME_SEC", "1763414400")
        .status()
        .unwrap()
        .success());
    assert!(std::process::Command::new(&disp_bin)
        .args([
            "--config-dir",
            tmp.to_str().unwrap(),
            "--media-dir",
            tmp.to_str().unwrap(),
            "--render-ppm",
            p_tyo.to_str().unwrap()
        ])
        .env("TZ", "Asia/Tokyo")
        .env("OPENFLUX_FIXED_TIME_SEC", "1763414400")
        .status()
        .unwrap()
        .success());
    assert_eq!(
        std::fs::read(&p_la).unwrap(),
        std::fs::read(&p_tyo).unwrap(),
        "WorldClock output changed with host TZ"
    );

    let mut prev = DisplayPreview::new(1920, 1080);
    prev.load_config_dir(tmp.to_str().unwrap(), tmp.to_str().unwrap())
        .unwrap();
    let lib_ppm = prev.render_ppm_bytes(1763414400, 0, false);
    assert_eq!(
        lib_ppm,
        std::fs::read(&p_la).unwrap(),
        "DisplayPreview and openflux-display CLI PPMs differ"
    );
    let hdr_end = lib_ppm
        .iter()
        .take(32)
        .enumerate()
        .filter(|&(_, &b)| b == b'\n')
        .nth(2)
        .map(|(i, _)| i + 1)
        .unwrap();
    let slot0_lit = (40..240usize).any(|y| {
        (15..410usize).any(|x| {
            let o = hdr_end + (y * 1920 + x) * 3;
            lib_ppm[o] > 40 || lib_ppm[o + 1] > 40 || lib_ppm[o + 2] > 40
        })
    });
    assert!(
        slot0_lit,
        "expected WorldClock slot 0 [8..417] to paint pixels on black background"
    );
    let slots_1_3_lit = (40..240usize).any(|y| {
        (506..1912usize).any(|x| {
            let o = hdr_end + (y * 1920 + x) * 3;
            lib_ppm[o] > 0 || lib_ppm[o + 1] > 0 || lib_ppm[o + 2] > 0
        })
    });
    assert!(
        !slots_1_3_lit,
        "empty slots 1..3 [506..1912] must stay pure black"
    );

    // 4. Pre-decode image bounds guard loads valid BMP & rejects 5000x5000 BMP header before allocating ~100 MiB
    let make_bmp = |w: u32, h: u32, px: Option<&[u8]>| -> Vec<u8> {
        let px_bytes = px.map(|p| p.len()).unwrap_or(0) as u32;
        let mut b = vec![0u8; 54];
        b[0..2].copy_from_slice(b"BM");
        b[2..6].copy_from_slice(&(54 + px_bytes).to_le_bytes());
        b[10..14].copy_from_slice(&54u32.to_le_bytes());
        b[14..18].copy_from_slice(&40u32.to_le_bytes());
        b[18..22].copy_from_slice(&w.to_le_bytes());
        b[22..26].copy_from_slice(&h.to_le_bytes());
        b[26..28].copy_from_slice(&1u16.to_le_bytes());
        b[28..30].copy_from_slice(&24u16.to_le_bytes());
        if let Some(p) = px {
            b.extend_from_slice(p);
        }
        b
    };
    let ok_bmp_path = tmp.join("ok.bmp");
    std::fs::write(
        &ok_bmp_path,
        make_bmp(1, 1, Some(&[0x10, 0x20, 0x30, 0x00])),
    )
    .unwrap();
    assert!(
        prev.renderer
            .assets
            .load_image(None, ok_bmp_path.to_str().unwrap())
            .is_some(),
        "valid 1x1 BMP must load"
    );
    let huge_bmp_path = tmp.join("huge.bmp");
    std::fs::write(&huge_bmp_path, make_bmp(5000, 5000, None)).unwrap();
    assert!(
        prev.renderer
            .assets
            .load_image(None, huge_bmp_path.to_str().unwrap())
            .is_none(),
        "5000x5000 BMP header must be rejected before allocating pixel buffers"
    );

    // 4b. Custom key icons in PNG, JPEG, BMP, and SVG all render graphics onto key #0
    let key_icons_dir = tmp.join("key_icons");
    std::fs::create_dir_all(&key_icons_dir).unwrap();
    let img_16 = image::RgbaImage::from_pixel(16, 16, image::Rgba([255, 0, 128, 255]));
    img_16.save(key_icons_dir.join("icon.png")).unwrap();
    image::DynamicImage::ImageRgba8(img_16.clone())
        .to_rgb8()
        .save(key_icons_dir.join("icon.jpg"))
        .unwrap();
    std::fs::write(
        key_icons_dir.join("icon.bmp"),
        make_bmp(2, 2, Some(&[255u8; 2 * 2 * 4])),
    )
    .unwrap();
    std::fs::write(
        key_icons_dir.join("icon.svg"),
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><g transform="translate(2,2)"><path id="p1" d="M0 0 H20 V20 H0 Z" fill="#ff2244"/></g></svg>"##,
    ).unwrap();
    // Stroked-only SVG with fill="none" must stay hollow in center and render colored stroke when retintIcons=false
    std::fs::write(
        key_icons_dir.join("ring.svg"),
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="9" fill="none" stroke="#ff3300" stroke-width="3"/></svg>"##,
    ).unwrap();
    let mut icon_d = DisplayPreview::new(1920, 1080);
    icon_d.set_custom_bg_rgba(&[0, 0, 0, 255], 1, 1).unwrap();
    let base_app = serde_json::json!({
        "selectedId": "default",
        "defaultTheme": {
            "backgroundImagePath": "black.ppm",
            "backgroundImageDimming": 0,
            "moduleBackgroundDimming": 0,
            "legendColor": 0xFF00FF00u32,
            "retintIcons": true,
            "stylemaps": { "0": { "styles": { "0": { "label": "X" } } } }
        }
    });
    icon_d
        .set_config_json(
            Some(&base_app.to_string()),
            Some(r#"{"parallaxActive":false}"#),
            None,
            Some(r#"{"tileTypes":[0,0,0,0]}"#),
            Some(tmp.to_str().unwrap()),
        )
        .unwrap();
    let k0 = icon_d.get_key_rects()[0];
    let cx = (k0.x + k0.w * 0.5) as usize;
    let cy = (k0.y + k0.h * 0.5) as usize;
    // With a solid 20/24 filled icon, the top-left corner of the icon box is lit (unlike text "X")
    let corner_x = (k0.x + k0.w * 0.5 - 12.0) as usize;
    let corner_y = (k0.y + k0.h * 0.5 - 12.0) as usize;
    for ext in ["icon.png", "icon.jpg", "icon.bmp", "icon.svg"] {
        let app_i = serde_json::json!({
            "selectedId": "default",
            "defaultTheme": {
                "backgroundImagePath": "black.ppm",
                "backgroundImageDimming": 0,
                "moduleBackgroundDimming": 0,
                "legendColor": 0xFF00FF00u32,
                "retintIcons": true,
                "stylemaps": { "0": { "styles": { "0": { "icon": ext, "label": "MISSING" } } } }
            }
        });
        icon_d
            .set_config_json(
                Some(&app_i.to_string()),
                Some(r#"{"parallaxActive":false}"#),
                None,
                Some(r#"{"tileTypes":[0,0,0,0]}"#),
                Some(tmp.to_str().unwrap()),
            )
            .unwrap();
        let f_i = icon_d.render_rgba(1763414400, 0, false);
        let p_center = &f_i[(cy * 1920 + cx) * 4..(cy * 1920 + cx) * 4 + 4];
        let p_corner = &f_i[(corner_y * 1920 + corner_x) * 4..(corner_y * 1920 + corner_x) * 4 + 4];
        assert!(
            p_center[1] > 180 || p_corner[1] > 180,
            "custom key icon {ext} failed to render pixels on key 0"
        );
    }
    // Verify SVG color preservation (retintIcons=false) and hollow center on fill="none" stroked circle
    let app_ring = serde_json::json!({
        "selectedId": "default",
        "defaultTheme": {
            "backgroundImagePath": "black.ppm",
            "backgroundImageDimming": 0,
            "moduleBackgroundDimming": 0,
            "legendColor": 0xFF00FF00u32,
            "retintIcons": false,
            "stylemaps": { "0": { "styles": { "0": { "icon": "ring.svg" } } } }
        }
    });
    icon_d
        .set_config_json(
            Some(&app_ring.to_string()),
            Some(r#"{"parallaxActive":false}"#),
            None,
            Some(r#"{"tileTypes":[0,0,0,0]}"#),
            Some(tmp.to_str().unwrap()),
        )
        .unwrap();
    let f_ring = icon_d.render_rgba(1763414400, 0, false);
    let c_px = &f_ring[(cy * 1920 + cx) * 4..(cy * 1920 + cx) * 4 + 4];
    assert_eq!(
        c_px[0], 0,
        "fill=none SVG circle center must stay transparent"
    );
    let has_orange_stroke = ((cy - 28)..=(cy + 28)).any(|yy| {
        ((cx - 28)..=(cx + 28)).any(|xx| {
            let p = &f_ring[(yy * 1920 + xx) * 4..(yy * 1920 + xx) * 4 + 4];
            p[0] > 180 && p[2] < 40
        })
    });
    assert!(
        has_orange_stroke,
        "retintIcons=false stroked SVG must preserve its #ff3300 color"
    );

    // External <image href="/abs/path/icon.svg"/> inside SVG must not read local filesystem
    let ext_svg = key_icons_dir.join("ext.svg");
    std::fs::write(
        &ext_svg,
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><image href="{}" x="0" y="0" width="24" height="24"/></svg>"##,
            key_icons_dir.join("icon.svg").display()
        ),
    ).unwrap();
    let loaded_ext = prev
        .renderer
        .assets
        .load_image(None, ext_svg.to_str().unwrap())
        .unwrap();
    assert!(
        loaded_ext.rgba.chunks_exact(4).all(|p| p[3] == 0),
        "external <image href=...> in SVG must not resolve local file bytes"
    );

    // 5. Headless target-frame stepping works without integer rounding freeze at 10, 24, and 60 FPS
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let step_dec = tmp.join("step_dec.py");
        std::fs::write(
            &step_dec,
            "#!/usr/bin/env python3\nimport sys\nw, h = int(sys.argv[2]), int(sys.argv[3])\nfor i in range(6):\n    c = [(200,0,0),(0,200,0),(0,0,200),(200,200,0),(0,200,200),(200,0,200)][i]\n    sys.stdout.buffer.write(bytes(c)*(w*h))\n    sys.stdout.buffer.flush()\n",
        ).unwrap();
        std::fs::set_permissions(&step_dec, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(tmp.join("clip.mp4"), b"dummy").unwrap();
        std::fs::write(
            tmp.join("appearance.json"),
            r#"{"selectedId":"default","defaultTheme":{"backgroundImagePath":"clip.mp4","backgroundImageDimming":0}}"#,
        ).unwrap();
        std::fs::write(tmp.join("mosaic.json"), r#"{"tileTypes":[0,0,0,0]}"#).unwrap();
        for fps_s in ["10", "24", "60"] {
            let p_f3 = tmp.join(format!("f3_{fps_s}.ppm"));
            assert!(
                std::process::Command::new(&disp_bin)
                    .args([
                        "--config-dir",
                        tmp.to_str().unwrap(),
                        "--media-dir",
                        tmp.to_str().unwrap(),
                        "--render-ppm",
                        p_f3.to_str().unwrap()
                    ])
                    .env("OPENFLUX_VIDEO_DECODER", &step_dec)
                    .env("OPENFLUX_VIDEO_FPS", fps_s)
                    .env("OPENFLUX_VIDEO_TARGET_FRAME", "3")
                    .env("OPENFLUX_FIXED_TIME_SEC", "1763414400")
                    .status()
                    .unwrap()
                    .success(),
                "target frame 3 failed at {fps_s} fps"
            );
            let bytes = std::fs::read(&p_f3).unwrap();
            let off = bytes
                .iter()
                .take(32)
                .enumerate()
                .filter(|&(_, &b)| b == b'\n')
                .nth(2)
                .map(|(i, _)| i + 1)
                .unwrap();
            // Frame 3 (1-based) = index 2 = blue (0, 0, 200)
            assert!(
                bytes[off + 2] > 180 && bytes[off] < 20 && bytes[off + 1] < 20,
                "fps={fps_s} got {:?}",
                &bytes[off..off + 3]
            );
        }
        // Short 6-frame video requesting target frame 7 must fail nonzero and write no PPM
        let p_f7 = tmp.join("f7_short.ppm");
        let st7 = std::process::Command::new(&disp_bin)
            .args([
                "--config-dir",
                tmp.to_str().unwrap(),
                "--media-dir",
                tmp.to_str().unwrap(),
                "--render-ppm",
                p_f7.to_str().unwrap(),
            ])
            .env("OPENFLUX_VIDEO_DECODER", &step_dec)
            .env("OPENFLUX_VIDEO_FPS", "24")
            .env("OPENFLUX_VIDEO_TARGET_FRAME", "7")
            .env("OPENFLUX_FIXED_TIME_SEC", "1763414400")
            .status()
            .unwrap();
        assert!(
            !st7.success(),
            "short 6-frame video must fail when target frame 7 requested"
        );
        assert!(
            !p_f7.exists(),
            "failed target-frame render must not write partial PPM"
        );
    }

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn autofocus_lock_reticle_and_zero_effective_alpha_blend_noop() {
    // 1. Preserve no-op when effective alpha rounds to zero on non-opaque destination
    let mut non_opaque = 0x1020_3040_u32;
    openflux_display::assets::blend_argb_cov(&mut non_opaque, 0x01FF_0000, 1);
    assert_eq!(
        non_opaque, 0x1020_3040,
        "effective alpha=0 must not alter destination alpha or RGB"
    );

    // 2. AutofocusLock renders corner reticle brackets around pressed key neighborhood & animates across stages
    let mut d = DisplayPreview::new(1920, 1080);
    let app = serde_json::json!({
        "selectedId": "default",
        "defaultTheme": {
            "selectedKeyEffect": "autofocus_lock",
            "ringGlowColor": 0xFF2D_D4BF_u32,
            "backgroundImageDimming": 100
        }
    });
    d.set_config_json(Some(&app.to_string()), None, None, None, None)
        .unwrap();
    let idle = d.render_rgba(1763414400, 100, false).to_vec();
    let mut bm = [0u8; 32];
    bm[52 / 8] |= 1 << (52 % 8);
    let hex_bm: String = bm.iter().map(|b| format!("{b:02x}")).collect();
    let zero_bm = "00".repeat(32);
    let press_msg = format!(r#"{{"type":"key_state","key_bitmap":"{hex_bm}"}}"#);
    let rel_msg = format!(r#"{{"type":"key_state","key_bitmap":"{zero_bm}"}}"#);
    d.apply_live_json(&press_msg, 100).unwrap();
    assert!(d.state.is_key_pressed(52), "key 52 must be marked pressed");
    let early = d.render_rgba(1763414400, 140, false).to_vec();
    let locked = d.render_rgba(1763414400, 345, false).to_vec();

    // Expect distinct animation stages (horizontal scan/outer corners at +40ms vs locked reticle at +245ms)
    let diff_early = idle
        .chunks_exact(4)
        .zip(early.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    let diff_locked = idle
        .chunks_exact(4)
        .zip(locked.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    let diff_stages = early
        .chunks_exact(4)
        .zip(locked.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    assert!(
        diff_early > 2000,
        "early autofocus scan should illuminate key + reticle, got {diff_early}"
    );
    assert!(
        diff_locked > 2000,
        "locked autofocus brackets should illuminate 3x3 reticle, got {diff_locked}"
    );
    assert!(
        diff_stages > 1000,
        "early scan and locked brackets must animate distinctly, got {diff_stages}"
    );

    // 3. Short 50ms tap must be 100% continuous at release
    let mut tap_d = DisplayPreview::new(1920, 1080);
    tap_d
        .set_config_json(Some(&app.to_string()), None, None, None, None)
        .unwrap();
    tap_d.apply_live_json(&press_msg, 100).unwrap();
    let held_150 = tap_d.render_rgba(1763414400, 150, false).to_vec();
    tap_d.apply_live_json(&rel_msg, 150).unwrap();
    let rel_150 = tap_d.render_rgba(1763414400, 150, false).to_vec();
    assert_eq!(
        held_150, rel_150,
        "at exact release timestamp t=150 (rel_age=0, glow=1), effect must be identical with zero age jump"
    );
    let tap_260 = tap_d.render_rgba(1763414400, 260, false).to_vec();
    assert_ne!(
        rel_150, tap_260,
        "released tap must continue animating smoothly"
    );
}

#[test]
fn pomodoro_keys_cmd_shortcut_overlay_and_ripple_phase_work_end_to_end() {
    let mut d = DisplayPreview::new(1920, 1080);
    let mut actions = vec![serde_json::json!({"type":0,"keyCode":"KC_A"}); 85];
    actions[0] = serde_json::json!({"type":0,"keyCode":"KC_LEFT_GUI"}); // Cmd on key 0
    actions[1] = serde_json::json!({"type":0,"keyCode":"KC_C"}); // C on key 1
    actions[2] = serde_json::json!({"type":0,"keyCode":"KC_V"}); // V on key 2
    actions[3] = serde_json::json!({"type":0,"keyCode":"KC_RIGHT_GUI"}); // Right Cmd on key 3
    actions[79] = serde_json::json!({"type":2,"targetLayer":1,"layerControlType":"momentaryLayer","keyCode":"MO(1)"}); // Fn on key 79
    actions[80] = serde_json::json!({"type":0,"keyCode":"KC_OF_RESET_POMODORO"});
    actions[81] = serde_json::json!({"type":0,"keyCode":"KC_OF_START_PAUSE_POMODORO"});
    let mut layer1_actions = vec![serde_json::json!({"type":0,"keyCode":"KC_TRANSPARENT"}); 85];
    layer1_actions[82] = serde_json::json!({"type":0,"keyCode":"KC_OF_START_PAUSE_POMODORO"});
    let km = serde_json::json!({
        "selectedFrameLayoutType": "ansi",
        "keymaps": { "ansi": { "defaultLayer": 0, "layers": [{ "keyActions": actions }, { "keyActions": layer1_actions }] } }
    });
    // Pomodoro tile (type 13) with a 10-minute duration.
    let mos = serde_json::json!({
        "tileTypes": [13, 0, 0, 0],
        "tileConfigs": [{"main": 10}, {}, {}, {}]
    });
    let app_no_fx = serde_json::json!({
        "selectedId": "default",
        "defaultTheme": { "selectedKeyEffect": "none", "ringGlowColor": 0xFF2D_D4BFu32, "backgroundImageDimming": 100 }
    });
    let set_json = serde_json::json!({ "timeout": 30 });
    d.set_config_json(
        Some(&app_no_fx.to_string()),
        Some(&set_json.to_string()),
        Some(&km.to_string()),
        Some(&mos.to_string()),
        None,
    )
    .unwrap();
    assert_eq!(d.state.pomo_remaining_s, 600);

    // 1. Batched [Fn+Pomo(L1) pressed, repeated, released] via d.state.apply_live_json directly must preserve press-time layer
    let mut bm = [0u8; 32];
    bm[79 / 8] |= 1 << (79 % 8);
    bm[82 / 8] |= 1 << (82 % 8);
    let hex_pomo: String = bm.iter().map(|b| format!("{b:02x}")).collect();
    let zero_bm = "00".repeat(32);
    d.state
        .apply_live_json(
            &format!(r#"{{"type":"key_state","key_bitmap":"{hex_pomo}"}}"#),
            1000,
        )
        .unwrap();
    d.state
        .apply_live_json(
            &format!(r#"{{"type":"key_state","key_bitmap":"{hex_pomo}"}}"#),
            1005,
        )
        .unwrap();
    d.state
        .apply_live_json(
            &format!(r#"{{"type":"key_state","key_bitmap":"{zero_bm}"}}"#),
            1010,
        )
        .unwrap();
    assert!(d.state.handle_key_press_actions(&d.cfg, 1020));
    assert!(d.state.pomo_running, "batched Fn-layer [pressed, released] must preserve press-time layer and toggle Pomodoro on");

    d.state.tick_pomo(4020);
    assert_eq!(d.state.pomo_remaining_s, 597);
    // Repeated same cmdSeq must not reset countdown anchor
    d.apply_live_json(
        r#"{"pomodoro":{"action":"toggle","defaultS":600,"cmdSeq":7}}"#,
        4100,
    )
    .unwrap();
    assert!(!d.state.pomo_running, "first cmdSeq=7 toggles off");
    d.apply_live_json(
        r#"{"pomodoro":{"action":"toggle","defaultS":600,"cmdSeq":7}}"#,
        4200,
    )
    .unwrap();
    assert!(
        !d.state.pomo_running,
        "duplicate cmdSeq=7 must be ignored cleanly"
    );
    d.apply_live_json(
        r#"{"pomodoro":{"action":"toggle","defaultS":600,"cmdSeq":8}}"#,
        4300,
    )
    .unwrap();
    assert!(
        d.state.pomo_running,
        "new cmdSeq=8 resumes countdown cleanly"
    );
    d.state.tick_pomo(7300);
    assert_eq!(d.state.pomo_remaining_s, 594);

    let mut bm_rst = [0u8; 32];
    bm_rst[80 / 8] |= 1 << (80 % 8);
    let hex_rst: String = bm_rst.iter().map(|b| format!("{b:02x}")).collect();
    d.apply_live_json(
        &format!(r#"{{"type":"key_state","key_bitmap":"{hex_rst}"}}"#),
        7200,
    )
    .unwrap();
    d.state.handle_key_press_actions(&d.cfg, 7200);
    assert!(!d.state.pomo_running);
    assert_eq!(d.state.pomo_remaining_s, 600);
    d.apply_live_json(
        &format!(r#"{{"type":"key_state","key_bitmap":"{zero_bm}"}}"#),
        7300,
    )
    .unwrap();

    // 2. With key effects disabled, holding Left Cmd or Right Cmd changes key 1 ("KC_C" -> "Copy") and restores on release
    let idle_px = d.render_rgba(1763414400, 8000, false).to_vec();
    let k1 = openflux_display::layout::build_physical_layout(
        openflux_display::layout::FrameLayoutType::Ansi,
    )
    .keys
    .into_iter()
    .find(|k| k.key_index == 1)
    .unwrap();
    let key1_rect = |frame: &[u8]| -> Vec<u8> {
        let mut out = Vec::new();
        let x0 = (k1.x as usize).min(1919);
        let x1 = ((k1.x + k1.w).ceil() as usize).min(1920);
        let y0 = (k1.y as usize).min(1079);
        let y1 = ((k1.y + k1.h).ceil() as usize).min(1080);
        for y in y0..y1 {
            let row = &frame[y * 1920 * 4..(y + 1) * 1920 * 4];
            out.extend_from_slice(&row[x0 * 4..x1 * 4]);
        }
        out
    };
    let idle_k1 = key1_rect(&idle_px);
    let mut bm_rcmd = [0u8; 32];
    bm_rcmd[0] |= 1 << 3;
    let hex_rcmd: String = bm_rcmd.iter().map(|b| format!("{b:02x}")).collect();
    d.apply_live_json(
        &format!(r#"{{"type":"key_state","key_bitmap":"{hex_rcmd}"}}"#),
        8100,
    )
    .unwrap();
    assert_eq!(d.state.held_modifier_mask(&d.cfg.keymap), 8);
    let cmd_px = d.render_rgba(1763414400, 8150, false).to_vec();
    assert_ne!(
        idle_k1,
        key1_rect(&cmd_px),
        "Right Cmd must change key 1 legend rect even with effects disabled"
    );
    d.apply_live_json(
        &format!(r#"{{"type":"key_state","key_bitmap":"{zero_bm}"}}"#),
        8200,
    )
    .unwrap();
    let post_rel_px = d.render_rgba(1763414400, 9000, false).to_vec();
    assert_eq!(
        idle_px, post_rel_px,
        "releasing Cmd must restore exact idle frame"
    );

    // Switching pack -> empty array resets cleanly back to default macOS overlays
    d.apply_live_json(
        r#"{"shortcutOverlays":[{"mods":8,"keyCode":"KC_C","label":"Brush"}]}"#,
        9050,
    )
    .unwrap();
    assert_eq!(d.state.shortcut_overlays[0].label, "Brush");
    d.apply_live_json(r#"{"shortcutOverlays":[]}"#, 9060)
        .unwrap();
    assert!(d
        .state
        .shortcut_overlays
        .iter()
        .any(|e| e.key_code == "KC_C" && e.label == "Copy"));

    // 3. Ripple Pulse: no solid key fill on press, two rapid taps spawn independent waves that continue past 520ms, then expire cleanly after 2140ms
    let app_ripple = serde_json::json!({
        "selectedId": "default",
        "defaultTheme": { "selectedKeyEffect": "ripple", "ringGlowColor": 0xFF2D_D4BFu32, "backgroundImageDimming": 100 }
    });
    d.set_config_json(
        Some(&app_ripple.to_string()),
        Some(&set_json.to_string()),
        Some(&km.to_string()),
        Some(&mos.to_string()),
        None,
    )
    .unwrap();
    d.state.ripple_pulses.clear();
    let ripple_idle = d.render_rgba(1763414400, 9900, false).to_vec();
    let mut bm_k2 = [0u8; 32];
    bm_k2[0] |= 1 << 2;
    let hex_k2: String = bm_k2.iter().map(|b| format!("{b:02x}")).collect();
    // Tap 1 at t=10_000, release at 10_040, Tap 2 at 10_220, release at 10_260
    d.apply_live_json(
        &format!(r#"{{"type":"key_state","key_bitmap":"{hex_k2}"}}"#),
        10_000,
    )
    .unwrap();
    d.apply_live_json(
        &format!(r#"{{"type":"key_state","key_bitmap":"{zero_bm}"}}"#),
        10_040,
    )
    .unwrap();
    d.apply_live_json(
        &format!(r#"{{"type":"key_state","key_bitmap":"{hex_k2}"}}"#),
        10_220,
    )
    .unwrap();
    d.apply_live_json(
        &format!(r#"{{"type":"key_state","key_bitmap":"{zero_bm}"}}"#),
        10_260,
    )
    .unwrap();
    assert_eq!(
        d.state.ripple_pulses.len(),
        2,
        "two rapid taps must spawn two independent ripple pulses"
    );
    let r_800 = d.render_rgba(1763414400, 10_850, false).to_vec();
    let r_1100 = d.render_rgba(1763414400, 11_150, false).to_vec();
    assert_ne!(
        r_800, ripple_idle,
        "waves must continue animating past 520ms after release"
    );
    assert_ne!(
        r_800, r_1100,
        "waves must keep expanding cleanly between 850ms and 1150ms"
    );
    let r_1800 = d.render_rgba(1763414400, 11_800, false).to_vec();
    assert_ne!(
        r_1800, ripple_idle,
        "tap 2 at 10220ms must still be expanding gently at 11800ms (2100ms wave)"
    );
    let r_expired = d.render_rgba(1763414400, 12_450, false).to_vec();
    assert_eq!(
        r_expired, ripple_idle,
        "frame must return cleanly to idle once all ripple pulses expire"
    );

    // 4. Display sleep timeout ignores unchanged telemetry and wakes on key transition or host monitor wake
    d.apply_live_json(
        &format!(r#"{{"type":"key_state","key_bitmap":"{zero_bm}"}}"#),
        10_500,
    )
    .unwrap();
    assert!(!d.state.is_display_asleep(&d.cfg, 20_000));
    // Sending unchanged telemetry must not extend deadline
    d.apply_live_json(
        &format!(r#"{{"type":"key_state","key_bitmap":"{zero_bm}"}}"#),
        35_000,
    )
    .unwrap();
    assert!(
        d.state.is_display_asleep(&d.cfg, 41_000),
        "inactivity timeout must expire under unchanged packets"
    );
    // Real key press wakes keyboard
    d.apply_live_json(
        &format!(r#"{{"type":"key_state","key_bitmap":"{hex_k2}"}}"#),
        41_500,
    )
    .unwrap();
    assert!(!d.state.is_display_asleep(&d.cfg, 42_000));
    // Host monitor sleep syncs immediately and wakes cleanly
    d.apply_live_json(r#"{"hostDisplayAsleep":true}"#, 43_000)
        .unwrap();
    assert!(d.state.is_display_asleep(&d.cfg, 43_100));
    d.apply_live_json(r#"{"hostDisplayAsleep":false}"#, 60_000)
        .unwrap();
    assert!(!d.state.is_display_asleep(&d.cfg, 60_500));
}

#[test]
fn procedural_shaders_and_lock_indicators_render_distinctly() {
    let mut d = DisplayPreview::new(1920, 1080);
    let mut actions = vec![serde_json::json!({"type":0,"keyCode":"KC_A"}); 85];
    actions[31] = serde_json::json!({"type":0,"keyCode":"KC_CAPS_LOCK"});
    let km = serde_json::json!({
        "selectedFrameLayoutType": "ansi",
        "keymaps": { "ansi": { "defaultLayer": 0, "layers": [{ "keyActions": actions }] } }
    });
    let app_t0 = serde_json::json!({
        "selectedId": "default",
        "defaultTheme": {
            "backgroundImagePath": "aurora_drift.frag",
            "backgroundShaderParams": { "hue": 0.0, "speed": 1.0 },
            "lockIndicatorStyle": "dot",
            "lockIndicatorColor": 0xFFFF_3355u32,
            "backgroundImageDimming": 0
        }
    });
    d.set_config_json(
        Some(&app_t0.to_string()),
        None,
        Some(&km.to_string()),
        None,
        None,
    )
    .unwrap();
    let f_t0 = d.render_rgba(1763414400, 0, false).to_vec();
    let f_t2 = d.render_rgba(1763414400, 2000, false).to_vec();
    assert_ne!(
        f_t0, f_t2,
        "procedural shader must animate cleanly over time"
    );

    let app_hue = serde_json::json!({
        "selectedId": "default",
        "defaultTheme": {
            "backgroundImagePath": "aurora_drift.frag",
            "backgroundShaderParams": { "hue": 120.0, "speed": 1.0 },
            "lockIndicatorStyle": "dot",
            "lockIndicatorColor": 0xFFFF_3355u32,
            "backgroundImageDimming": 0
        }
    });
    d.set_config_json(
        Some(&app_hue.to_string()),
        None,
        Some(&km.to_string()),
        None,
        None,
    )
    .unwrap();
    let f_hue = d.render_rgba(1763414400, 0, false).to_vec();
    assert_ne!(f_t0, f_hue, "shader hue rotation must visibly shift colors");

    // Enabling caps_lock must illuminate Lock Key Indicator on Caps Lock key (#31)
    let f_nocaps = d.render_rgba(1763414400, 3000, false).to_vec();
    d.apply_live_json(r#"{"type":"key_state","caps_lock":true}"#, 3000)
        .unwrap();
    let f_caps = d.render_rgba(1763414400, 3000, false).to_vec();
    assert_ne!(
        f_nocaps, f_caps,
        "caps_lock must render lock indicator dot on Caps Lock key"
    );

    // Setting custom RGBA background overrides shader without erasing bg_path, and clear_custom_bg resumes shader
    let mut ds = DisplayPreview::new(1920, 1080);
    let shader_app = serde_json::json!({
        "selectedId": "default",
        "defaultTheme": {
            "backgroundImagePath": "aurora_drift.frag",
            "backgroundImageDimming": 0
        }
    });
    ds.set_config_json(Some(&shader_app.to_string()), None, None, None, None)
        .unwrap();
    let f_shader = ds.render_rgba(1763414400, 500, false).to_vec();
    ds.set_custom_bg_rgba(&[210, 10, 10, 255], 1, 1).unwrap();
    assert_eq!(ds.cfg.appearance.bg_path, "aurora_drift.frag");
    let f_custom = ds.render_rgba(1763414400, 500, false).to_vec();
    assert_eq!(
        &f_custom[0..3],
        &[210, 10, 10],
        "custom RGBA must take precedence over configured shader"
    );
    ds.clear_custom_bg();
    assert_eq!(ds.cfg.appearance.bg_path, "aurora_drift.frag");
    let f_resumed = ds.render_rgba(1763414400, 500, false).to_vec();
    assert_eq!(
        f_shader, f_resumed,
        "clear_custom_bg must resume configured procedural shader"
    );
}

#[test]
fn vram_clears_when_omitted_weather_renders_forecast_and_shortcuts_have_no_forced_box() {
    let mut d = DisplayPreview::new(1920, 1080);
    let no_dim_app = serde_json::json!({
        "selectedId": "default",
        "defaultTheme": {
            "backgroundImageDimming": 0,
            "moduleBackgroundDimming": 0
        }
    });
    let mosaic_hw_wx = serde_json::json!({
        "tileTypes": [8, 15, 0, 0],
        "tileConfigs": [{}, {"city": "Tokyo", "celcius": true}, {}, {}]
    });
    d.set_config_json(
        Some(&no_dim_app.to_string()),
        None,
        None,
        Some(&mosaic_hw_wx.to_string()),
        None,
    )
    .unwrap();

    // 1. Discrete VRAM populates -> missing/null VRAM clears previously known VRAM
    d.apply_live_json(r#"{"hardwareData":{"cpu":25,"memory":8.0,"totalMemory":32,"gpuUsage":60,"vramUsed":6.0,"totalVram":8.0,"gpuUnified":false,"netDown":"1 MB/s","netUp":"0 MB/s"}}"#, 1000).unwrap();
    assert_eq!(d.state.vram_used_gb, Some(6.0));
    assert_eq!(d.state.vram_total_gb, Some(8.0));
    let f_vram = d.render_rgba(1763414400, 1000, false).to_vec();

    d.apply_live_json(r#"{"hardwareData":{"cpu":25,"memory":8.0,"totalMemory":32,"gpuUsage":60,"vramUsed":null,"totalVram":null,"gpuUnified":false,"netDown":"1 MB/s","netUp":"0 MB/s"}}"#, 2000).unwrap();
    assert_eq!(d.state.vram_used_gb, None);
    assert_eq!(d.state.vram_total_gb, None);
    let f_no_vram = d.render_rgba(1763414400, 2000, false).to_vec();
    assert_ne!(
        f_vram, f_no_vram,
        "clearing VRAM to null must update rendered Hardware Status cell"
    );

    // 2. Nighttime weather with sunrise/sunset/AQI and 3h forecast renders cleanly
    d.apply_live_json(r#"{"weather":{"city":"Tokyo","temp":12.4,"high":16.0,"low":8.0,"condition":"Partly Cloudy","isDay":false,"celsius":true,"ready":true,"sunrise":"6:21 AM","sunset":"4:33 PM","aqi":18,"utcOffsetSec":32400,"hourly":[{"hour":"12 AM","temp":11.8,"condition":"Partly Cloudy","isDay":false},{"hour":"1 AM","temp":11.2,"condition":"Clear","isDay":false},{"hour":"2 AM","temp":10.9,"condition":"Cloudy","isDay":false}]}}"#, 3000).unwrap();
    let f_wx_night = d.render_rgba(1763414400, 3000, false).to_vec();
    assert_ne!(
        f_no_vram, f_wx_night,
        "weather snapshot with sunrise/sunset/AQI/hourly must render into Weather slot"
    );

    // 3. Shortcut overlay must not paint a solid/translucent background fill behind the key
    d.set_custom_bg_rgba(&[0, 0, 0, 255], 1, 1).unwrap();
    let mut actions = vec![serde_json::json!({"type":0,"keyCode":"KC_A"}); 85];
    actions[34] = serde_json::json!({"type":0,"keyCode":"KC_C"});
    actions[66] = serde_json::json!({"type":0,"keyCode":"KC_LGUI"});
    let km = serde_json::json!({
        "selectedFrameLayoutType": "ansi",
        "keymaps": { "ansi": { "defaultLayer": 0, "layers": [{ "keyActions": actions }] } }
    });
    let no_px_settings = r#"{"parallaxActive":false,"displayOffsetX":0,"displayOffsetY":0}"#;
    d.set_config_json(
        Some(&no_dim_app.to_string()),
        Some(no_px_settings),
        Some(&km.to_string()),
        None,
        None,
    )
    .unwrap();
    let f_no_mod = d.render_rgba(1763414400, 3900, false).to_vec();
    let mut bm_cmd = [0u8; 32];
    bm_cmd[66 / 8] |= 1 << (66 % 8);
    let hex_cmd: String = bm_cmd.iter().map(|b| format!("{b:02x}")).collect();
    d.apply_live_json(
        &format!(r#"{{"type":"key_state","key_bitmap":"{hex_cmd}"}}"#),
        4000,
    )
    .unwrap();
    assert!(
        d.state
            .shortcut_overlays
            .iter()
            .any(|ov| ov.key_code == "KC_C"),
        "Cmd+Copy shortcut overlay must be active"
    );
    let f_sc = d.render_rgba(1763414400, 4000, false).to_vec();
    assert_ne!(
        f_sc, f_no_mod,
        "holding Cmd must change key 34 from normal C to Copy shortcut"
    );
    let kr_c = &d.renderer.phys_layout.keys[34];
    // Corner pixel inside key 34 (away from top-left legend & center icon) must remain pure black [0,0,0]
    let px_x = (kr_c.x as usize) + (kr_c.w as usize) - 6;
    let px_y = (kr_c.y as usize) + (kr_c.h as usize) - 6;
    let off = (px_y * 1920 + px_x) * 4;
    assert_eq!(
        &f_sc[off..off + 3],
        &[0, 0, 0],
        "shortcut key must not draw a forced background box"
    );

    // 4. FN-layer media and brightness keycodes render Lucide SVG icons by keycode;
    //    non-empty stylemap labels override icons, empty labels use the keycode default, and transparent keys inherit.
    let mut base_actions = vec![serde_json::json!({"type":0,"keyCode":"KC_A"}); 85];
    base_actions[1] = serde_json::json!({"type":0,"keyCode":"KC_AUDIO_VOL_UP"});
    let mut fn_actions = vec![serde_json::json!({"type":0,"keyCode":"KC_TRANSPARENT"}); 85];
    fn_actions[2] = serde_json::json!({"type":0,"keyCode":"KC_MEDIA_PLAY_PAUSE"});
    fn_actions[3] = serde_json::json!({"type":0,"keyCode":"KC_BRIGHTNESS_UP"});
    let km_fn = serde_json::json!({
        "selectedFrameLayoutType": "ansi",
        "keymaps": {
            "ansi": {
                "defaultLayer": 0,
                "layers": [
                    { "keyActions": base_actions },
                    { "keyActions": fn_actions }
                ]
            }
        }
    });
    let mut d_fn = DisplayPreview::new(1920, 1080);
    d_fn.set_custom_bg_rgba(&[0, 0, 0, 255], 1, 1).unwrap();
    d_fn.set_config_json(
        Some(&no_dim_app.to_string()),
        Some(no_px_settings),
        Some(&km_fn.to_string()),
        None,
        None,
    )
    .unwrap();
    d_fn.apply_live_json(r#"{"type":"key_state","active_layer_bitfield":2}"#, 1000)
        .unwrap();
    let f_fn = d_fn.render_rgba(1763414400, 1000, false).to_vec();

    // Explicit custom label "LOUD" on key 1 overrides builtin Lucide icon
    let app_custom_lbl = serde_json::json!({
        "selectedId": "default",
        "defaultTheme": {
            "backgroundImageDimming": 0,
            "moduleBackgroundDimming": 0,
            "stylemaps": { "1": { "styles": { "1": { "label": "LOUD" } } } }
        }
    });
    d_fn.set_config_json(
        Some(&app_custom_lbl.to_string()),
        Some(no_px_settings),
        Some(&km_fn.to_string()),
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        d_fn.cfg
            .appearance
            .stylemaps
            .get(&1)
            .and_then(|s| s.keys.get(&1))
            .and_then(|k| k.label.as_deref()),
        Some("LOUD")
    );
    let f_fn_loud = d_fn.render_rgba(1763414400, 1000, false).to_vec();
    assert_ne!(
        f_fn, f_fn_loud,
        "explicit stylemap label must override automatic keycode icon"
    );

    // Explicit empty label "" in stylemaps means clear/inherit default from keycode
    let app_blank_lbl = serde_json::json!({
        "selectedId": "default",
        "defaultTheme": {
            "backgroundImageDimming": 0,
            "moduleBackgroundDimming": 0,
            "stylemaps": { "1": { "styles": { "1": { "label": "" } } } }
        }
    });
    d_fn.set_config_json(
        Some(&app_blank_lbl.to_string()),
        Some(no_px_settings),
        Some(&km_fn.to_string()),
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        d_fn.cfg
            .appearance
            .stylemaps
            .get(&1)
            .and_then(|s| s.keys.get(&1))
            .and_then(|k| k.label.as_deref()),
        None
    );
    let f_fn_blank = d_fn.render_rgba(1763414400, 1000, false).to_vec();
    assert_eq!(
        f_fn, f_fn_blank,
        "empty stylemap label clears override and falls back to builtin icon"
    );
}

#[test]
fn ipc_line_buffer_enforces_per_record_limit_and_skips_oversized_remainder() {
    let mut lb = openflux_display::ipc::LineStreamBuffer::new();

    // Valid line + oversized 65_537-byte record in one chunk: keeps "ok", drops oversized, keeps "after"
    let mut mixed = Vec::new();
    mixed.extend_from_slice(b"ok\n");
    mixed.extend_from_slice(&vec![b'x'; 65_537]);
    mixed.extend_from_slice(b"\nafter\n");
    assert_eq!(lb.push_bytes(&mixed), vec!["ok", "after"]);

    // Near-limit valid record (65_000 bytes) followed by another line in the terminating chunk
    let near = vec![b'a'; 65_000];
    assert!(lb.push_bytes(&near).is_empty());
    let tail = b"z\nsecond\n".to_vec();
    let got = lb.push_bytes(&tail);
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].len(), 65_001);
    assert_eq!(got[1], "second");

    // Multi-chunk oversized record discards remainder across chunks until '\n'
    let big = vec![b'q'; 70_000];
    assert!(lb.push_bytes(&big).is_empty());
    assert_eq!(
        lb.push_bytes(b"remainder_of_oversized\nnext_ok\n"),
        vec!["next_ok"]
    );
}

#[test]
fn svg_tint_honors_alpha_fallback_width_matches_and_hex_shorthand_and_cache_limit() {
    use openflux_display::assets::AssetCache;
    let mut assets = AssetCache::new();

    // 1. Transparent tint (0x00FF0000) must leave black background unchanged
    let mut pix = vec![0xFF00_0000u32; 64 * 64];
    assert!(assets.draw_lucide_shortcut_svg("copy", &mut pix, 64, 64, 32, 32, 24, 0x00FF_0000));
    assert!(pix.iter().all(|&p| p == 0xFF00_0000));

    // Opaque tint draws non-black pixels
    assert!(assets.draw_lucide_shortcut_svg("copy", &mut pix, 64, 64, 32, 32, 24, 0xFFFF_FFFF));
    assert!(pix.iter().any(|&p| p != 0xFF00_0000));

    // 2. Fallback bitmap measurement must match draw_text advance when font is unavailable
    let mut fallback_pix = vec![0xFF00_0000u32; 200 * 40];
    let measured = assets.measure_text(None, "NoSuchFontFamily", 400.0, false, 14, "ABCD");
    let drawn = assets.draw_text(
        None,
        "NoSuchFontFamily",
        400.0,
        false,
        &mut fallback_pix,
        200,
        40,
        0,
        0,
        14,
        0xFFFF_FFFF,
        "ABCD",
    );
    assert_eq!(measured, drawn);

    // 3. Oversized SVG (>16 MiB cache budget) must be rejected without evicting existing entries
    assert!(!assets.draw_lucide_shortcut_svg("copy", &mut pix, 64, 64, 32, 32, 4096, 0xFFFF_FFFF));

    // 4. 3-digit and 4-digit hex shorthand lockIndicatorColor + "Shortcut" non-scissors + "undo" open arc
    let mut d = DisplayPreview::new(1920, 1080);
    d.set_custom_bg_rgba(&[0, 0, 0, 255], 1, 1).unwrap();
    let app = serde_json::json!({
        "selectedId": "default",
        "defaultTheme": {
            "id": "default",
            "backgroundImageDimming": 0,
            "lockIndicatorColor": "#FFF"
        }
    });
    d.set_config_json(Some(&app.to_string()), None, None, None, None)
        .unwrap();
    assert_eq!(d.cfg.appearance.lock_indicator_argb, Some(0xFFFF_FFFF));

    let app4 = serde_json::json!({
        "selectedId": "default",
        "defaultTheme": {
            "id": "default",
            "backgroundImageDimming": 0,
            "lockIndicatorColor": "#F08A"
        }
    });
    d.set_config_json(Some(&app4.to_string()), None, None, None, None)
        .unwrap();
    assert_eq!(d.cfg.appearance.lock_indicator_argb, Some(0xAAFF_0088));

    // Combined shortcut path: "Shortcut" must not match "cut" scissors (unlike "Cut"), and "Undo" renders cleanly
    let mut km_sc = vec![serde_json::json!({"type": 0, "keyCode": "KC_NO"}); 85];
    km_sc[0] = serde_json::json!({"type": 0, "keyCode": "KC_LGUI"});
    km_sc[1] = serde_json::json!({"type": 0, "keyCode": "KC_A"});
    km_sc[2] = serde_json::json!({"type": 0, "keyCode": "KC_S"});
    let km_sc_json = serde_json::json!({
        "selectedFrameLayoutType": "ansi",
        "keymaps": {
            "ansi": {
                "defaultLayer": 0,
                "layers": [{"name": "Base", "keyActions": km_sc}]
            }
        }
    });
    d.set_config_json(None, None, Some(&km_sc_json.to_string()), None, None)
        .unwrap();
    let bm_lgui = format!("01{}", "00".repeat(31));
    d.apply_live_json(
        &format!(
            r#"{{"type":"key_state","key_bitmap":"{}","shortcutOverlays":[{{"mods":8,"keyCode":"KC_A","label":"Shortcut","icon":""}},{{"mods":8,"keyCode":"KC_S","label":"Undo","icon":""}}]}}"#,
            bm_lgui
        ),
        10,
    )
    .unwrap();
    let f_shortcut = d.render_rgba(1763414400, 10, false);
    let kr_a = d.get_key_rects()[1];
    // Top center icon area of key 1 (where scissors would draw at cy - 4) must remain black for "Shortcut"
    let icon_y = (kr_a.y + kr_a.h * 0.5 - 8.0) as usize;
    let icon_x = (kr_a.x + kr_a.w * 0.5) as usize;
    let icon_px = (icon_y * 1920 + icon_x) * 4;
    assert_eq!(&f_shortcut[icon_px..icon_px + 3], &[0, 0, 0]);

    d.apply_live_json(
        r#"{"shortcutOverlays":[{"mods":8,"keyCode":"KC_A","label":"Cut","icon":""}]}"#,
        20,
    )
    .unwrap();
    let f_cut = d.render_rgba(1763414400, 20, false);
    assert_ne!(f_shortcut, f_cut);

    // 5. Display preinst script scopes process kill with OPENFLUX_SYSROOT and skips factory match under sysroot
    let preinst = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packaging/preinst");
    let pre_txt = std::fs::read_to_string(&preinst).unwrap();
    assert!(pre_txt.contains(r#"${ROOT}/usr/lib/openflux/display/openflux-display"#));
    assert!(pre_txt.contains(r#"if [ -z "$ROOT" ]"#));
}

#[test]
fn function_keys_handle_theme_cycle_brightness_three_layer_fallthrough_and_profile_switch_guard() {
    use openflux_display::config::{DisplayConfig, LiveState};

    let tmp = std::env::temp_dir().join(format!("openflux_disp_funcs_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let app = serde_json::json!({
        "selectedId": "default",
        "defaultTheme": {
            "id": "default",
            "name": "Aurora",
            "legendColor": 0xFFFFFFFFu32,
            "backgroundImageDimming": 10.0
        },
        "customThemes": [
            { "id": "ocean", "name": "Ocean", "legendColor": 0xFF00AAFFu32 },
            { "id": "ember", "name": "Ember", "legendColor": 0xFFFF5500u32 }
        ]
    });
    std::fs::write(tmp.join("appearance.json"), app.to_string()).unwrap();

    // 3-layer keymap:
    // L0: key0 = KC_OF_PROFILE_CYCLE_NEXT, key1 = KC_OF_START_PAUSE_POMODORO, key2 = KC_OF_BRIGHTNESS_UP
    // L1: key0 = KC_OF_RESET_POMODORO, key1 = KC_OF_THEME_CYCLE_NEXT, key2 = KC_NO (stops fallthrough)
    // L2: key0 = KC_TRANSPARENT, key1 = KC_TRANSPARENT (falls through to L1, not L0), key2 = KC_TRANSPARENT
    let mut l0 = vec![serde_json::json!({"type":0,"keyCode":"KC_NO"}); 85];
    l0[0] = serde_json::json!({"type":0,"keyCode":"KC_OF_PROFILE_CYCLE_NEXT"});
    l0[1] = serde_json::json!({"type":0,"keyCode":"KC_OF_START_PAUSE_POMODORO"});
    l0[2] = serde_json::json!({"type":0,"keyCode":"KC_OF_BRIGHTNESS_UP"});

    let mut l1 = vec![serde_json::json!({"type":0,"keyCode":"KC_NO"}); 85];
    l1[0] = serde_json::json!({"type":0,"keyCode":"KC_OF_RESET_POMODORO"});
    l1[1] = serde_json::json!({"type":0,"keyCode":"KC_OF_THEME_CYCLE_NEXT"});
    l1[2] = serde_json::json!({"type":0,"keyCode":"KC_NO"});

    let mut l2 = vec![serde_json::json!({"type":0,"keyCode":"KC_TRANSPARENT"}); 85];
    l2[3] = serde_json::json!({"type":0,"keyCode":"KC_OF_BRIGHTNESS_DOWN"});

    let km = serde_json::json!({
        "selectedFrameLayoutType": "ansi",
        "keymaps": {
            "ansi": {
                "defaultLayer": 0,
                "layers": [
                    { "name": "L0", "keyActions": l0 },
                    { "name": "L1", "keyActions": l1 },
                    { "name": "L2", "keyActions": l2 }
                ]
            }
        }
    });
    std::fs::write(tmp.join("keymap.json"), km.to_string()).unwrap();

    let mut cfg = DisplayConfig::load_dir(&tmp);
    let mut st = LiveState::from_config(&cfg);
    assert_eq!(cfg.theme_ids(), vec!["default", "ocean", "ember"]);

    // 1. With layers 1 and 2 held (bitfield = 0b111 = 7), key1 is KC_TRANSPARENT on L2 ->
    //    falls through to L1 (KC_OF_THEME_CYCLE_NEXT), not straight to L0 (Pomodoro).
    let bm_k1 = format!("02{}", "00".repeat(31));
    let bm_k2 = format!("04{}", "00".repeat(31));
    let bm_zero = "00".repeat(32);
    st.apply_live_json(
        &format!(r#"{{"type":"key_state","active_profile":0,"active_layer_bitfield":7,"key_bitmap":"{bm_k1}"}}"#),
        10,
    ).unwrap();
    assert!(st.handle_key_press_actions(&cfg, 10));
    assert_eq!(st.pending_theme_delta, 1);
    assert!(!st.pomo_running);

    // Cycling the theme writes theme_state.json, not appearance.json, and survives restart
    // (load_dir -> LiveState::from_config -> reconcile_live_theme), the first key_state and
    // unchanged host heartbeats on profiles 0 and 1.
    std::fs::write(
        tmp.join("profiles.json"),
        serde_json::json!([
            { "id": "p0", "themeId": "default" },
            { "id": "p1", "themeId": "ember" }
        ])
        .to_string(),
    )
    .unwrap();
    cfg.load_profiles_json(&std::fs::read_to_string(tmp.join("profiles.json")).unwrap());
    st.apply_live_json(
        r#"{"type":"live_data","payload":{"activeThemeId":"default","activeThemeProfileSlot":0,"themeRev":1}}"#,
        5,
    )
    .unwrap();
    let td = std::mem::take(&mut st.pending_theme_delta);
    let app_disk_before = std::fs::read_to_string(tmp.join("appearance.json")).unwrap();
    assert!(cfg.cycle_theme(td, &mut st, Some(&tmp)));
    assert_eq!(cfg.active_theme_id, "ocean");
    assert_eq!(
        std::fs::read_to_string(tmp.join("appearance.json")).unwrap(),
        app_disk_before
    );
    let mut restarted_cfg = DisplayConfig::load_dir(&tmp);
    let mut restarted_st = LiveState::from_config(&restarted_cfg);
    assert!(!restarted_cfg.reconcile_live_theme(&restarted_st));
    assert_eq!(restarted_cfg.active_theme_id, "ocean");
    // Unchanged host heartbeat after restart keeps restored "ocean"
    restarted_st
        .apply_live_json(
            r#"{"type":"live_data","payload":{"activeThemeId":"default","activeThemeProfileSlot":0,"themeRev":1}}"#,
            11,
        )
        .unwrap();
    assert!(!restarted_cfg.reconcile_live_theme(&restarted_st));
    assert_eq!(restarted_cfg.active_theme_id, "ocean");
    // Profile 1: cycle to "ocean", restart, feed the first key_state(profile=1) and a heartbeat; the theme stays "ocean".
    restarted_st
        .apply_live_json(
            &format!(r#"{{"type":"key_state","active_profile":1,"key_bitmap":"{bm_zero}"}}"#),
            12,
        )
        .unwrap();
    assert!(restarted_cfg.reconcile_live_theme(&restarted_st));
    assert_eq!(restarted_cfg.active_theme_id, "ember");
    restarted_st
        .apply_live_json(
            r#"{"type":"live_data","payload":{"activeThemeId":"ember","activeThemeProfileSlot":1,"themeRev":2}}"#,
            13,
        )
        .unwrap();
    assert!(restarted_cfg.cycle_theme(-1, &mut restarted_st, Some(&tmp)));
    assert_eq!(restarted_cfg.active_theme_id, "ocean");
    let mut p1_cfg = DisplayConfig::load_dir(&tmp);
    let mut p1_st = LiveState::from_config(&p1_cfg);
    p1_st
        .apply_live_json(
            &format!(r#"{{"type":"key_state","active_profile":1,"key_bitmap":"{bm_zero}"}}"#),
            14,
        )
        .unwrap();
    assert!(!p1_cfg.reconcile_live_theme(&p1_st));
    assert_eq!(p1_cfg.active_theme_id, "ocean");
    p1_st
        .apply_live_json(
            r#"{"type":"live_data","payload":{"activeThemeId":"ember","activeThemeProfileSlot":1,"themeRev":2}}"#,
            15,
        )
        .unwrap();
    assert!(!p1_cfg.reconcile_live_theme(&p1_st));
    assert_eq!(p1_cfg.active_theme_id, "ocean");
    // Newer host themeRev=3 supersedes restored theme
    p1_st
        .apply_live_json(
            r#"{"type":"live_data","payload":{"activeThemeId":"ember","activeThemeProfileSlot":1,"themeRev":3}}"#,
            16,
        )
        .unwrap();
    assert!(p1_cfg.reconcile_live_theme(&p1_st));
    assert_eq!(p1_cfg.active_theme_id, "ember");
    st = LiveState::from_config(&cfg);
    assert!(cfg.cycle_theme(-1, &mut st, Some(&tmp)));
    assert_eq!(cfg.active_theme_id, "default");

    // 1b. Concurrent host edits to appearance.json and settings.json without a marker are never lost
    //     or overwritten by cycle_theme, and even if the host removes the previously cached theme ("ocean"),
    //     the next poll_reload drops the stale generation override and loads the new host appearance & settings.
    let mut reloader = openflux_display::config::ConfigReloader::new(&tmp);
    std::thread::sleep(std::time::Duration::from_millis(5));
    let updated_app = serde_json::json!({
        "selectedId": "default",
        "defaultTheme": { "id": "default", "name": "Aurora" },
        "customThemes": [
            { "id": "ember", "name": "Ember" },
            { "id": "nebula", "name": "Nebula" }
        ]
    });
    std::fs::write(tmp.join("appearance.json"), updated_app.to_string()).unwrap();
    std::fs::write(tmp.join("settings.json"), r#"{"timeout": 42}"#).unwrap();
    assert!(cfg.cycle_theme(1, &mut st, Some(&tmp)));
    assert!(std::fs::read_to_string(tmp.join("appearance.json"))
        .unwrap()
        .contains("nebula"));
    assert!(reloader.poll_reload(&mut cfg));
    assert_eq!(cfg.active_theme_id, "default");
    assert!((cfg.settings.timeout_sec - 42.0).abs() < 0.01);
    assert!(cfg.theme_ids().iter().any(|id| id == "nebula"));
    assert!(!cfg.theme_ids().iter().any(|id| id == "ocean"));
    // reconcile_live_theme(&st) after poll_reload does not revive the stale "ocean" local override
    // against the newly loaded appearance generation.
    assert!(!cfg.reconcile_live_theme(&st));
    assert_eq!(cfg.active_theme_id, "default");
    // Restore original appearance with ocean/ember for remaining tests
    std::thread::sleep(std::time::Duration::from_millis(5));
    std::fs::write(tmp.join("appearance.json"), app.to_string()).unwrap();
    assert!(reloader.poll_reload(&mut cfg));
    assert!(!cfg.reconcile_live_theme(&st));

    std::fs::write(tmp.join("config.updating"), b"1").unwrap();
    let _ = std::fs::remove_file(tmp.join("theme_state.json"));
    assert!(cfg.cycle_theme(1, &mut st, Some(&tmp)));
    assert!(!tmp.join("theme_state.json").exists());
    let _ = std::fs::remove_file(tmp.join("config.updating"));
    assert!(cfg.cycle_theme(-1, &mut st, Some(&tmp)));
    assert_eq!(cfg.active_theme_id, "default");

    // 2. Without of_events_seq: on L2+L1 (bitfield=7), key2 is KC_TRANSPARENT on L2
    //    and KC_NO on L1 -> stops at KC_NO, not L0 BRIGHTNESS_UP.
    st.apply_live_json(
        &format!(r#"{{"type":"key_state","active_profile":0,"active_layer_bitfield":7,"key_bitmap":"{bm_k2}"}}"#),
        20,
    ).unwrap();
    assert!(!st.handle_key_press_actions(&cfg, 20));
    assert!(st.pending_brightness_steps.is_empty());

    // Clear keys + establish valid zero baseline with ordered of_events_seq=0
    st.apply_live_json(
        &format!(r#"{{"type":"key_state","seq":1,"timestamp_ms":100,"active_profile":0,"active_layer_bitfield":1,"of_events_seq":0,"of_events_ring":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"key_bitmap":"{bm_zero}"}}"#),
        30,
    ).unwrap();
    assert!(!st.handle_key_press_actions(&cfg, 30));

    // 2b. Initial nonzero snapshot on a fresh LiveState is a baseline; its events do not fire (for seq=1 and seq=17),
    //     and repeated identical snapshots are no-ops.
    let mut seq1_st = LiveState::from_config(&cfg);
    seq1_st.apply_live_json(
        &format!(r#"{{"type":"key_state","seq":5,"timestamp_ms":500,"of_events_seq":1,"of_events_ring":[14,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"key_bitmap":"{bm_zero}"}}"#),
        31,
    ).unwrap();
    assert!(!seq1_st.handle_key_press_actions(&cfg, 31));
    assert_eq!(seq1_st.pending_theme_delta, 0);

    let mut reconnect_st = LiveState::from_config(&cfg);
    reconnect_st.apply_live_json(
        &format!(r#"{{"type":"key_state","seq":50,"timestamp_ms":5000,"of_events_seq":17,"of_events_ring":[14,14,14,14,14,14,14,14,14,14,14,14,14,14,14,14],"key_bitmap":"{bm_zero}"}}"#),
        31,
    ).unwrap();
    assert!(!reconnect_st.handle_key_press_actions(&cfg, 31));
    assert_eq!(reconnect_st.pending_theme_delta, 0);
    // Repeated identical snapshot -> no-op
    reconnect_st.apply_live_json(
        &format!(r#"{{"type":"key_state","seq":51,"timestamp_ms":5100,"of_events_seq":17,"of_events_ring":[14,14,14,14,14,14,14,14,14,14,14,14,14,14,14,14],"key_bitmap":"{bm_zero}"}}"#),
        32,
    ).unwrap();
    assert!(!reconnect_st.handle_key_press_actions(&cfg, 32));
    // Genuine u16 wrap (65535 -> 1) while uptime advances fires the 2 new events (BRIGHTNESS_UP)
    reconnect_st.prev_of_events_seq = 65535;
    let mut wrap_ring = [0u8; 16];
    wrap_ring[15] = 2;
    wrap_ring[0] = 2;
    reconnect_st.apply_live_json(
        &format!(r#"{{"type":"key_state","seq":52,"timestamp_ms":5200,"of_events_seq":1,"of_events_ring":{},"key_bitmap":"{bm_zero}"}}"#, serde_json::to_string(&wrap_ring).unwrap()),
        33,
    ).unwrap();
    assert!(reconnect_st.handle_key_press_actions(&cfg, 33));
    assert_eq!(reconnect_st.pending_brightness_steps, vec![1, 1]);
    // MCU reboot (timestamp_ms & seq reset toward 0 while of_events_seq drops 1 -> 0) baselines without phantom wrap
    reconnect_st.pending_brightness_steps.clear();
    reconnect_st.apply_live_json(
        &format!(r#"{{"type":"key_state","seq":1,"timestamp_ms":50,"of_events_seq":0,"of_events_ring":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"key_bitmap":"{bm_zero}"}}"#),
        34,
    ).unwrap();
    assert!(!reconnect_st.handle_key_press_actions(&cfg, 34));
    assert!(reconnect_st.pending_brightness_steps.is_empty());

    // 3. Ordered firmware-resolved event ring:
    //    a) Simultaneous profile-cycle (4) + BRIGHTNESS_UP (2) while destination layer has RESET_POMODORO ->
    //       fires BRIGHTNESS_UP, not RESET_POMODORO.
    st.pomo_running = false;
    st.pomo_remaining_s = 123;
    st.pending_brightness_steps.clear();
    st.pending_theme_delta = 0;
    let bm_k0_k1 = format!("03{}", "00".repeat(31));
    st.apply_live_json(
        &format!(r#"{{"type":"key_state","seq":2,"timestamp_ms":200,"active_profile":1,"active_layer_bitfield":2,"of_events_seq":2,"of_events_ring":[4,2,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"key_bitmap":"{bm_k0_k1}"}}"#),
        40,
    ).unwrap();
    assert!(st.handle_key_press_actions(&cfg, 40));
    assert_eq!(st.pending_brightness_steps, vec![1]);
    assert!(!st.pomo_running && st.pomo_remaining_s == 123);
    assert_eq!(st.pending_theme_delta, 0);

    //    b) RESET (13) -> START_PAUSE (12) in one update ends running; START_PAUSE (12) -> RESET (13) ends stopped.
    st.pomo_running = true;
    st.pomo_remaining_s = 99;
    st.apply_live_json(
        &format!(r#"{{"type":"key_state","seq":3,"timestamp_ms":300,"active_profile":1,"active_layer_bitfield":2,"of_events_seq":4,"of_events_ring":[4,2,13,12,0,0,0,0,0,0,0,0,0,0,0,0],"key_bitmap":"{bm_zero}"}}"#),
        42,
    ).unwrap();
    assert!(st.handle_key_press_actions(&cfg, 42));
    assert!(st.pomo_running);

    st.apply_live_json(
        &format!(r#"{{"type":"key_state","seq":4,"timestamp_ms":400,"active_profile":1,"active_layer_bitfield":2,"of_events_seq":6,"of_events_ring":[4,2,13,12,12,13,0,0,0,0,0,0,0,0,0,0],"key_bitmap":"{bm_zero}"}}"#),
        45,
    ).unwrap();
    assert!(st.handle_key_press_actions(&cfg, 45));
    assert!(!st.pomo_running);

    // 4. Host live_data 5s keepalive repeats of activeThemeId/themeRev don't overwrite a keyboard-cycled theme,
    //    an explicit host pick of the same theme with a bumped themeRev applies, and a missing themeRev
    //    falls back to theme ID/slot deduplication.
    st.apply_live_json(
        &format!(r#"{{"type":"key_state","seq":5,"timestamp_ms":450,"active_profile":0,"active_layer_bitfield":1,"of_events_seq":6,"of_events_ring":[4,2,13,12,12,13,0,0,0,0,0,0,0,0,0,0],"key_bitmap":"{bm_zero}"}}"#),
        48,
    ).unwrap();
    st.apply_live_json(
        r#"{"type":"live_data","payload":{"activeThemeId":"default","activeThemeProfileSlot":0,"themeRev":10}}"#,
        50,
    ).unwrap();
    assert_eq!(st.active_theme_id, "default");
    assert!(cfg.cycle_theme(1, &mut st, Some(&tmp)));
    assert_eq!(cfg.active_theme_id, "ocean");
    // Heartbeat repeats A with themeRev=10 -> ignored, "ocean" sticks
    st.apply_live_json(
        r#"{"type":"live_data","payload":{"activeThemeId":"default","activeThemeProfileSlot":0,"themeRev":10}}"#,
        60,
    ).unwrap();
    assert!(!cfg.reconcile_live_theme(&st));
    assert_eq!(cfg.active_theme_id, "ocean");
    // Host explicitly re-selects "default" in Studio -> themeRev=11 applies "default"
    st.apply_live_json(
        r#"{"type":"live_data","payload":{"activeThemeId":"default","activeThemeProfileSlot":0,"themeRev":11}}"#,
        70,
    ).unwrap();
    assert!(cfg.reconcile_live_theme(&st));
    assert_eq!(cfg.active_theme_id, "default");
    // Missing themeRev: cycling to "ocean" + repeat "default" without themeRev keeps "ocean"; new ID "ember" applies
    assert!(cfg.cycle_theme(1, &mut st, Some(&tmp)));
    assert_eq!(cfg.active_theme_id, "ocean");
    st.apply_live_json(
        r#"{"type":"live_data","payload":{"activeThemeId":"default","activeThemeProfileSlot":0}}"#,
        80,
    )
    .unwrap();
    assert!(!cfg.reconcile_live_theme(&st));
    assert_eq!(cfg.active_theme_id, "ocean");
    st.apply_live_json(
        r#"{"type":"live_data","payload":{"activeThemeId":"ember","activeThemeProfileSlot":0}}"#,
        90,
    )
    .unwrap();
    assert!(cfg.reconcile_live_theme(&st));
    assert_eq!(cfg.active_theme_id, "ember");

    // 5. Ordered per-line IPC batch flushing via apply_ipc_line:
    //    a) key_state THEME_CYCLE_NEXT (14) followed by live_data explicit host pick "ember" (themeRev=99)
    //       in the same poll batch ends on "ember" (later host pick wins, not cycled from "ember").
    let mut batch_st = LiveState::from_config(&cfg);
    batch_st.apply_live_json(
        &format!(r#"{{"type":"key_state","seq":1,"timestamp_ms":100,"active_profile":0,"of_events_seq":0,"of_events_ring":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"key_bitmap":"{bm_zero}"}}"#),
        95,
    ).unwrap();
    cfg.apply_ipc_line(&mut batch_st, r#"{"type":"live_data","payload":{"activeThemeId":"default","activeThemeProfileSlot":0,"themeRev":98}}"#, 95, Some(&tmp));
    assert_eq!(cfg.active_theme_id, "default");
    cfg.apply_ipc_line(&mut batch_st, &format!(r#"{{"type":"key_state","seq":2,"timestamp_ms":110,"active_profile":0,"of_events_seq":1,"of_events_ring":[14,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"key_bitmap":"{bm_zero}"}}"#), 96, Some(&tmp));
    cfg.apply_ipc_line(&mut batch_st, r#"{"type":"live_data","payload":{"activeThemeId":"ember","activeThemeProfileSlot":0,"themeRev":99}}"#, 96, Some(&tmp));
    assert_eq!(cfg.active_theme_id, "ember");

    //    b) key_state(profile=0) THEME_CYCLE_NEXT (14) followed by key_state(profile=1) in the same batch
    //       applies the cycle to profile 0 and then switches profile 1 to "ember".
    cfg.apply_ipc_line(&mut batch_st, &format!(r#"{{"type":"key_state","seq":3,"timestamp_ms":120,"active_profile":0,"of_events_seq":2,"of_events_ring":[14,14,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"key_bitmap":"{bm_zero}"}}"#), 97, Some(&tmp));
    cfg.apply_ipc_line(&mut batch_st, &format!(r#"{{"type":"key_state","seq":4,"timestamp_ms":130,"active_profile":1,"of_events_seq":2,"of_events_ring":[14,14,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"key_bitmap":"{bm_zero}"}}"#), 97, Some(&tmp));
    assert_eq!(cfg.active_theme_id, "ember");

    //    c) Intra-snapshot ring ordering with of_events_profiles:
    //       1) [14, 4] (THEME_NEXT on profile 0 -> PROFILE_NEXT to profile 1) ends on profile 1's "ember".
    let mut intra_st = LiveState::from_config(&cfg);
    intra_st.apply_live_json(
        &format!(r#"{{"type":"key_state","seq":1,"timestamp_ms":10,"active_profile":0,"of_events_seq":0,"of_events_ring":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"of_events_profiles":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"key_bitmap":"{bm_zero}"}}"#),
        98,
    ).unwrap();
    cfg.reconcile_live_theme(&intra_st);
    assert_eq!(cfg.active_theme_id, "default");
    cfg.apply_ipc_line(
        &mut intra_st,
        &format!(r#"{{"type":"key_state","seq":2,"timestamp_ms":20,"active_profile":1,"of_events_seq":2,"of_events_ring":[14,4,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"of_events_profiles":[0,1,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"key_bitmap":"{bm_zero}"}}"#),
        99,
        Some(&tmp),
    );
    assert_eq!(cfg.active_theme_id, "ember");

    //       2) [4, 14] (PROFILE_NEXT to profile 1 -> THEME_NEXT on profile 1) cycles from "ember" to "default".
    let _ = std::fs::remove_file(tmp.join("theme_state.json"));
    cfg.theme_override = None;
    let mut rev_st = LiveState::from_config(&cfg);
    rev_st.apply_live_json(
        &format!(r#"{{"type":"key_state","seq":1,"timestamp_ms":10,"active_profile":0,"of_events_seq":0,"of_events_ring":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"of_events_profiles":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"key_bitmap":"{bm_zero}"}}"#),
        100,
    ).unwrap();
    cfg.reconcile_live_theme(&rev_st);
    assert_eq!(cfg.active_theme_id, "default");
    cfg.apply_ipc_line(
        &mut rev_st,
        &format!(r#"{{"type":"key_state","seq":2,"timestamp_ms":20,"active_profile":1,"of_events_seq":2,"of_events_ring":[4,14,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"of_events_profiles":[1,1,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"key_bitmap":"{bm_zero}"}}"#),
        101,
        Some(&tmp),
    );
    assert_eq!(cfg.active_theme_id, "default"); // cycled forward from "ember" (last) to "default" (first), not "ocean"

    let _ = std::fs::remove_dir_all(&tmp);
}
