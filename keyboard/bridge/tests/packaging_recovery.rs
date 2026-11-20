use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn repo_bridge_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_root_dir() -> PathBuf {
    repo_bridge_dir()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn write_exec(path: &Path, content: &str) {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).unwrap();
    }
    fs::write(path, content).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn make_temp_root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("openflux_pkg_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("tmp")).unwrap();
    fs::create_dir_all(dir.join("usr/bin")).unwrap();
    fs::create_dir_all(dir.join("home/root/openflux/logs")).unwrap();
    fs::create_dir_all(dir.join("home/root/flux/logs")).unwrap();
    dir
}

fn sha256_file(path: &Path) -> String {
    let out = if Command::new("sha256sum").arg("--version").output().is_ok() {
        Command::new("sha256sum").arg(path).output().unwrap()
    } else {
        Command::new("shasum")
            .args(["-a", "256"])
            .arg(path)
            .output()
            .unwrap()
    };
    assert!(out.status.success(), "sha256 failed on {}", path.display());
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .unwrap()
        .to_string()
}

fn build_mock_ipk(out_ipk: &Path, pkg_name: &str, corrupt_data: bool) {
    let stage = out_ipk.parent().unwrap().join(format!("_stage_{pkg_name}"));
    let _ = fs::remove_dir_all(&stage);
    let ctrl_dir = stage.join("control");
    let data_dir = stage.join("data");
    fs::create_dir_all(&ctrl_dir).unwrap();
    fs::create_dir_all(data_dir.join("usr/bin")).unwrap();
    if pkg_name == "garfield" {
        write_exec(
            &data_dir.join("usr/lib/garfield/scripts/flux-weston-autolaunch.sh"),
            "#!/bin/sh\necho \"factory-autolaunch\" >> \"${OPENFLUX_SYSROOT}/tmp/boot_trace.log\"\n",
        );
    }
    fs::write(
        ctrl_dir.join("control"),
        format!("Package: {pkg_name}\nVersion: 1.4.11\nArchitecture: aarch64\n"),
    )
    .unwrap();
    fs::write(data_dir.join("usr/bin/dummy"), b"ok\n").unwrap();

    let build_ipk_py = repo_root_dir().join("scripts/build_ipk.py");
    let st = Command::new("python3")
        .arg(&build_ipk_py)
        .args([
            "--control-dir",
            ctrl_dir.to_str().unwrap(),
            "--data-dir",
            data_dir.to_str().unwrap(),
            "-o",
            out_ipk.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(st.success(), "build_ipk.py failed for {pkg_name}");

    if corrupt_data {
        let extract_dir = stage.join("repack");
        fs::create_dir_all(&extract_dir).unwrap();
        let st_x = Command::new("ar")
            .args(["x", out_ipk.to_str().unwrap()])
            .current_dir(&extract_dir)
            .status()
            .unwrap();
        assert!(st_x.success());
        let data_gz = extract_dir.join("data.tar.gz");
        fs::set_permissions(&data_gz, fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(&data_gz, b"NOT_A_VALID_GZIP_TAR").unwrap();
        fs::remove_file(out_ipk).unwrap();
        let st_r = Command::new("ar")
            .args([
                "rc",
                out_ipk.to_str().unwrap(),
                "debian-binary",
                "control.tar.gz",
                "data.tar.gz",
            ])
            .current_dir(&extract_dir)
            .status()
            .unwrap();
        assert!(st_r.success());
    }
    let _ = fs::remove_dir_all(&stage);
}

#[test]
fn staged_preinst_writes_target_attempt_and_rotates_on_armed_retries() {
    let pkg_dir = repo_bridge_dir();
    let raw_preinst = fs::read_to_string(pkg_dir.join("packaging/preinst")).unwrap();
    let root = make_temp_root("preinst_attempt");

    // Stage preinst the exact way Makefile substitutes @OPENFLUX_VERSION@ with VERSION=1.2.3.
    let staged_preinst = root.join("tmp/staged_preinst_123");
    fs::write(
        &staged_preinst,
        raw_preinst.replace("@OPENFLUX_VERSION@", "1.2.3"),
    )
    .unwrap();
    fs::set_permissions(&staged_preinst, fs::Permissions::from_mode(0o755)).unwrap();

    // 1. First conversion fails closed on missing rescue files or manifest mismatch (changing nothing),
    //    and proceeds cleanly once the verified rescue pair + manifest.sha256 are staged.
    assert!(!Command::new("sh")
        .arg(&staged_preinst)
        .env("OPENFLUX_SYSROOT", &root)
        .env("OPENFLUX_SKIP_SYNC", "1")
        .status()
        .unwrap()
        .success());
    assert!(!root.join("home/root/.openflux/rollback/bridge").exists());

    {
        let root_fc = make_temp_root("first_conv_rescue");
        let rescue_dir = root_fc.join("home/root/.openflux/rescue");
        let bridge_file = rescue_dir.join("openflux-bridge");
        let launch_file = rescue_dir.join("openflux-autolaunch.sh");
        write_exec(&bridge_file, "#!/bin/sh\n# rescue-bridge\n");
        let pre_fc = root_fc.join("tmp/preinst");
        fs::write(&pre_fc, raw_preinst.replace("@OPENFLUX_VERSION@", "1.0.0")).unwrap();
        fs::set_permissions(&pre_fc, fs::Permissions::from_mode(0o755)).unwrap();

        // 1a: Missing openflux-autolaunch.sh -> preinst refuses and changes nothing.
        fs::write(
            rescue_dir.join("manifest.sha256"),
            format!("{}  openflux-bridge\n", sha256_file(&bridge_file)),
        )
        .unwrap();
        assert!(!Command::new("sh")
            .arg(&pre_fc)
            .env("OPENFLUX_SYSROOT", &root_fc)
            .env("OPENFLUX_SKIP_SYNC", "1")
            .status()
            .unwrap()
            .success());
        assert!(!root_fc.join("home/root/.openflux/rollback/bridge").exists());
        assert!(!root_fc.join("usr/bin/flux-weston-autolaunch.sh").exists());

        // 1b: Manifest mismatch -> preinst refuses and changes nothing.
        write_exec(
            &launch_file,
            "#!/bin/sh\necho rescue-openflux-booted >> \"${OPENFLUX_SYSROOT}/tmp/fc_boot.log\"\n",
        );
        fs::write(
            rescue_dir.join("manifest.sha256"),
            format!(
                "{}  openflux-bridge\n{}  openflux-autolaunch.sh\n",
                sha256_file(&bridge_file),
                "0".repeat(64),
            ),
        )
        .unwrap();
        assert!(!Command::new("sh")
            .arg(&pre_fc)
            .env("OPENFLUX_SYSROOT", &root_fc)
            .env("OPENFLUX_SKIP_SYNC", "1")
            .status()
            .unwrap()
            .success());
        assert!(!root_fc.join("home/root/.openflux/rollback/bridge").exists());
        assert!(!root_fc.join("usr/bin/flux-weston-autolaunch.sh").exists());

        // 1c: Full verified rescue pair + matching manifest -> proceeds, and a power cut recovers the OpenFlux rescue bridge.
        fs::write(
            rescue_dir.join("manifest.sha256"),
            format!(
                "{}  openflux-bridge\n{}  openflux-autolaunch.sh\n",
                sha256_file(&bridge_file),
                sha256_file(&launch_file),
            ),
        )
        .unwrap();
        assert!(Command::new("sh")
            .arg(&pre_fc)
            .env("OPENFLUX_SYSROOT", &root_fc)
            .env("OPENFLUX_SKIP_SYNC", "1")
            .status()
            .unwrap()
            .success());
        assert!(root_fc
            .join("home/root/.openflux/rollback/bridge/armed")
            .is_file());
        assert!(Command::new("sh")
            .arg(root_fc.join("usr/bin/flux-weston-autolaunch.sh"))
            .env("OPENFLUX_SYSROOT", &root_fc)
            .env("OPENFLUX_SKIP_SYNC", "1")
            .status()
            .unwrap()
            .success());
        assert!(!root_fc
            .join("home/root/.openflux/rollback/bridge/armed")
            .exists());
        assert!(root_fc
            .join("usr/lib/openflux/bridge/openflux-bridge")
            .is_file());
        assert!(fs::read_to_string(root_fc.join("tmp/fc_boot.log"))
            .unwrap()
            .contains("rescue-openflux-booted"));
        let _ = fs::remove_dir_all(&root_fc);
    }

    // 2. Seed existing OpenFlux 0.9 bridge, run staged 1.2.3 preinst -> writes attempt with version=openflux-bridge 1.2.3 and arms.
    write_exec(
        &root.join("usr/lib/openflux/bridge/openflux-bridge"),
        "#!/bin/sh\n# old-0.9\n",
    );
    write_exec(
        &root.join("usr/lib/openflux/bridge/openflux-autolaunch.sh"),
        "#!/bin/sh\n# old-launcher\n",
    );
    let st_up = Command::new("sh")
        .arg(&staged_preinst)
        .env("OPENFLUX_SYSROOT", &root)
        .env("OPENFLUX_SKIP_SYNC", "1")
        .status()
        .unwrap();
    assert!(st_up.success());

    let rb_dir = root.join("home/root/.openflux/rollback/bridge");
    assert!(rb_dir.join("armed").is_file());
    let attempt1 = fs::read_to_string(rb_dir.join("attempt")).unwrap();
    assert!(
        attempt1.contains("version=openflux-bridge 1.2.3\n"),
        "got: {attempt1}"
    );
    let id1 = attempt1
        .lines()
        .find_map(|l| l.strip_prefix("attempt_id="))
        .unwrap()
        .to_string();
    assert!(!id1.is_empty());

    // Simulate stale trial marker from interrupted 1.2.3 attempt and a retry to 1.2.4.
    fs::write(
        rb_dir.join(format!("trial-{id1}")),
        "version=openflux-bridge 1.2.3\n",
    )
    .unwrap();
    let staged_124 = root.join("tmp/staged_preinst_124");
    fs::write(
        &staged_124,
        raw_preinst.replace("@OPENFLUX_VERSION@", "1.2.4"),
    )
    .unwrap();
    fs::set_permissions(&staged_124, fs::Permissions::from_mode(0o755)).unwrap();
    let st_retry = Command::new("sh")
        .arg(&staged_124)
        .env("OPENFLUX_SYSROOT", &root)
        .env("OPENFLUX_SKIP_SYNC", "1")
        .status()
        .unwrap();
    assert!(st_retry.success());

    // The original 0.9 snapshot is preserved, the stale trial marker is cleared, and the attempt metadata rotates to 1.2.4.
    assert!(fs::read_to_string(rb_dir.join("openflux-bridge"))
        .unwrap()
        .contains("old-0.9"));
    assert!(
        !rb_dir.join(format!("trial-{id1}")).exists(),
        "stale trial marker must be cleared on retry"
    );
    let attempt2 = fs::read_to_string(rb_dir.join("attempt")).unwrap();
    assert!(
        attempt2.contains("version=openflux-bridge 1.2.4\n"),
        "got: {attempt2}"
    );
    let id2 = attempt2
        .lines()
        .find_map(|l| l.strip_prefix("attempt_id="))
        .unwrap()
        .to_string();
    assert_ne!(id1, id2);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn packaged_shell_scripts_pass_sh_syntax_check() {
    let pkg_dir = repo_bridge_dir();
    let scripts = [
        pkg_dir.join("packaging/preinst"),
        pkg_dir.join("packaging/postinst"),
        pkg_dir.join("packaging/prerm"),
        pkg_dir.join("packaging/openflux-autolaunch.sh"),
        pkg_dir.join("packaging/restore-factory.sh"),
        pkg_dir.join("factory-restore-packaging/postinst"),
    ];
    for script in scripts {
        let out = Command::new("sh")
            .args(["-n", script.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "sh -n failed for {}: {}",
            script.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn confirmed_retry_quiesces_old_watchdog_and_upgrade_publishes_trial_marker() {
    let pkg_dir = repo_bridge_dir();
    let raw_preinst = fs::read_to_string(pkg_dir.join("packaging/preinst")).unwrap();
    let raw_postinst = fs::read_to_string(pkg_dir.join("packaging/postinst")).unwrap();
    let bin_bridge = PathBuf::from(env!("CARGO_BIN_EXE_openflux-bridge"));
    let version = env!("CARGO_PKG_VERSION");

    let root = make_temp_root("trial_e2e");
    let rb_dir = root.join("home/root/.openflux/rollback/bridge");
    fs::create_dir_all(&rb_dir).unwrap();

    // 1. Seed a confirmed earlier attempt whose sleeping watchdog is still running (`armed` deleted by the host).
    fs::write(
        rb_dir.join("attempt"),
        "attempt_id=oldattempt\nversion=openflux-bridge 0.9.0\n",
    )
    .unwrap();
    write_exec(&rb_dir.join("watchdog.sh"), "#!/bin/sh\nsleep 30\n");
    let mut old_wd = Command::new("sh")
        .arg(rb_dir.join("watchdog.sh"))
        .spawn()
        .unwrap();

    // Seed installed 0.9 bridge + launcher.
    write_exec(
        &root.join("usr/lib/openflux/bridge/openflux-bridge"),
        "#!/bin/sh\n# old-0.9\n",
    );
    write_exec(
        &root.join("usr/lib/openflux/bridge/openflux-autolaunch.sh"),
        "#!/bin/sh\nexit 0\n",
    );

    let staged_pre = root.join("tmp/preinst");
    let staged_post = root.join("tmp/postinst");
    fs::write(
        &staged_pre,
        raw_preinst.replace("@OPENFLUX_VERSION@", version),
    )
    .unwrap();
    fs::write(
        &staged_post,
        raw_postinst.replace("@OPENFLUX_VERSION@", version),
    )
    .unwrap();
    fs::set_permissions(&staged_pre, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(&staged_post, fs::Permissions::from_mode(0o755)).unwrap();

    // Run preinst for the next upgrade -> stops the earlier sleeping watchdog, saves the 0.9 snapshot, writes the attempt + armed.
    assert!(Command::new("sh")
        .arg(&staged_pre)
        .env("OPENFLUX_SYSROOT", &root)
        .env("OPENFLUX_SKIP_SYNC", "1")
        .env("OPENFLUX_PREINST_SLEEP", "0.05")
        .status()
        .unwrap()
        .success());
    let wd_done = old_wd.try_wait().unwrap().is_some() || {
        std::thread::sleep(Duration::from_millis(100));
        old_wd.try_wait().unwrap().is_some()
    };
    if !wd_done {
        let _ = old_wd.kill();
        let _ = old_wd.wait();
        panic!("preinst failed to stop stale watchdog from confirmed attempt");
    }

    let attempt_txt = fs::read_to_string(rb_dir.join("attempt")).unwrap();
    let attempt_id = attempt_txt
        .lines()
        .find_map(|l| l.strip_prefix("attempt_id="))
        .unwrap()
        .to_string();
    assert_ne!(attempt_id, "oldattempt");

    // 2. Unpack a candidate wrapper that runs the real compiled openflux-bridge binary in test FD mode.
    let bridge_bin_dst = root.join("usr/lib/openflux/bridge/openflux-bridge");
    write_exec(
        &bridge_bin_dst,
        &format!(
            "#!/bin/sh\nexec 3</dev/null 4>/dev/null\nexport OPENFLUX_TEST_EP_FDS=4,3\nexec \"{}\" \"$@\"\n",
            bin_bridge.display()
        ),
    );
    write_exec(
        &root.join("usr/lib/openflux/bridge/openflux-autolaunch.sh"),
        "#!/bin/sh\nexec \"${OPENFLUX_SYSROOT}/usr/lib/openflux/bridge/openflux-bridge\" \"${OPENFLUX_SYSROOT}/dev/ffs-openflux\"\n",
    );

    let ready_file = root.join("tmp/bridge_ready");
    let st_post = Command::new("sh")
        .arg(&staged_post)
        .env("OPENFLUX_SYSROOT", &root)
        .env("OPENFLUX_SKIP_SYNC", "1")
        .env("OPENFLUX_STOP_WAIT_ITERS", "1")
        .env("OPENFLUX_POSTINST_SLEEP", "0.05")
        .env("OPENFLUX_READY_WAIT_ITERS", "40")
        .env("OPENFLUX_STARTUP_CHECK_SLEEP", "0.05")
        .env("OPENFLUX_STABILIZE_SLEEP", "0.05")
        .env("OPENFLUX_USB_CONFIRM_ITERS", "40")
        .env("OPENFLUX_USB_CONFIRM_SLEEP", "0.1")
        .env("OPENFLUX_READY_FILE", &ready_file)
        .status()
        .unwrap();
    assert!(
        st_post.success(),
        "postinst trial launch with real openflux-bridge failed"
    );

    let trial_path = rb_dir.join(format!("trial-{attempt_id}"));
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && !trial_path.exists() {
        std::thread::sleep(Duration::from_millis(30));
    }
    assert!(
        trial_path.is_file(),
        "postinst + real openflux-bridge did not publish {}",
        trial_path.display()
    );
    let trial_txt = fs::read_to_string(&trial_path).unwrap();
    assert!(trial_txt.contains(&format!("version=openflux-bridge {version}")));

    // Confirm upgrade by removing armed, then clean up child processes and temp root.
    let _ = fs::remove_file(rb_dir.join("armed"));
    if let Ok(pid_s) = fs::read_to_string(&ready_file) {
        let pid = pid_s.trim();
        if !pid.is_empty() {
            let _ = Command::new("kill").args(["-TERM", pid]).status();
        }
    }
    let _ = Command::new("pkill")
        .args(["-KILL", "-f", &root.to_string_lossy()])
        .status();
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn upgrade_rollback_paths_succeed_and_preserve_armed_on_copy_failure() {
    let pkg_dir = repo_bridge_dir();
    let preinst = pkg_dir.join("packaging/preinst");
    let postinst = pkg_dir.join("packaging/postinst");
    let autolaunch = pkg_dir.join("packaging/openflux-autolaunch.sh");

    let seed_existing_openflux = |root: &Path| {
        write_exec(
            &root.join("usr/lib/openflux/bridge/openflux-bridge"),
            "#!/bin/sh\n# old-bridge-0.9\necho \"$$\" > \"${OPENFLUX_READY_FILE}\"\nsleep 30\n",
        );
        write_exec(
            &root.join("usr/lib/openflux/bridge/openflux-autolaunch.sh"),
            "#!/bin/sh\necho \"old-launcher-0.9\" >> \"${OPENFLUX_SYSROOT}/tmp/launch_trace.log\"\n",
        );
        fs::copy(
            root.join("usr/lib/openflux/bridge/openflux-autolaunch.sh"),
            root.join("usr/bin/flux-weston-autolaunch.sh"),
        )
        .unwrap();
        fs::set_permissions(
            root.join("usr/bin/flux-weston-autolaunch.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        fs::create_dir_all(root.join("home/root/.openflux/versions")).unwrap();
        fs::write(
            root.join("home/root/.openflux/versions/bridge"),
            "openflux-bridge 0.9.0\n",
        )
        .unwrap();
    };

    // Case A: Immediate readiness failure -> rolls back to 0.9 bridge + launcher and clears armed.
    {
        let root = make_temp_root("imm_rb");
        seed_existing_openflux(&root);
        assert!(Command::new("sh")
            .arg(&preinst)
            .env("OPENFLUX_SYSROOT", &root)
            .env("OPENFLUX_SKIP_SYNC", "1")
            .status()
            .unwrap()
            .success());
        write_exec(
            &root.join("usr/lib/openflux/bridge/openflux-bridge"),
            "#!/bin/sh\nexit 1\n",
        );
        write_exec(
            &root.join("usr/lib/openflux/bridge/openflux-autolaunch.sh"),
            "#!/bin/sh\nexit 1\n",
        );
        let st = Command::new("sh")
            .arg(&postinst)
            .env("OPENFLUX_SYSROOT", &root)
            .env("OPENFLUX_SKIP_SYNC", "1")
            .env("OPENFLUX_STOP_WAIT_ITERS", "1")
            .env("OPENFLUX_POSTINST_SLEEP", "0.05")
            .env("OPENFLUX_READY_WAIT_ITERS", "2")
            .env("OPENFLUX_STARTUP_CHECK_SLEEP", "0.05")
            .env("OPENFLUX_READY_FILE", root.join("tmp/ready"))
            .status()
            .unwrap();
        assert!(!st.success(), "postinst must fail on broken candidate");
        assert!(
            fs::read_to_string(root.join("usr/lib/openflux/bridge/openflux-bridge"))
                .unwrap()
                .contains("old-bridge-0.9")
        );
        assert!(!root
            .join("home/root/.openflux/rollback/bridge/armed")
            .exists());
        let _ = fs::remove_dir_all(&root);
    }

    // Case B: Watchdog timeout rollback -> rolls back after USB confirm timeout.
    {
        let root = make_temp_root("wd_rb");
        seed_existing_openflux(&root);
        assert!(Command::new("sh")
            .arg(&preinst)
            .env("OPENFLUX_SYSROOT", &root)
            .env("OPENFLUX_SKIP_SYNC", "1")
            .status()
            .unwrap()
            .success());
        write_exec(
            &root.join("usr/lib/openflux/bridge/openflux-bridge"),
            "#!/bin/sh\n# new-bridge-1.0\necho \"$$\" > \"${OPENFLUX_READY_FILE}\"\nsleep 30\n",
        );
        write_exec(
            &root.join("usr/lib/openflux/bridge/openflux-autolaunch.sh"),
            "#!/bin/sh\n\"${OPENFLUX_SYSROOT}/usr/lib/openflux/bridge/openflux-bridge\"\n",
        );
        let ready_file = root.join("tmp/ready");
        let st = Command::new("sh")
            .arg(&postinst)
            .env("OPENFLUX_SYSROOT", &root)
            .env("OPENFLUX_SKIP_SYNC", "1")
            .env("OPENFLUX_STOP_WAIT_ITERS", "1")
            .env("OPENFLUX_POSTINST_SLEEP", "0.05")
            .env("OPENFLUX_READY_WAIT_ITERS", "20")
            .env("OPENFLUX_STARTUP_CHECK_SLEEP", "0.05")
            .env("OPENFLUX_STABILIZE_SLEEP", "0.05")
            .env("OPENFLUX_USB_CONFIRM_ITERS", "2")
            .env("OPENFLUX_USB_CONFIRM_SLEEP", "0.1")
            .env("OPENFLUX_READY_FILE", &ready_file)
            .status()
            .unwrap();
        assert!(
            st.success(),
            "postinst should succeed once candidate is ready"
        );

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline
            && root
                .join("home/root/.openflux/rollback/bridge/armed")
                .exists()
        {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            !root
                .join("home/root/.openflux/rollback/bridge/armed")
                .exists(),
            "watchdog did not disarm after rollback"
        );
        assert!(
            fs::read_to_string(root.join("usr/lib/openflux/bridge/openflux-bridge"))
                .unwrap()
                .contains("old-bridge-0.9")
        );
        let _ = fs::remove_dir_all(&root);
    }

    // Case C: Mocked cp failure keeps `armed` and snapshots intact across immediate, watchdog, and boot rollback.
    for mode in ["immediate", "watchdog", "boot"] {
        let root = make_temp_root(&format!("cp_fail_{mode}"));
        seed_existing_openflux(&root);
        assert!(Command::new("sh")
            .arg(&preinst)
            .env("OPENFLUX_SYSROOT", &root)
            .env("OPENFLUX_SKIP_SYNC", "1")
            .status()
            .unwrap()
            .success());

        let fake_bin = root.join("fake_bin");
        write_exec(
            &fake_bin.join("cp"),
            "#!/bin/sh\nfor a in \"$@\"; do\n  case \"$a\" in\n    */rollback/bridge/*) exit 1 ;;\n  esac\ndone\nexec /bin/cp \"$@\"\n",
        );
        let path_env = format!("{}:{}", fake_bin.display(), std::env::var("PATH").unwrap());

        if mode == "immediate" {
            write_exec(
                &root.join("usr/lib/openflux/bridge/openflux-bridge"),
                "#!/bin/sh\nexit 1\n",
            );
            write_exec(
                &root.join("usr/lib/openflux/bridge/openflux-autolaunch.sh"),
                "#!/bin/sh\nexit 1\n",
            );
            let st = Command::new("sh")
                .arg(&postinst)
                .env("PATH", &path_env)
                .env("OPENFLUX_SYSROOT", &root)
                .env("OPENFLUX_SKIP_SYNC", "1")
                .env("OPENFLUX_STOP_WAIT_ITERS", "1")
                .env("OPENFLUX_POSTINST_SLEEP", "0.05")
                .env("OPENFLUX_READY_WAIT_ITERS", "2")
                .env("OPENFLUX_STARTUP_CHECK_SLEEP", "0.05")
                .env("OPENFLUX_READY_FILE", root.join("tmp/ready"))
                .status()
                .unwrap();
            assert!(!st.success());
            assert!(
                root.join("home/root/.openflux/rollback/bridge/armed")
                    .exists(),
                "immediate copy failure must preserve armed"
            );
            assert!(root
                .join("home/root/.openflux/rollback/bridge/openflux-bridge")
                .exists());
        } else if mode == "watchdog" {
            write_exec(
                &root.join("usr/lib/openflux/bridge/openflux-bridge"),
                "#!/bin/sh\necho \"$$\" > \"${OPENFLUX_READY_FILE}\"\nsleep 30\n",
            );
            write_exec(
                &root.join("usr/lib/openflux/bridge/openflux-autolaunch.sh"),
                "#!/bin/sh\n\"${OPENFLUX_SYSROOT}/usr/lib/openflux/bridge/openflux-bridge\"\n",
            );
            let st = Command::new("sh")
                .arg(&postinst)
                .env("PATH", &path_env)
                .env("OPENFLUX_SYSROOT", &root)
                .env("OPENFLUX_SKIP_SYNC", "1")
                .env("OPENFLUX_STOP_WAIT_ITERS", "1")
                .env("OPENFLUX_POSTINST_SLEEP", "0.05")
                .env("OPENFLUX_READY_WAIT_ITERS", "20")
                .env("OPENFLUX_STARTUP_CHECK_SLEEP", "0.05")
                .env("OPENFLUX_STABILIZE_SLEEP", "0.05")
                .env("OPENFLUX_USB_CONFIRM_ITERS", "2")
                .env("OPENFLUX_USB_CONFIRM_SLEEP", "0.1")
                .env("OPENFLUX_READY_FILE", root.join("tmp/ready"))
                .status()
                .unwrap();
            assert!(st.success());
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut logged_err = false;
            while Instant::now() < deadline {
                if let Ok(log) =
                    fs::read_to_string(root.join("home/root/openflux/logs/bridge_rollback.log"))
                {
                    if log.contains("watchdog rollback copy/flush failed") {
                        logged_err = true;
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            assert!(logged_err, "watchdog copy failure was not recorded");
            assert!(
                root.join("home/root/.openflux/rollback/bridge/armed")
                    .exists(),
                "watchdog copy failure must preserve armed"
            );
            let _ = Command::new("pkill")
                .args(["-KILL", "-f", &root.to_string_lossy()])
                .status();
        } else {
            let st = Command::new("sh")
                .arg(&autolaunch)
                .env("PATH", &path_env)
                .env("OPENFLUX_SYSROOT", &root)
                .env("OPENFLUX_SKIP_SYNC", "1")
                .status()
                .unwrap();
            assert!(
                !st.success(),
                "boot rollback must exit nonzero when cp fails"
            );
            assert!(
                root.join("home/root/.openflux/rollback/bridge/armed")
                    .exists(),
                "boot copy failure must preserve armed"
            );
        }
        let _ = fs::remove_dir_all(&root);
    }

    // Case D: Successful boot rollback via openflux-autolaunch.sh clears armed and execs restored launcher.
    {
        let root = make_temp_root("boot_rb_ok");
        seed_existing_openflux(&root);
        assert!(Command::new("sh")
            .arg(&preinst)
            .env("OPENFLUX_SYSROOT", &root)
            .env("OPENFLUX_SKIP_SYNC", "1")
            .status()
            .unwrap()
            .success());
        write_exec(
            &root.join("usr/lib/openflux/bridge/openflux-bridge"),
            "#!/bin/sh\n# new-1.0\n",
        );
        let st = Command::new("sh")
            .arg(&autolaunch)
            .env("OPENFLUX_SYSROOT", &root)
            .env("OPENFLUX_SKIP_SYNC", "1")
            .status()
            .unwrap();
        assert!(st.success(), "boot rollback failed");
        assert!(!root
            .join("home/root/.openflux/rollback/bridge/armed")
            .exists());
        assert!(
            fs::read_to_string(root.join("usr/lib/openflux/bridge/openflux-bridge"))
                .unwrap()
                .contains("old-bridge-0.9")
        );
        assert!(fs::read_to_string(root.join("tmp/launch_trace.log"))
            .unwrap()
            .contains("old-launcher-0.9"));
        let _ = fs::remove_dir_all(&root);
    }

    // Case D2: An armed sentinel without a rollback snapshot keeps the sentinel and still starts
    // the installed bridge, so the host can reach the keyboard.
    {
        let root = make_temp_root("boot_rb_no_snapshot");
        let armed = root.join("home/root/.openflux/rollback/bridge/armed");
        fs::create_dir_all(armed.parent().unwrap()).unwrap();
        fs::write(&armed, "pending-host-usb-confirm\n").unwrap();
        write_exec(
            &root.join("usr/lib/openflux/bridge/openflux-bridge"),
            "#!/bin/sh\necho started >> \"${OPENFLUX_SYSROOT}/tmp/bridge.started\"\nsleep 2\n",
        );

        use std::os::unix::process::CommandExt;
        let mut child = unsafe {
            Command::new("sh")
                .arg(&autolaunch)
                .env("OPENFLUX_SYSROOT", &root)
                .env("OPENFLUX_SKIP_SYNC", "1")
                .pre_exec(|| {
                    libc::setpgid(0, 0);
                    Ok(())
                })
                .spawn()
                .unwrap()
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !root.join("tmp/bridge.started").exists() {
            std::thread::sleep(Duration::from_millis(30));
        }
        unsafe {
            libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
        }
        let _ = child.wait();

        assert!(
            root.join("tmp/bridge.started").is_file(),
            "installed bridge was not started without a rollback snapshot"
        );
        assert!(
            armed.exists(),
            "armed sentinel must stay without a snapshot"
        );
        assert!(
            fs::read_to_string(root.join("home/root/openflux/logs/autolaunch.log"))
                .unwrap()
                .contains("rollback snapshot missing")
        );
        let _ = fs::remove_dir_all(&root);
    }

    // Case E: A flush failure before disarming preserves the armed sentinel and the recovery launcher at OS_BOOT_LAUNCHER.
    {
        let root = make_temp_root("sync_fail");
        seed_existing_openflux(&root);
        assert!(Command::new("sh")
            .arg(&preinst)
            .env("OPENFLUX_SYSROOT", &root)
            .env("OPENFLUX_SKIP_SYNC", "1")
            .status()
            .unwrap()
            .success());
        let os_launcher = root.join("usr/bin/flux-weston-autolaunch.sh");
        fs::copy(&autolaunch, &os_launcher).unwrap();
        fs::set_permissions(&os_launcher, fs::Permissions::from_mode(0o755)).unwrap();

        let fake_bin = root.join("fake_bin");
        write_exec(&fake_bin.join("sync"), "#!/bin/sh\nexit 1\n");
        let path_env = format!("{}:{}", fake_bin.display(), std::env::var("PATH").unwrap());
        let st = Command::new("sh")
            .arg(&os_launcher)
            .env("PATH", &path_env)
            .env("OPENFLUX_SYSROOT", &root)
            .status()
            .unwrap();
        assert!(
            !st.success(),
            "boot rollback must fail when sync fails before disarming"
        );
        assert!(
            root.join("home/root/.openflux/rollback/bridge/armed")
                .exists(),
            "failed sync must preserve armed"
        );
        let retry_st = Command::new("sh")
            .arg(&os_launcher)
            .env("OPENFLUX_SYSROOT", &root)
            .env("OPENFLUX_SKIP_SYNC", "1")
            .status()
            .unwrap();
        assert!(
            retry_st.success(),
            "recovery retry via OS_BOOT_LAUNCHER after sync failure failed"
        );
        assert!(!root
            .join("home/root/.openflux/rollback/bridge/armed")
            .exists());
        assert!(
            fs::read_to_string(root.join("usr/lib/openflux/bridge/openflux-bridge"))
                .unwrap()
                .contains("old-bridge-0.9")
        );
        assert!(fs::read_to_string(root.join("tmp/launch_trace.log"))
            .unwrap()
            .contains("old-launcher-0.9"));
        let _ = fs::remove_dir_all(&root);
    }
}

#[test]
fn restore_factory_binds_manifest_checks_data_tar_and_rescues_on_opkg_install_failure() {
    let pkg_dir = repo_bridge_dir();
    let restore_sh = pkg_dir.join("packaging/restore-factory.sh");

    let setup_sysroot = |name: &str| -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let root = make_temp_root(name);
        write_exec(
            &root.join("usr/lib/openflux/bridge/openflux-bridge"),
            "#!/bin/sh\necho rescued-bridge >> \"${OPENFLUX_SYSROOT}/tmp/rescue.log\"\n",
        );
        write_exec(
            &root.join("usr/lib/openflux/bridge/openflux-autolaunch.sh"),
            "#!/bin/sh\necho rescued-autolaunch >> \"${OPENFLUX_SYSROOT}/tmp/rescue.log\"\n",
        );
        write_exec(
            &root.join("usr/lib/openflux/display/openflux-display"),
            "#!/bin/sh\nexit 0\n",
        );
        fs::copy(
            root.join("usr/lib/openflux/bridge/openflux-autolaunch.sh"),
            root.join("usr/bin/flux-weston-autolaunch.sh"),
        )
        .unwrap();
        fs::set_permissions(
            root.join("usr/bin/flux-weston-autolaunch.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();

        let restore_dir = root.join("home/root/.openflux/factory-restore");
        fs::create_dir_all(&restore_dir).unwrap();
        let b_ipk = restore_dir.join("garfield_1.4.11_aarch64.ipk");
        let d_ipk = restore_dir.join("garfunkel_1.4.28_aarch64.ipk");
        build_mock_ipk(&b_ipk, "garfield", false);
        build_mock_ipk(&d_ipk, "garfunkel", false);
        let manifest = format!(
            "{}  {}\n{}  {}\n",
            sha256_file(&b_ipk),
            b_ipk.file_name().unwrap().to_string_lossy(),
            sha256_file(&d_ipk),
            d_ipk.file_name().unwrap().to_string_lossy(),
        );
        fs::write(restore_dir.join("manifest.sha256"), manifest).unwrap();
        (root, restore_dir, b_ipk, d_ipk)
    };

    // 1. Negative preflight: extra unverified garfield_*.ipk alongside valid ones must abort before touching OpenFlux.
    {
        let (root, restore_dir, _, _) = setup_sysroot("restore_dup");
        build_mock_ipk(
            &restore_dir.join("garfield_1.4.12_aarch64.ipk"),
            "garfield",
            false,
        );
        let st = Command::new("sh")
            .args([restore_sh.to_str().unwrap(), restore_dir.to_str().unwrap()])
            .env("OPENFLUX_SYSROOT", &root)
            .env("OPENFLUX_STOP_SLEEP", "0.05")
            .status()
            .unwrap();
        assert!(!st.success(), "duplicate bridge IPK must fail preflight");
        assert!(root
            .join("usr/lib/openflux/bridge/openflux-bridge")
            .exists());
        assert!(!root.join("tmp/openflux_updating_bridge").exists());
        let _ = fs::remove_dir_all(&root);
    }

    // 2. Negative preflight: a valid manifest checksum on an unrelated file while the selected IPK has no manifest entry.
    {
        let (root, restore_dir, b_ipk, d_ipk) = setup_sysroot("restore_unbound");
        let extra = restore_dir.join("unrelated.txt");
        fs::write(&extra, b"ok\n").unwrap();
        fs::write(
            restore_dir.join("manifest.sha256"),
            format!(
                "{}  unrelated.txt\n{}  {}\n",
                sha256_file(&extra),
                sha256_file(&d_ipk),
                d_ipk.file_name().unwrap().to_string_lossy(),
            ),
        )
        .unwrap();
        assert!(b_ipk.exists());
        let st = Command::new("sh")
            .args([restore_sh.to_str().unwrap(), restore_dir.to_str().unwrap()])
            .env("OPENFLUX_SYSROOT", &root)
            .env("OPENFLUX_STOP_SLEEP", "0.05")
            .status()
            .unwrap();
        assert!(
            !st.success(),
            "selected IPK without manifest entry must fail preflight"
        );
        assert!(root
            .join("usr/lib/openflux/bridge/openflux-bridge")
            .exists());
        let _ = fs::remove_dir_all(&root);
    }

    // 3. Negative preflight: valid control.tar.gz + valid manifest checksum, but a corrupt data.tar.gz fails before removal.
    {
        let (root, restore_dir, b_ipk, d_ipk) = setup_sysroot("restore_bad_data");
        build_mock_ipk(&b_ipk, "garfield", true);
        let manifest = format!(
            "{}  {}\n{}  {}\n",
            sha256_file(&b_ipk),
            b_ipk.file_name().unwrap().to_string_lossy(),
            sha256_file(&d_ipk),
            d_ipk.file_name().unwrap().to_string_lossy(),
        );
        fs::write(restore_dir.join("manifest.sha256"), manifest).unwrap();
        let st = Command::new("sh")
            .args([restore_sh.to_str().unwrap(), restore_dir.to_str().unwrap()])
            .env("OPENFLUX_SYSROOT", &root)
            .env("OPENFLUX_STOP_SLEEP", "0.05")
            .status()
            .unwrap();
        assert!(!st.success(), "corrupt data.tar.gz must fail preflight");
        assert!(root
            .join("usr/lib/openflux/bridge/openflux-bridge")
            .exists());
        assert!(!root.join("tmp/openflux_updating_bridge").exists());
        let _ = fs::remove_dir_all(&root);
    }

    // 3b. Negative preflight: a truncated data.tar.gz (lists the launcher on tar -tzf, then fails) aborts before opkg/rescue.
    {
        let (root, restore_dir, b_ipk, d_ipk) = setup_sysroot("restore_trunc_data");
        let trunc_stage = restore_dir.join("_trunc_stage");
        fs::create_dir_all(&trunc_stage).unwrap();
        assert!(Command::new("ar")
            .args(["x", b_ipk.to_str().unwrap()])
            .current_dir(&trunc_stage)
            .status()
            .unwrap()
            .success());
        let data_gz = trunc_stage.join("data.tar.gz");
        fs::set_permissions(&data_gz, fs::Permissions::from_mode(0o644)).unwrap();
        let gz_bytes = fs::read(&data_gz).unwrap();
        // Truncate the tail off the gzip stream (keeps header + early entries, fails CRC/EOF at end).
        let cut_len = (gz_bytes.len() * 3 / 4).max(32);
        fs::write(&data_gz, &gz_bytes[..cut_len]).unwrap();
        fs::remove_file(&b_ipk).unwrap();
        assert!(Command::new("ar")
            .args([
                "rc",
                b_ipk.to_str().unwrap(),
                "debian-binary",
                "control.tar.gz",
                "data.tar.gz"
            ])
            .current_dir(&trunc_stage)
            .status()
            .unwrap()
            .success());
        let _ = fs::remove_dir_all(&trunc_stage);
        let manifest = format!(
            "{}  {}\n{}  {}\n",
            sha256_file(&b_ipk),
            b_ipk.file_name().unwrap().to_string_lossy(),
            sha256_file(&d_ipk),
            d_ipk.file_name().unwrap().to_string_lossy(),
        );
        fs::write(restore_dir.join("manifest.sha256"), manifest).unwrap();
        let st = Command::new("sh")
            .args([restore_sh.to_str().unwrap(), restore_dir.to_str().unwrap()])
            .env("OPENFLUX_SYSROOT", &root)
            .env("OPENFLUX_STOP_SLEEP", "0.05")
            .status()
            .unwrap();
        assert!(
            !st.success(),
            "truncated data.tar.gz must fail preflight before rescue/opkg"
        );
        assert!(!root.join("home/root/.openflux/rescue").exists());
        assert!(!root.join("tmp/openflux_updating_bridge").exists());
        let _ = fs::remove_dir_all(&root);
    }

    // 3c. Negative preflight: valid data.tar.gz + truncated control.tar.gz (emits valid Package/Arch, then fails at gzip
    //     EOF) also aborts before rescue/opkg.
    {
        let (root, restore_dir, b_ipk, d_ipk) = setup_sysroot("restore_trunc_ctrl");
        let trunc_stage = restore_dir.join("_trunc_ctrl_stage");
        fs::create_dir_all(&trunc_stage).unwrap();
        assert!(Command::new("ar")
            .args(["x", b_ipk.to_str().unwrap()])
            .current_dir(&trunc_stage)
            .status()
            .unwrap()
            .success());
        let ctrl_gz = trunc_stage.join("control.tar.gz");
        fs::set_permissions(&ctrl_gz, fs::Permissions::from_mode(0o644)).unwrap();
        let gz_bytes = fs::read(&ctrl_gz).unwrap();
        // Strip the final 4 bytes (gzip ISIZE trailer) so tar -xzO emits the control text, then fails at CRC/EOF.
        assert!(gz_bytes.len() > 8);
        fs::write(&ctrl_gz, &gz_bytes[..gz_bytes.len() - 4]).unwrap();
        fs::remove_file(&b_ipk).unwrap();
        assert!(Command::new("ar")
            .args([
                "rc",
                b_ipk.to_str().unwrap(),
                "debian-binary",
                "control.tar.gz",
                "data.tar.gz"
            ])
            .current_dir(&trunc_stage)
            .status()
            .unwrap()
            .success());
        let _ = fs::remove_dir_all(&trunc_stage);
        let manifest = format!(
            "{}  {}\n{}  {}\n",
            sha256_file(&b_ipk),
            b_ipk.file_name().unwrap().to_string_lossy(),
            sha256_file(&d_ipk),
            d_ipk.file_name().unwrap().to_string_lossy(),
        );
        fs::write(restore_dir.join("manifest.sha256"), manifest).unwrap();
        let st = Command::new("sh")
            .args([restore_sh.to_str().unwrap(), restore_dir.to_str().unwrap()])
            .env("OPENFLUX_SYSROOT", &root)
            .env("OPENFLUX_STOP_SLEEP", "0.05")
            .status()
            .unwrap();
        assert!(
            !st.success(),
            "truncated control.tar.gz must fail preflight before rescue/opkg"
        );
        assert!(!root.join("home/root/.openflux/rescue").exists());
        assert!(!root.join("tmp/openflux_updating_bridge").exists());
        let _ = fs::remove_dir_all(&root);
    }

    // 4. Partial restore failure: the garfield postinst overwrites the launcher and starts the factory daemon,
    //    then garfunkel fails -> restore-factory stops the factory daemon and reinstates OpenFlux.
    {
        let (root, _restore_dir, _, _) = setup_sysroot("restore_opkg_live_fail");
        fs::copy(
            &restore_sh,
            root.join("usr/lib/openflux/bridge/restore-factory.sh"),
        )
        .unwrap();
        fs::set_permissions(
            root.join("usr/lib/openflux/bridge/restore-factory.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let fake_bin = root.join("fake_bin");
        let fail_flag = root.join("tmp/fail_once");
        fs::write(&fail_flag, b"1").unwrap();
        write_exec(
            &fake_bin.join("opkg"),
            &format!(
                r#"#!/bin/sh
if [ "$1" = "remove" ]; then
    rm -f "${{OPENFLUX_SYSROOT}}/usr/lib/openflux/bridge/openflux-bridge" \
          "${{OPENFLUX_SYSROOT}}/usr/lib/openflux/bridge/openflux-autolaunch.sh" \
          "${{OPENFLUX_SYSROOT}}/usr/lib/openflux/bridge/restore-factory.sh" \
          "${{OPENFLUX_SYSROOT}}/usr/lib/openflux/display/openflux-display"
    exit 0
fi
if [ -e "{flag}" ]; then
    rm -f "{flag}"
    mkdir -p "${{OPENFLUX_SYSROOT}}/usr/lib/garfield" "${{OPENFLUX_SYSROOT}}/usr/bin"
    cat > "${{OPENFLUX_SYSROOT}}/usr/lib/garfield/garfield" <<'G_EOF'
#!/bin/sh
echo "$$" > "${{OPENFLUX_SYSROOT}}/tmp/live_garfield.pid"
sleep 30
G_EOF
    chmod +x "${{OPENFLUX_SYSROOT}}/usr/lib/garfield/garfield"
    cat > "${{OPENFLUX_SYSROOT}}/usr/bin/flux-weston-autolaunch.sh" <<'L_EOF'
#!/bin/sh
echo "factory-overwrote-launcher" >> "${{OPENFLUX_SYSROOT}}/tmp/factory_ran.log"
"${{OPENFLUX_SYSROOT}}/usr/lib/garfield/garfield" &
wait
L_EOF
    chmod +x "${{OPENFLUX_SYSROOT}}/usr/bin/flux-weston-autolaunch.sh"
    nohup "${{OPENFLUX_SYSROOT}}/usr/bin/flux-weston-autolaunch.sh" </dev/null >/dev/null 2>&1 &
    sleep 0.2
    exit 1
fi
exit 0
"#,
                flag = fail_flag.display()
            ),
        );
        let path_env = format!("{}:{}", fake_bin.display(), std::env::var("PATH").unwrap());
        let st = Command::new("sh")
            .args([
                restore_sh.to_str().unwrap(),
                root.join("home/root/.openflux/factory-restore")
                    .to_str()
                    .unwrap(),
            ])
            .env("PATH", &path_env)
            .env("OPENFLUX_SYSROOT", &root)
            .env("OPENFLUX_SKIP_SYNC", "1")
            .env("OPENFLUX_STOP_SLEEP", "0.1")
            .status()
            .unwrap();
        assert!(
            !st.success(),
            "restore-factory must exit nonzero when opkg install fails"
        );
        assert!(
            root.join("usr/lib/openflux/bridge/openflux-bridge")
                .is_file(),
            "rescued bridge missing"
        );
        assert!(
            root.join("usr/lib/openflux/bridge/openflux-autolaunch.sh")
                .is_file(),
            "rescued launcher missing"
        );
        assert!(
            root.join("usr/lib/openflux/bridge/restore-factory.sh")
                .is_file(),
            "rescued restore-factory.sh missing"
        );
        assert!(
            root.join("usr/lib/openflux/display/openflux-display")
                .is_file(),
            "rescued display missing"
        );
        assert!(
            !root.join("tmp/openflux_updating_bridge").exists(),
            "bridge update semaphore leaked"
        );
        assert!(
            !root.join("tmp/openflux_updating_display").exists(),
            "display update semaphore leaked"
        );
        assert!(
            !root.join("tmp/updating_garfield").exists(),
            "factory update semaphore leaked"
        );
        assert!(
            !root.join("etc/init.d/S05openflux-recovery").exists(),
            "recovery init script left behind after reinstate"
        );
        if let Ok(pid_txt) = fs::read_to_string(root.join("tmp/live_garfield.pid")) {
            let pid = pid_txt.trim();
            if !pid.is_empty() {
                let alive = Command::new("kill")
                    .args(["-0", pid])
                    .status()
                    .unwrap()
                    .success();
                assert!(
                    !alive,
                    "live factory garfield PID {pid} was not stopped during rescue"
                );
            }
        }

        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline && !root.join("tmp/rescue.log").exists() {
            std::thread::sleep(Duration::from_millis(30));
        }
        assert!(fs::read_to_string(root.join("tmp/rescue.log"))
            .unwrap()
            .contains("rescued-autolaunch"));

        // A second restore through the reinstated `/usr/lib/openflux/bridge/restore-factory.sh` succeeds.
        let st_retry = Command::new("sh")
            .args([
                root.join("usr/lib/openflux/bridge/restore-factory.sh")
                    .to_str()
                    .unwrap(),
                root.join("home/root/.openflux/factory-restore")
                    .to_str()
                    .unwrap(),
            ])
            .env("PATH", &path_env)
            .env("OPENFLUX_SYSROOT", &root)
            .env("OPENFLUX_SKIP_SYNC", "1")
            .env("OPENFLUX_STOP_SLEEP", "0.05")
            .status()
            .unwrap();
        assert!(
            st_retry.success(),
            "second restore retry failed after live rescue"
        );
        let deadline2 = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline2 && !root.join("tmp/boot_trace.log").exists() {
            std::thread::sleep(Duration::from_millis(30));
        }
        assert!(fs::read_to_string(root.join("tmp/boot_trace.log"))
            .unwrap()
            .contains("factory-autolaunch"));
        let _ = fs::remove_dir_all(&root);
    }

    // 5. Power loss right after garfield overwrote OS_BOOT_LAUNCHER (before garfunkel finishes): boot runs
    //    /etc/init.d/S05openflux-recovery before the Weston autolaunch -> OpenFlux is reinstated.
    {
        let (root, restore_dir, _, _) = setup_sysroot("restore_powerloss_after_overwrite");
        fs::copy(
            &restore_sh,
            root.join("usr/lib/openflux/bridge/restore-factory.sh"),
        )
        .unwrap();
        fs::set_permissions(
            root.join("usr/lib/openflux/bridge/restore-factory.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let fake_bin = root.join("fake_bin");
        let kill_flag = root.join("tmp/kill_once");
        fs::write(&kill_flag, b"1").unwrap();
        write_exec(
            &fake_bin.join("opkg"),
            &format!(
                r#"#!/bin/sh
if [ "$1" = "remove" ]; then
    rm -f "${{OPENFLUX_SYSROOT}}/usr/lib/openflux/bridge/openflux-bridge" \
          "${{OPENFLUX_SYSROOT}}/usr/lib/openflux/bridge/openflux-autolaunch.sh" \
          "${{OPENFLUX_SYSROOT}}/usr/lib/openflux/bridge/restore-factory.sh" \
          "${{OPENFLUX_SYSROOT}}/usr/lib/openflux/display/openflux-display"
    exit 0
fi
if [ -e "{flag}" ]; then
    rm -f "{flag}"
    cat > "${{OPENFLUX_SYSROOT}}/usr/bin/flux-weston-autolaunch.sh" <<'L_EOF'
#!/bin/sh
echo "half-restored-factory-launcher" >> "${{OPENFLUX_SYSROOT}}/tmp/factory_half.log"
L_EOF
    chmod +x "${{OPENFLUX_SYSROOT}}/usr/bin/flux-weston-autolaunch.sh"
    kill -KILL "$PPID"
    exit 137
fi
exit 0
"#,
                flag = kill_flag.display()
            ),
        );
        let path_env = format!("{}:{}", fake_bin.display(), std::env::var("PATH").unwrap());
        let st = Command::new("sh")
            .args([restore_sh.to_str().unwrap(), restore_dir.to_str().unwrap()])
            .env("PATH", &path_env)
            .env("OPENFLUX_SYSROOT", &root)
            .env("OPENFLUX_SKIP_SYNC", "1")
            .env("OPENFLUX_STOP_SLEEP", "0.05")
            .status()
            .unwrap();
        assert!(!st.success());
        assert!(!root
            .join("usr/lib/openflux/bridge/openflux-bridge")
            .exists());
        assert!(root.join("home/root/.openflux/rescue/armed").is_file());
        assert!(root.join("etc/init.d/S05openflux-recovery").is_file());
        assert!(
            fs::read_to_string(root.join("usr/bin/flux-weston-autolaunch.sh"))
                .unwrap()
                .contains("half-restored-factory-launcher")
        );

        write_exec(&fake_bin.join("sync"), "#!/bin/sh\nexit 1\n");
        let fail_boot = Command::new("sh")
            .args([
                root.join("etc/init.d/S05openflux-recovery")
                    .to_str()
                    .unwrap(),
                "start",
            ])
            .env("PATH", &path_env)
            .env("OPENFLUX_SYSROOT", &root)
            .status()
            .unwrap();
        assert!(
            !fail_boot.success(),
            "S05openflux-recovery must fail closed when sync fails"
        );
        assert!(
            root.join("home/root/.openflux/rescue/armed").is_file(),
            "sync failure must preserve rescue/armed"
        );

        let rec_st = Command::new("sh")
            .args([
                root.join("etc/init.d/S05openflux-recovery")
                    .to_str()
                    .unwrap(),
                "start",
            ])
            .env("OPENFLUX_SYSROOT", &root)
            .env("OPENFLUX_SKIP_SYNC", "1")
            .env("OPENFLUX_STOP_SLEEP", "0.05")
            .status()
            .unwrap();
        assert!(rec_st.success(), "S05openflux-recovery failed");
        assert!(!root.join("home/root/.openflux/rescue/armed").exists());
        assert!(!root.join("etc/init.d/S05openflux-recovery").exists());
        assert!(root
            .join("usr/lib/openflux/bridge/openflux-bridge")
            .is_file());
        assert!(root
            .join("usr/lib/openflux/display/openflux-display")
            .is_file());

        let boot_st = Command::new("sh")
            .arg(root.join("usr/bin/flux-weston-autolaunch.sh"))
            .env("OPENFLUX_SYSROOT", &root)
            .status()
            .unwrap();
        assert!(boot_st.success());
        assert!(fs::read_to_string(root.join("tmp/rescue.log"))
            .unwrap()
            .contains("rescued-autolaunch"));
        assert!(
            !root.join("tmp/factory_half.log").exists(),
            "half-restored factory launcher must never execute after recovery"
        );
        assert!(
            root.join("usr/lib/openflux/bridge/restore-factory.sh")
                .is_file(),
            "S05 must reinstate restore-factory.sh"
        );

        // A second restore after boot recovery succeeds through the reinstated restore-factory.sh.
        let st_retry = Command::new("sh")
            .args([
                root.join("usr/lib/openflux/bridge/restore-factory.sh")
                    .to_str()
                    .unwrap(),
                restore_dir.to_str().unwrap(),
            ])
            .env("PATH", &path_env)
            .env("OPENFLUX_SYSROOT", &root)
            .env("OPENFLUX_SKIP_SYNC", "1")
            .env("OPENFLUX_STOP_SLEEP", "0.05")
            .status()
            .unwrap();
        assert!(
            st_retry.success(),
            "second restore retry failed after boot recovery"
        );
        let deadline_r = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline_r && !root.join("tmp/boot_trace.log").exists() {
            std::thread::sleep(Duration::from_millis(30));
        }
        assert!(fs::read_to_string(root.join("tmp/boot_trace.log"))
            .unwrap()
            .contains("factory-autolaunch"));
        let _ = fs::remove_dir_all(&root);
    }

    // 6. Direct-launcher recovery: power loss right after opkg remove, before the factory package overwrites OS_BOOT_LAUNCHER.
    {
        let (root, restore_dir, _, _) = setup_sysroot("restore_direct_launcher");
        let fake_bin = root.join("fake_bin");
        write_exec(
            &fake_bin.join("opkg"),
            r#"#!/bin/sh
if [ "$1" = "remove" ]; then
    rm -f "${OPENFLUX_SYSROOT}/usr/lib/openflux/bridge/openflux-bridge" \
          "${OPENFLUX_SYSROOT}/usr/lib/openflux/bridge/openflux-autolaunch.sh" \
          "${OPENFLUX_SYSROOT}/usr/lib/openflux/display/openflux-display"
    exit 0
fi
kill -KILL "$PPID"
exit 137
"#,
        );
        let path_env = format!("{}:{}", fake_bin.display(), std::env::var("PATH").unwrap());
        let st = Command::new("sh")
            .args([restore_sh.to_str().unwrap(), restore_dir.to_str().unwrap()])
            .env("PATH", &path_env)
            .env("OPENFLUX_SYSROOT", &root)
            .env("OPENFLUX_SKIP_SYNC", "1")
            .env("OPENFLUX_STOP_SLEEP", "0.05")
            .status()
            .unwrap();
        assert!(!st.success());
        assert!(!root
            .join("usr/lib/openflux/bridge/openflux-bridge")
            .exists());
        assert!(root.join("home/root/.openflux/rescue/armed").is_file());

        let direct_st = Command::new("sh")
            .arg(root.join("usr/bin/flux-weston-autolaunch.sh"))
            .env("OPENFLUX_SYSROOT", &root)
            .env("OPENFLUX_SKIP_SYNC", "1")
            .env("OPENFLUX_STOP_SLEEP", "0.05")
            .status()
            .unwrap();
        assert!(
            direct_st.success(),
            "direct launcher recovery killed itself or failed"
        );
        assert!(!root.join("home/root/.openflux/rescue/armed").exists());
        assert!(!root.join("etc/init.d/S05openflux-recovery").exists());
        assert!(root
            .join("usr/lib/openflux/bridge/openflux-bridge")
            .is_file());
        assert!(root
            .join("usr/lib/openflux/display/openflux-display")
            .is_file());
        assert!(fs::read_to_string(root.join("tmp/rescue.log"))
            .unwrap()
            .contains("rescued-autolaunch"));
        let _ = fs::remove_dir_all(&root);
    }

    // 7. Happy path: even if the factory postinst leaves OS_BOOT_LAUNCHER unchanged and returns 0,
    //    restore-factory extracts and installs the launcher from the verified garfield IPK before disarming rescue.
    {
        let (root, restore_dir, _, _) = setup_sysroot("restore_ok");
        // The OpenFlux launcher had set the OS time service's init script aside.
        write_exec(
            &root.join("etc/init.d/disabled_S49chrony"),
            "#!/bin/sh\n# os-time-service\n",
        );
        let fake_bin = root.join("fake_bin");
        write_exec(&fake_bin.join("opkg"), "#!/bin/sh\nexit 0\n");
        let path_env = format!("{}:{}", fake_bin.display(), std::env::var("PATH").unwrap());
        let st = Command::new("sh")
            .args([restore_sh.to_str().unwrap(), restore_dir.to_str().unwrap()])
            .env("PATH", &path_env)
            .env("OPENFLUX_SYSROOT", &root)
            .env("OPENFLUX_SKIP_SYNC", "1")
            .env("OPENFLUX_STOP_SLEEP", "0.05")
            .status()
            .unwrap();
        assert!(st.success(), "restore-factory happy path failed");
        assert!(!root.join("etc/init.d/disabled_S49chrony").exists());
        assert!(fs::read_to_string(root.join("etc/init.d/S49chrony"))
            .unwrap()
            .contains("os-time-service"));
        assert!(!root.join("home/root/.openflux/rescue").exists());
        assert!(!root.join("etc/init.d/S05openflux-recovery").exists());
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline && !root.join("tmp/boot_trace.log").exists() {
            std::thread::sleep(Duration::from_millis(30));
        }
        assert!(fs::read_to_string(root.join("tmp/boot_trace.log"))
            .unwrap()
            .contains("factory-autolaunch"));
        let _ = fs::remove_dir_all(&root);
    }

    // 8. openflux-factory-restore postinst: rejects an unresolved opkg owner even under `var/lib/opkg/info/`,
    //    rejects tampered packages, and waits for the outer `opkg -> sh` ancestor without OPENFLUX_OPKG_LOCK.
    {
        let fr_postinst = pkg_dir.join("factory-restore-packaging/postinst");
        let (root, _restore_dir, b_ipk, _) = setup_sysroot("factory_restore_pkg");
        fs::copy(
            &restore_sh,
            root.join("usr/lib/openflux/bridge/restore-factory.sh"),
        )
        .unwrap();
        fs::set_permissions(
            root.join("usr/lib/openflux/bridge/restore-factory.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();

        let fake_bin = root.join("fake_bin");
        write_exec(
            &fake_bin.join("opkg"),
            r#"#!/bin/sh
if [ "$1" = "--outer-sh" ]; then
    sh -c 'sh "$1"' sh "$2" || exit 1
    sleep "${OUTER_HOLD_SEC:-0.35}"
    exit 0
fi
if [ "$1" = "install" ]; then
    cat > "${OPENFLUX_SYSROOT}/usr/bin/flux-weston-autolaunch.sh" <<'F_EOF'
#!/bin/sh
echo "factory-autolaunch" >> "${OPENFLUX_SYSROOT}/tmp/boot_trace.log"
F_EOF
    chmod +x "${OPENFLUX_SYSROOT}/usr/bin/flux-weston-autolaunch.sh"
fi
exit 0
"#,
        );
        let path_env = format!("{}:{}", fake_bin.display(), std::env::var("PATH").unwrap());

        // 8a: Placed under `var/lib/opkg/info/...` and run via `sh -c` without an opkg executable -> fails closed.
        let info_dir = root.join("var/lib/opkg/info");
        fs::create_dir_all(&info_dir).unwrap();
        let placed_postinst = info_dir.join("openflux-factory-restore.postinst");
        fs::copy(&fr_postinst, &placed_postinst).unwrap();
        fs::set_permissions(&placed_postinst, fs::Permissions::from_mode(0o755)).unwrap();
        let no_opkg_st = Command::new("sh")
            .args(["-c", "sh \"$1\"", "sh", placed_postinst.to_str().unwrap()])
            .env("PATH", &path_env)
            .env("OPENFLUX_SYSROOT", &root)
            .status()
            .unwrap();
        assert!(
            !no_opkg_st.success(),
            "postinst must fail closed when invoked under opkg/info without real opkg ancestor"
        );
        assert!(root
            .join("usr/lib/openflux/bridge/openflux-bridge")
            .is_file());

        // 8b: A tampered staged bridge IPK -> postinst exits nonzero and leaves OpenFlux untouched.
        let saved_ipk = fs::read(&b_ipk).unwrap();
        fs::write(&b_ipk, b"tampered").unwrap();
        let bad_st = Command::new(fake_bin.join("opkg"))
            .args(["--outer-sh", placed_postinst.to_str().unwrap()])
            .env("PATH", &path_env)
            .env("OPENFLUX_SYSROOT", &root)
            .status()
            .unwrap();
        assert!(
            !bad_st.success(),
            "factory-restore postinst must reject tampered staged IPK"
        );
        assert!(root
            .join("usr/lib/openflux/bridge/openflux-bridge")
            .is_file());
        assert!(!root
            .join("home/root/openflux/logs/factory_restore.log")
            .exists());

        // 8c: Valid IPK again -> an opkg -> sh wrapper holds the outer opkg PID for 0.35s with no lock override;
        //     the detached worker waits for the outer opkg PID to exit before running restore-factory.sh.
        fs::write(&b_ipk, &saved_ipk).unwrap();
        let ok_st = Command::new(fake_bin.join("opkg"))
            .args(["--outer-sh", placed_postinst.to_str().unwrap()])
            .env("PATH", &path_env)
            .env("OPENFLUX_SYSROOT", &root)
            .env("OPENFLUX_SKIP_SYNC", "1")
            .env("OPENFLUX_STOP_SLEEP", "0.05")
            .env("OPENFLUX_OPKG_WAIT_ITERS", "40")
            .env("OPENFLUX_OPKG_WAIT_SLEEP", "0.05")
            .env("OUTER_HOLD_SEC", "0.35")
            .status()
            .unwrap();
        assert!(
            ok_st.success(),
            "factory-restore postinst failed on valid staged files"
        );
        let deadline = Instant::now() + Duration::from_secs(4);
        while Instant::now() < deadline && !root.join("tmp/boot_trace.log").exists() {
            std::thread::sleep(Duration::from_millis(30));
        }
        assert!(fs::read_to_string(root.join("tmp/boot_trace.log"))
            .unwrap()
            .contains("factory-autolaunch"));
        let _ = fs::remove_dir_all(&root);
    }

    // 9. The production default path (no OPENFLUX_OPKG_LOCK override) with a shell intermediary and an outer
    //    transaction: a held opkg lock is waited for, within a bounded timeout.
    {
        let fr_postinst = pkg_dir.join("factory-restore-packaging/postinst");
        let (root, _restore_dir, _, _) = setup_sysroot("factory_restore_opkg_lock");
        fs::copy(
            &restore_sh,
            root.join("usr/lib/openflux/bridge/restore-factory.sh"),
        )
        .unwrap();
        fs::set_permissions(
            root.join("usr/lib/openflux/bridge/restore-factory.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();

        let info_dir = root.join("var/lib/opkg/info");
        fs::create_dir_all(&info_dir).unwrap();
        let placed_postinst = info_dir.join("openflux-factory-restore.postinst");
        fs::copy(&fr_postinst, &placed_postinst).unwrap();
        fs::set_permissions(&placed_postinst, fs::Permissions::from_mode(0o755)).unwrap();

        let fake_bin = root.join("fake_bin");
        let lock_file = root.join("var/lock/opkg.lock");
        fs::create_dir_all(lock_file.parent().unwrap()).unwrap();
        write_exec(
            &fake_bin.join("opkg"),
            &format!(
                r#"#!/bin/sh
LOCK="{lock}"
if [ "$1" = "--outer-install" ]; then
    touch "$LOCK"
    sh -c 'sh "$1"' sh "$2" || exit 1
    sleep "${{OUTER_HOLD_SEC:-0.35}}"
    rm -f "$LOCK"
    exit 0
fi
if [ -e "$LOCK" ]; then
    echo "NESTED_OPKG_LOCK_VIOLATION" >> "{viol}"
    exit 99
fi
if [ "$1" = "install" ]; then
    cat > "${{OPENFLUX_SYSROOT}}/usr/bin/flux-weston-autolaunch.sh" <<'F_EOF'
#!/bin/sh
echo "factory-autolaunch" >> "${{OPENFLUX_SYSROOT}}/tmp/boot_trace.log"
F_EOF
    chmod +x "${{OPENFLUX_SYSROOT}}/usr/bin/flux-weston-autolaunch.sh"
fi
exit 0
"#,
                lock = lock_file.display(),
                viol = root.join("tmp/lock_violation.log").display()
            ),
        );
        let path_env = format!("{}:{}", fake_bin.display(), std::env::var("PATH").unwrap());

        let st_ok = Command::new(fake_bin.join("opkg"))
            .args(["--outer-install", placed_postinst.to_str().unwrap()])
            .env("PATH", &path_env)
            .env("OPENFLUX_SYSROOT", &root)
            .env("OPENFLUX_OPKG_WAIT_ITERS", "40")
            .env("OPENFLUX_OPKG_WAIT_SLEEP", "0.05")
            .env("OPENFLUX_SKIP_SYNC", "1")
            .env("OPENFLUX_STOP_SLEEP", "0.05")
            .env("OUTER_HOLD_SEC", "0.35")
            .status()
            .unwrap();
        assert!(st_ok.success());
        let deadline = Instant::now() + Duration::from_secs(4);
        while Instant::now() < deadline && !root.join("tmp/boot_trace.log").exists() {
            std::thread::sleep(Duration::from_millis(30));
        }
        assert!(fs::read_to_string(root.join("tmp/boot_trace.log"))
            .unwrap()
            .contains("factory-autolaunch"));
        assert!(
            !root.join("tmp/lock_violation.log").exists(),
            "detached worker ran nested opkg before outer transaction finished"
        );
        let _ = fs::remove_dir_all(&root);

        // 9b: Default `${ROOT}/var/lock/opkg.lock` remains held past bounded wait (no OPENFLUX_OPKG_LOCK override)
        let (root2, _restore_dir2, _, _) = setup_sysroot("factory_restore_opkg_timeout");
        fs::copy(
            &restore_sh,
            root2.join("usr/lib/openflux/bridge/restore-factory.sh"),
        )
        .unwrap();
        fs::set_permissions(
            root2.join("usr/lib/openflux/bridge/restore-factory.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let fake_bin2 = root2.join("fake_bin");
        let lock_file2 = root2.join("var/lock/opkg.lock");
        fs::create_dir_all(lock_file2.parent().unwrap()).unwrap();
        fs::write(&lock_file2, b"held\n").unwrap();
        write_exec(
            &fake_bin2.join("opkg"),
            &format!(
                "#!/bin/sh\necho \"NESTED_OPKG_CALLED\" >> \"{}\"\nexit 99\n",
                root2.join("tmp/lock_violation2.log").display()
            ),
        );
        let path_env2 = format!("{}:{}", fake_bin2.display(), std::env::var("PATH").unwrap());
        let st_to = Command::new("sh")
            .arg(&fr_postinst)
            .env("PATH", &path_env2)
            .env("OPENFLUX_SYSROOT", &root2)
            .env("OPENFLUX_OPKG_WAIT_ITERS", "3")
            .env("OPENFLUX_OPKG_WAIT_SLEEP", "0.05")
            .status()
            .unwrap();
        assert!(st_to.success());
        let deadline2 = Instant::now() + Duration::from_secs(3);
        let mut saw_timeout = false;
        while Instant::now() < deadline2 {
            if let Ok(log) =
                fs::read_to_string(root2.join("home/root/openflux/logs/factory_restore.log"))
            {
                if log.contains("timed out waiting for outer opkg") {
                    saw_timeout = true;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        assert!(saw_timeout, "expected bounded wait timeout log");
        assert!(!root2.join("tmp/lock_violation2.log").exists());
        assert!(root2
            .join("usr/lib/openflux/bridge/openflux-bridge")
            .is_file());
        let _ = fs::remove_dir_all(&root2);
    }
}

#[test]
fn sysroot_scripts_never_touch_host_commands_or_unscoped_patterns() {
    let pkg_dir = repo_bridge_dir();
    let display_prerm = repo_root_dir().join("keyboard/display/packaging/prerm");
    let bridge_prerm = pkg_dir.join("packaging/prerm");
    let autolaunch = pkg_dir.join("packaging/openflux-autolaunch.sh");

    let root = make_temp_root("sysroot_guard");
    let fake_bin = root.join("fake_bin");
    let cmd_log = root.join("tmp/cmd_guard.log");

    for tool in ["pkill", "pgrep", "pidof", "killall"] {
        write_exec(
            &fake_bin.join(tool),
            &format!(
                "#!/bin/sh\necho \"{t}:$*\" >> \"{log}\"\nexit 1\n",
                t = tool,
                log = cmd_log.display()
            ),
        );
    }
    let path_env = format!("{}:{}", fake_bin.display(), std::env::var("PATH").unwrap());

    // 1. Run both prerm scripts under OPENFLUX_SYSROOT and verify zero unscoped pkill patterns.
    assert!(Command::new("sh")
        .arg(&bridge_prerm)
        .env("PATH", &path_env)
        .env("OPENFLUX_SYSROOT", &root)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("sh")
        .arg(&display_prerm)
        .env("PATH", &path_env)
        .env("OPENFLUX_SYSROOT", &root)
        .env("OPENFLUX_STOP_SLEEP", "0.05")
        .status()
        .unwrap()
        .success());

    // 2. Run openflux-autolaunch.sh briefly under OPENFLUX_SYSROOT with staged manifest and dummy binaries.
    write_exec(
        &root.join("usr/lib/openflux/bridge/openflux-bridge"),
        "#!/bin/sh\nsleep 2\n",
    );
    write_exec(
        &root.join("usr/lib/openflux/display/openflux-display"),
        "#!/bin/sh\nsleep 2\n",
    );
    write_exec(
        &root.join("usr/bin/flux-ipk-monitor.sh"),
        "#!/bin/sh\nsleep 2\n",
    );
    write_exec(
        &root.join("tmp/ipk-updates/update-manifest"),
        "#!/bin/sh\necho ran-sysroot-manifest >> \"${OPENFLUX_SYSROOT}/tmp/manifest.ran\"\n",
    );

    use std::os::unix::process::CommandExt;
    let mut child = unsafe {
        Command::new("sh")
            .arg(&autolaunch)
            .env("PATH", &path_env)
            .env("OPENFLUX_SYSROOT", &root)
            .pre_exec(|| {
                libc::setpgid(0, 0);
                Ok(())
            })
            .spawn()
            .unwrap()
    };
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && !root.join("tmp/manifest.ran").exists() {
        std::thread::sleep(Duration::from_millis(30));
    }
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.wait();
    std::thread::sleep(Duration::from_millis(50));

    assert!(
        root.join("tmp/manifest.ran").is_file(),
        "sysroot update-manifest did not run"
    );
    let logged = fs::read_to_string(&cmd_log).unwrap_or_default();
    assert!(
        !logged.contains("pidof:"),
        "openflux-autolaunch invoked pidof under SYSROOT: {logged}"
    );
    assert!(
        !logged.contains("killall:"),
        "openflux-autolaunch invoked killall under SYSROOT: {logged}"
    );
    for line in logged.lines() {
        if line.starts_with("pkill:") || line.starts_with("pgrep:") {
            assert!(
                line.contains(&root.to_string_lossy().to_string()),
                "unscoped process match under SYSROOT: {line}"
            );
        }
    }
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn nondefault_release_version_propagates_to_bridge_binary_and_publishes_trial_marker() {
    let pkg_dir = repo_bridge_dir();
    let raw_preinst = fs::read_to_string(pkg_dir.join("packaging/preinst")).unwrap();
    let raw_postinst = fs::read_to_string(pkg_dir.join("packaging/postinst")).unwrap();
    let root = make_temp_root("nondefault_ver");
    let target_dir = root.join("cargo_target");

    // Compile openflux-bridge with OPENFLUX_VERSION=1.0.1 into an isolated target-dir.
    let build_st = Command::new("cargo")
        .args([
            "build",
            "-p",
            "openflux-bridge",
            "--bin",
            "openflux-bridge",
            "--target-dir",
            target_dir.to_str().unwrap(),
        ])
        .env("OPENFLUX_VERSION", "1.0.1")
        .status()
        .unwrap();
    assert!(
        build_st.success(),
        "cargo build with OPENFLUX_VERSION=1.0.1 failed"
    );
    let bin_101 = target_dir.join("debug/openflux-bridge");
    let ver_out = Command::new(&bin_101).arg("--version").output().unwrap();
    assert!(ver_out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&ver_out.stdout).trim(),
        "openflux-bridge 1.0.1"
    );

    // Seed a 1.0.0 bridge and run the staged 1.0.1 preinst/postinst -> trial-<attempt_id> is published.
    write_exec(
        &root.join("usr/lib/openflux/bridge/openflux-bridge"),
        "#!/bin/sh\n# old-1.0.0\n",
    );
    write_exec(
        &root.join("usr/lib/openflux/bridge/openflux-autolaunch.sh"),
        "#!/bin/sh\nexit 0\n",
    );

    let staged_pre = root.join("tmp/preinst_101");
    let staged_post = root.join("tmp/postinst_101");
    fs::write(
        &staged_pre,
        raw_preinst.replace("@OPENFLUX_VERSION@", "1.0.1"),
    )
    .unwrap();
    fs::write(
        &staged_post,
        raw_postinst.replace("@OPENFLUX_VERSION@", "1.0.1"),
    )
    .unwrap();
    fs::set_permissions(&staged_pre, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(&staged_post, fs::Permissions::from_mode(0o755)).unwrap();

    assert!(Command::new("sh")
        .arg(&staged_pre)
        .env("OPENFLUX_SYSROOT", &root)
        .env("OPENFLUX_SKIP_SYNC", "1")
        .status()
        .unwrap()
        .success());
    let rb_dir = root.join("home/root/.openflux/rollback/bridge");
    let attempt_txt = fs::read_to_string(rb_dir.join("attempt")).unwrap();
    assert!(
        attempt_txt.contains("version=openflux-bridge 1.0.1\n"),
        "got: {attempt_txt}"
    );
    let attempt_id = attempt_txt
        .lines()
        .find_map(|l| l.strip_prefix("attempt_id="))
        .unwrap()
        .to_string();

    write_exec(
        &root.join("usr/lib/openflux/bridge/openflux-bridge"),
        &format!(
            "#!/bin/sh\nexec 3</dev/null 4>/dev/null\nexport OPENFLUX_TEST_EP_FDS=4,3\nexec \"{}\" \"$@\"\n",
            bin_101.display()
        ),
    );
    write_exec(
        &root.join("usr/lib/openflux/bridge/openflux-autolaunch.sh"),
        "#!/bin/sh\nexec \"${OPENFLUX_SYSROOT}/usr/lib/openflux/bridge/openflux-bridge\" \"${OPENFLUX_SYSROOT}/dev/ffs-openflux\"\n",
    );

    let ready_file = root.join("tmp/bridge_ready");
    assert!(Command::new("sh")
        .arg(&staged_post)
        .env("OPENFLUX_SYSROOT", &root)
        .env("OPENFLUX_SKIP_SYNC", "1")
        .env("OPENFLUX_STOP_WAIT_ITERS", "1")
        .env("OPENFLUX_POSTINST_SLEEP", "0.05")
        .env("OPENFLUX_READY_WAIT_ITERS", "40")
        .env("OPENFLUX_STARTUP_CHECK_SLEEP", "0.05")
        .env("OPENFLUX_STABILIZE_SLEEP", "0.05")
        .env("OPENFLUX_USB_CONFIRM_ITERS", "40")
        .env("OPENFLUX_USB_CONFIRM_SLEEP", "0.1")
        .env("OPENFLUX_READY_FILE", &ready_file)
        .status()
        .unwrap()
        .success());

    let trial_path = rb_dir.join(format!("trial-{attempt_id}"));
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && !trial_path.exists() {
        std::thread::sleep(Duration::from_millis(30));
    }
    assert!(
        trial_path.is_file(),
        "1.0.1 candidate did not publish {}",
        trial_path.display()
    );
    assert!(fs::read_to_string(&trial_path)
        .unwrap()
        .contains("version=openflux-bridge 1.0.1"));

    let _ = fs::remove_file(rb_dir.join("armed"));
    if let Ok(pid_s) = fs::read_to_string(&ready_file) {
        let pid = pid_s.trim();
        if !pid.is_empty() {
            let _ = Command::new("kill").args(["-TERM", pid]).status();
        }
    }
    let _ = Command::new("pkill")
        .args(["-KILL", "-f", &root.to_string_lossy()])
        .status();
    let _ = fs::remove_dir_all(&root);
}
