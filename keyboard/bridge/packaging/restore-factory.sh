#!/bin/sh
# Verifies staged downloaded factory packages, removes Keyglass device packages, and installs factory packages
# while keeping an out-of-package Keyglass rescue bridge armed until factory install completes.
set -e

ROOT="${KEYGLASS_SYSROOT:-}"
PREFLIGHT_ONLY=0
if [ "${1:-}" = "--preflight-only" ]; then
    PREFLIGHT_ONLY=1
    shift
fi
RESTORE_DIR="${1:-${ROOT}/home/root/.keyglass/factory-restore}"
RESTORE_MANIFEST="${RESTORE_DIR}/manifest.sha256"
OS_BOOT_LAUNCHER="${ROOT}/usr/bin/flux-weston-autolaunch.sh"
RECOVERY_INIT="${ROOT}/etc/init.d/S05keyglass-recovery"
RESCUE_DIR="${ROOT}/home/root/.keyglass/rescue"
RESCUE_ARMED="${RESCUE_DIR}/armed"
GADGET_PATH="/sys/kernel/config/usb_gadget/g1"

flush_fs() {
    [ "${KEYGLASS_SKIP_SYNC:-0}" = "1" ] && return 0
    sync
}

atomic_copy() {
    src="$1"
    dst="$2"
    exec_flag="$3"
    mkdir -p "$(dirname "$dst")" || return 1
    cp -p "$src" "${dst}.tmp" || { rm -f "${dst}.tmp"; return 1; }
    if [ "$exec_flag" = "1" ]; then
        chmod +x "${dst}.tmp" || { rm -f "${dst}.tmp"; return 1; }
    fi
    mv -f "${dst}.tmp" "$dst" || { rm -f "${dst}.tmp"; return 1; }
    return 0
}

sha_cmd=""
file_sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print tolower($1)}'
    else
        shasum -a 256 "$1" | awk '{print tolower($1)}'
    fi
}

if command -v sha256sum >/dev/null 2>&1; then
    sha_cmd="sha256sum"
elif command -v shasum >/dev/null 2>&1; then
    sha_cmd="shasum -a 256"
else
    echo "ERROR: sha256 verification tool unavailable" >&2
    exit 1
fi

# Read-only preflight before stopping services or removing working Keyglass packages.
if [ ! -d "$RESTORE_DIR" ] || [ ! -s "$RESTORE_MANIFEST" ]; then
    echo "ERROR: restore directory or manifest.sha256 missing in ${RESTORE_DIR}" >&2
    exit 1
fi

if ! (cd "$RESTORE_DIR" && $sha_cmd -c "$RESTORE_MANIFEST" >/dev/null 2>&1); then
    echo "ERROR: restore package checksum verification failed; aborting" >&2
    exit 1
fi

verify_manifest_binding() {
    ipk_path="$1"
    base=$(basename "$ipk_path")
    actual_hash=$(file_sha256 "$ipk_path")
    manifest_hash=$(awk -v b="$base" '
        {
            f = $2
            sub(/^\*\.\//, "", f)
            sub(/^\.\//, "", f)
            sub(/^\*/, "", f)
            if (f == b) { print tolower($1); exit }
        }
    ' "$RESTORE_MANIFEST")
    [ -n "$manifest_hash" ] || return 1
    [ "$actual_hash" = "$manifest_hash" ] || return 1
    return 0
}

verify_ipk_archive() {
    ipk_path="$1"
    exp_pkg="$2"
    members=$(ar t "$ipk_path" 2>/dev/null) || return 1
    [ -n "$members" ] || return 1

    echo "$members" | grep -qx "control.tar.gz" || return 1
    ctrl=$(ar p "$ipk_path" control.tar.gz 2>/dev/null | tar -xzO ./control 2>/dev/null) \
        || ctrl=$(ar p "$ipk_path" control.tar.gz 2>/dev/null | tar -xzO control 2>/dev/null) \
        || return 1
    [ -n "$ctrl" ] || return 1
    echo "$ctrl" | grep -qx "Package: ${exp_pkg}" || return 1
    echo "$ctrl" | grep -qx "Architecture: aarch64" || return 1

    data_member=$(echo "$members" | grep -E '^data\.tar(\.(gz|xz|bz2))?$' | head -n 1 || true)
    [ -n "$data_member" ] || return 1
    case "$data_member" in
        data.tar.gz)
            list=$(ar p "$ipk_path" "$data_member" 2>/dev/null | tar -tzf - 2>/dev/null) || return 1
            ;;
        data.tar.xz)
            list=$(ar p "$ipk_path" "$data_member" 2>/dev/null | tar -tJf - 2>/dev/null) || return 1
            ;;
        data.tar.bz2)
            list=$(ar p "$ipk_path" "$data_member" 2>/dev/null | tar -tjf - 2>/dev/null) || return 1
            ;;
        data.tar)
            list=$(ar p "$ipk_path" "$data_member" 2>/dev/null | tar -tf - 2>/dev/null) || return 1
            ;;
        *)
            return 1
            ;;
    esac
    [ -n "$list" ] || return 1
    if [ "$exp_pkg" = "garfield" ]; then
        echo "$list" | grep -qE '^(\./)?usr/lib/garfield/scripts/flux-weston-autolaunch\.sh$' || return 1
    fi
    return 0
}

extract_factory_launcher_from_ipk() {
    ipk_path="$1"
    dst="$2"
    members=$(ar t "$ipk_path" 2>/dev/null || true)
    data_member=$(echo "$members" | grep -E '^data\.tar(\.(gz|xz|bz2))?$' | head -n 1 || true)
    [ -n "$data_member" ] || return 1
    case "$data_member" in
        data.tar.gz) tflag="-xzO" ;;
        data.tar.xz) tflag="-xJO" ;;
        data.tar.bz2) tflag="-xjO" ;;
        data.tar) tflag="-xO" ;;
        *) return 1 ;;
    esac
    mkdir -p "$(dirname "$dst")" || return 1
    (
        ar p "$ipk_path" "$data_member" 2>/dev/null | tar "$tflag" -f - "./usr/lib/garfield/scripts/flux-weston-autolaunch.sh" 2>/dev/null \
        || ar p "$ipk_path" "$data_member" 2>/dev/null | tar "$tflag" -f - "usr/lib/garfield/scripts/flux-weston-autolaunch.sh" 2>/dev/null
    ) > "${dst}.tmp" || { rm -f "${dst}.tmp"; return 1; }
    [ -s "${dst}.tmp" ] || { rm -f "${dst}.tmp"; return 1; }
    chmod +x "${dst}.tmp" || { rm -f "${dst}.tmp"; return 1; }
    mv -f "${dst}.tmp" "$dst" || { rm -f "${dst}.tmp"; return 1; }
    return 0
}

BRIDGE_IPK=""
DISPLAY_IPK=""
BRIDGE_COUNT=0
DISPLAY_COUNT=0

for ipk in "$RESTORE_DIR"/garfield_*.ipk; do
    [ -e "$ipk" ] || continue
    [ -s "$ipk" ] || { echo "ERROR: empty bridge factory IPK: $ipk" >&2; exit 1; }
    BRIDGE_COUNT=$((BRIDGE_COUNT + 1))
    BRIDGE_IPK="$ipk"
done

for ipk in "$RESTORE_DIR"/garfunkel_*.ipk; do
    [ -e "$ipk" ] || continue
    [ -s "$ipk" ] || { echo "ERROR: empty display factory IPK: $ipk" >&2; exit 1; }
    DISPLAY_COUNT=$((DISPLAY_COUNT + 1))
    DISPLAY_IPK="$ipk"
done

if [ "$BRIDGE_COUNT" -ne 1 ] || [ "$DISPLAY_COUNT" -ne 1 ]; then
    echo "ERROR: restore directory must contain exactly one garfield_*.ipk and one garfunkel_*.ipk" >&2
    exit 1
fi

verify_manifest_binding "$BRIDGE_IPK" || { echo "ERROR: bridge factory IPK missing or mismatched in manifest.sha256: $BRIDGE_IPK" >&2; exit 1; }
verify_manifest_binding "$DISPLAY_IPK" || { echo "ERROR: display factory IPK missing or mismatched in manifest.sha256: $DISPLAY_IPK" >&2; exit 1; }
verify_ipk_archive "$BRIDGE_IPK" "garfield" || { echo "ERROR: invalid or corrupt bridge factory IPK: $BRIDGE_IPK" >&2; exit 1; }
verify_ipk_archive "$DISPLAY_IPK" "garfunkel" || { echo "ERROR: invalid or corrupt display factory IPK: $DISPLAY_IPK" >&2; exit 1; }

if [ "$PREFLIGHT_ONLY" -eq 1 ]; then
    exit 0
fi

stop_factory_services() {
    mkdir -p "${ROOT}/tmp"
    touch "${ROOT}/tmp/updating_garfield" "${ROOT}/tmp/updating_garfunkel"
    for pat in \
        "$OS_BOOT_LAUNCHER" \
        "${ROOT}/usr/lib/garfield/garfield" \
        "${ROOT}/usr/bin/garfield" \
        "${ROOT}/usr/lib/garfunkel/garfunkel" \
        "${ROOT}/usr/bin/garfunkel"; do
        pkill -TERM -f "$pat" 2>/dev/null || true
    done
    sleep "${KEYGLASS_STOP_SLEEP:-2}"
    for pat in \
        "$OS_BOOT_LAUNCHER" \
        "${ROOT}/usr/lib/garfield/garfield" \
        "${ROOT}/usr/bin/garfield" \
        "${ROOT}/usr/lib/garfunkel/garfunkel" \
        "${ROOT}/usr/bin/garfunkel"; do
        pkill -KILL -f "$pat" 2>/dev/null || true
    done
    if [ -z "$ROOT" ]; then
        if [ -d "$GADGET_PATH" ]; then
            printf "\n" > "${GADGET_PATH}/UDC" 2>/dev/null || true
            rm -f "${GADGET_PATH}/configs/c.1/ffs.garfield" "${GADGET_PATH}/os_desc/c.1" 2>/dev/null || true
            rmdir "${GADGET_PATH}/configs/c.1/strings/0x409" "${GADGET_PATH}/configs/c.1" \
                  "${GADGET_PATH}/functions/ffs.garfield" "${GADGET_PATH}/strings/0x409" \
                  "$GADGET_PATH" 2>/dev/null || true
        fi
        umount -l "/dev/ffs-garfield" 2>/dev/null || true
    fi
}

install_restore_boot_rescue_hooks() {
    mkdir -p "${ROOT}/usr/bin" "${ROOT}/etc/init.d"

    cat > "${RECOVERY_INIT}.tmp" <<'EOF'
#!/bin/sh
mode="${1:-start}"
case "$mode" in
    start|start-and-exec) ;;
    *) exit 0 ;;
esac

ROOT="${KEYGLASS_SYSROOT:-}"
RESCUE_DIR="${ROOT}/home/root/.keyglass/rescue"
RESCUE_ARMED="${RESCUE_DIR}/armed"
OS_BOOT_LAUNCHER="${ROOT}/usr/bin/flux-weston-autolaunch.sh"
LOG_DIR="${ROOT}/home/root/keyglass/logs"

if [ ! -f "$RESCUE_ARMED" ] || [ ! -d "$RESCUE_DIR" ]; then
    rm -f "${ROOT}/etc/init.d/S05keyglass-recovery"
    if [ "$mode" = "start-and-exec" ] && [ -x "${ROOT}/usr/lib/keyglass/bridge/keyglass-autolaunch.sh" ]; then
        exec "${ROOT}/usr/lib/keyglass/bridge/keyglass-autolaunch.sh"
    fi
    exit 0
fi

flush_fs() {
    [ "${KEYGLASS_SKIP_SYNC:-0}" = "1" ] && return 0
    sync
}

atomic_copy() {
    src="$1"
    dst="$2"
    exec_flag="$3"
    mkdir -p "$(dirname "$dst")" || return 1
    cp -p "$src" "${dst}.tmp" || { rm -f "${dst}.tmp"; return 1; }
    if [ "$exec_flag" = "1" ]; then
        chmod +x "${dst}.tmp" || { rm -f "${dst}.tmp"; return 1; }
    fi
    mv -f "${dst}.tmp" "$dst" || { rm -f "${dst}.tmp"; return 1; }
    return 0
}

mkdir -p "$LOG_DIR" 2>/dev/null || true
echo "$(date '+%Y-%m-%d %H:%M:%S') - S05keyglass-recovery triggered; reinstating Keyglass" >> "${LOG_DIR}/autolaunch.log"

for pat in \
    "$OS_BOOT_LAUNCHER" \
    "${ROOT}/usr/lib/garfield/garfield" \
    "${ROOT}/usr/bin/garfield" \
    "${ROOT}/usr/lib/garfunkel/garfunkel" \
    "${ROOT}/usr/bin/garfunkel"; do
    pkill -TERM -f "$pat" 2>/dev/null || true
done
sleep "${KEYGLASS_STOP_SLEEP:-1}"
for pat in \
    "$OS_BOOT_LAUNCHER" \
    "${ROOT}/usr/lib/garfield/garfield" \
    "${ROOT}/usr/bin/garfield" \
    "${ROOT}/usr/lib/garfunkel/garfunkel" \
    "${ROOT}/usr/bin/garfunkel"; do
    pkill -KILL -f "$pat" 2>/dev/null || true
done

ok=1
if [ -f "${RESCUE_DIR}/keyglass-bridge" ]; then
    atomic_copy "${RESCUE_DIR}/keyglass-bridge" "${ROOT}/usr/lib/keyglass/bridge/keyglass-bridge" 1 || ok=0
fi
if [ "$ok" -eq 1 ] && [ -f "${RESCUE_DIR}/keyglass-display" ]; then
    atomic_copy "${RESCUE_DIR}/keyglass-display" "${ROOT}/usr/lib/keyglass/display/keyglass-display" 1 || ok=0
fi
if [ "$ok" -eq 1 ] && [ -f "${RESCUE_DIR}/bridge.version" ]; then
    atomic_copy "${RESCUE_DIR}/bridge.version" "${ROOT}/home/root/.keyglass/versions/bridge" 0 || ok=0
fi
if [ "$ok" -eq 1 ] && [ -f "${RESCUE_DIR}/display.version" ]; then
    atomic_copy "${RESCUE_DIR}/display.version" "${ROOT}/home/root/.keyglass/versions/display" 0 || ok=0
fi
if [ "$ok" -eq 1 ] && [ -f "${RESCUE_DIR}/keyglass-autolaunch.sh" ]; then
    atomic_copy "${RESCUE_DIR}/keyglass-autolaunch.sh" "${ROOT}/usr/lib/keyglass/bridge/keyglass-autolaunch.sh" 1 || ok=0
fi
if [ "$ok" -eq 1 ] && [ -f "${RESCUE_DIR}/restore-factory.sh" ]; then
    atomic_copy "${RESCUE_DIR}/restore-factory.sh" "${ROOT}/usr/lib/keyglass/bridge/restore-factory.sh" 1 || ok=0
fi
if [ "$ok" -ne 1 ] || ! flush_fs; then
    echo "$(date '+%Y-%m-%d %H:%M:%S') - ERROR: S05keyglass-recovery payload copy/flush failed; preserving armed marker" >> "${LOG_DIR}/autolaunch.log"
    exit 1
fi

if [ -f "${RESCUE_DIR}/keyglass-autolaunch.sh" ]; then
    atomic_copy "${RESCUE_DIR}/keyglass-autolaunch.sh" "$OS_BOOT_LAUNCHER" 1 && flush_fs || ok=0
elif [ -f "${RESCUE_DIR}/os-autolaunch.sh" ]; then
    atomic_copy "${RESCUE_DIR}/os-autolaunch.sh" "$OS_BOOT_LAUNCHER" 1 && flush_fs || ok=0
fi
if [ "$ok" -ne 1 ]; then
    echo "$(date '+%Y-%m-%d %H:%M:%S') - ERROR: S05keyglass-recovery launcher copy/flush failed; preserving armed marker" >> "${LOG_DIR}/autolaunch.log"
    exit 1
fi

rm -f "$RESCUE_ARMED" "${ROOT}/etc/init.d/S05keyglass-recovery" \
      "${ROOT}/tmp/keyglass_updating_bridge" "${ROOT}/tmp/keyglass_updating_display" \
      "${ROOT}/tmp/updating_garfield" "${ROOT}/tmp/updating_garfunkel" \
      "${ROOT}/tmp/keyglass_bridge_ready" "${ROOT}/tmp/keyglass_display_active"
flush_fs || true
if [ "$mode" = "start-and-exec" ] && [ -x "$OS_BOOT_LAUNCHER" ]; then
    exec "$OS_BOOT_LAUNCHER"
fi
exit 0
EOF
    chmod +x "${RECOVERY_INIT}.tmp"
    mv -f "${RECOVERY_INIT}.tmp" "$RECOVERY_INIT"

    cat > "${OS_BOOT_LAUNCHER}.tmp" <<'EOF'
#!/bin/sh
ROOT="${KEYGLASS_SYSROOT:-}"
RECOVERY_INIT="${ROOT}/etc/init.d/S05keyglass-recovery"
RESCUE_ARMED="${ROOT}/home/root/.keyglass/rescue/armed"

if [ -f "$RESCUE_ARMED" ] && [ -x "$RECOVERY_INIT" ]; then
    exec "$RECOVERY_INIT" start-and-exec
fi

if [ -x "${ROOT}/usr/lib/keyglass/bridge/keyglass-autolaunch.sh" ]; then
    exec "${ROOT}/usr/lib/keyglass/bridge/keyglass-autolaunch.sh"
fi
EOF
    chmod +x "${OS_BOOT_LAUNCHER}.tmp"
    mv -f "${OS_BOOT_LAUNCHER}.tmp" "$OS_BOOT_LAUNCHER"
}

stage_keyglass_rescue() {
    [ -s "${ROOT}/usr/lib/keyglass/bridge/keyglass-bridge" ] || {
        echo "ERROR: missing installed keyglass-bridge binary; aborting rescue stage" >&2
        exit 1
    }
    [ -s "${ROOT}/usr/lib/keyglass/bridge/keyglass-autolaunch.sh" ] || {
        echo "ERROR: missing installed keyglass-autolaunch.sh; aborting rescue stage" >&2
        exit 1
    }
    rm -rf "$RESCUE_DIR"
    mkdir -p "$RESCUE_DIR"
    cp -p "${ROOT}/usr/lib/keyglass/bridge/keyglass-bridge" "${RESCUE_DIR}/keyglass-bridge"
    cp -p "${ROOT}/usr/lib/keyglass/bridge/keyglass-autolaunch.sh" "${RESCUE_DIR}/keyglass-autolaunch.sh"
    [ -f "${ROOT}/usr/lib/keyglass/bridge/restore-factory.sh" ] && cp -p "${ROOT}/usr/lib/keyglass/bridge/restore-factory.sh" "${RESCUE_DIR}/restore-factory.sh"
    [ -f "${ROOT}/usr/lib/keyglass/display/keyglass-display" ] && cp -p "${ROOT}/usr/lib/keyglass/display/keyglass-display" "${RESCUE_DIR}/keyglass-display"
    [ -f "${ROOT}/home/root/.keyglass/versions/bridge" ] && cp -p "${ROOT}/home/root/.keyglass/versions/bridge" "${RESCUE_DIR}/bridge.version"
    [ -f "${ROOT}/home/root/.keyglass/versions/display" ] && cp -p "${ROOT}/home/root/.keyglass/versions/display" "${RESCUE_DIR}/display.version"
    [ -f "$OS_BOOT_LAUNCHER" ] && cp -p "$OS_BOOT_LAUNCHER" "${RESCUE_DIR}/os-autolaunch.sh"
    # Flush complete rescue snapshot before writing armed marker or boot hooks.
    flush_fs
    echo "pending-factory-restore" > "$RESCUE_ARMED"
    install_restore_boot_rescue_hooks
    flush_fs
}

reinstate_keyglass_rescue() {
    stop_factory_services
    ok=1
    mkdir -p "${ROOT}/usr/lib/keyglass/bridge" "${ROOT}/usr/lib/keyglass/display" "${ROOT}/usr/bin" "${ROOT}/home/root/.keyglass/versions" "${ROOT}/home/root/keyglass/logs" || ok=0
    if [ "$ok" -eq 1 ] && [ -f "${RESCUE_DIR}/keyglass-bridge" ]; then
        atomic_copy "${RESCUE_DIR}/keyglass-bridge" "${ROOT}/usr/lib/keyglass/bridge/keyglass-bridge" 1 || ok=0
    fi
    if [ "$ok" -eq 1 ] && [ -f "${RESCUE_DIR}/keyglass-display" ]; then
        atomic_copy "${RESCUE_DIR}/keyglass-display" "${ROOT}/usr/lib/keyglass/display/keyglass-display" 1 || ok=0
    fi
    if [ "$ok" -eq 1 ] && [ -f "${RESCUE_DIR}/bridge.version" ]; then
        atomic_copy "${RESCUE_DIR}/bridge.version" "${ROOT}/home/root/.keyglass/versions/bridge" 0 || ok=0
    fi
    if [ "$ok" -eq 1 ] && [ -f "${RESCUE_DIR}/display.version" ]; then
        atomic_copy "${RESCUE_DIR}/display.version" "${ROOT}/home/root/.keyglass/versions/display" 0 || ok=0
    fi
    if [ "$ok" -eq 1 ] && [ -f "${RESCUE_DIR}/keyglass-autolaunch.sh" ]; then
        atomic_copy "${RESCUE_DIR}/keyglass-autolaunch.sh" "${ROOT}/usr/lib/keyglass/bridge/keyglass-autolaunch.sh" 1 || ok=0
    fi
    if [ "$ok" -eq 1 ] && [ -f "${RESCUE_DIR}/restore-factory.sh" ]; then
        atomic_copy "${RESCUE_DIR}/restore-factory.sh" "${ROOT}/usr/lib/keyglass/bridge/restore-factory.sh" 1 || ok=0
    fi
    if [ "$ok" -ne 1 ] || ! flush_fs; then
        echo "ERROR: rescue payload copy/flush failed; keeping rescue armed marker" >&2
        return 1
    fi
    if [ -f "${RESCUE_DIR}/keyglass-autolaunch.sh" ]; then
        atomic_copy "${RESCUE_DIR}/keyglass-autolaunch.sh" "$OS_BOOT_LAUNCHER" 1 && flush_fs || ok=0
    elif [ -f "${RESCUE_DIR}/os-autolaunch.sh" ]; then
        atomic_copy "${RESCUE_DIR}/os-autolaunch.sh" "$OS_BOOT_LAUNCHER" 1 && flush_fs || ok=0
    fi
    if [ "$ok" -ne 1 ]; then
        echo "ERROR: rescue launcher copy/flush failed; keeping rescue armed marker" >&2
        return 1
    fi
    rm -f "$RESCUE_ARMED" "$RECOVERY_INIT" \
          "${ROOT}/tmp/keyglass_updating_bridge" "${ROOT}/tmp/keyglass_updating_display" \
          "${ROOT}/tmp/updating_garfield" "${ROOT}/tmp/updating_garfunkel" \
          "${ROOT}/tmp/keyglass_bridge_ready" "${ROOT}/tmp/keyglass_display_active"
    flush_fs || true
    if [ -x "$OS_BOOT_LAUNCHER" ]; then
        nohup "$OS_BOOT_LAUNCHER" </dev/null > "${ROOT}/home/root/keyglass/logs/autolaunch.log" 2>&1 &
    fi
    return 0
}

stage_keyglass_rescue

mkdir -p "${ROOT}/tmp"
touch "${ROOT}/tmp/keyglass_updating_bridge" "${ROOT}/tmp/keyglass_updating_display"
pkill -TERM -f "${ROOT}/usr/lib/keyglass/display/keyglass-display" 2>/dev/null || true
pkill -TERM -f "${ROOT}/usr/lib/keyglass/bridge/keyglass-bridge" 2>/dev/null || true
pkill -TERM -f "$OS_BOOT_LAUNCHER" 2>/dev/null || true
sleep "${KEYGLASS_STOP_SLEEP:-2}"
pkill -KILL -f "${ROOT}/usr/lib/keyglass/display/keyglass-display" 2>/dev/null || true
pkill -KILL -f "${ROOT}/usr/lib/keyglass/bridge/keyglass-bridge" 2>/dev/null || true

# Tear down Keyglass FunctionFS gadget cleanly before factory restore (only on real device root).
if [ -z "$ROOT" ]; then
    if [ -d "$GADGET_PATH" ]; then
        printf "\n" > "${GADGET_PATH}/UDC" 2>/dev/null || true
        rm -f "${GADGET_PATH}/configs/c.1/ffs.keyglass" "${GADGET_PATH}/os_desc/c.1" 2>/dev/null || true
        rmdir "${GADGET_PATH}/configs/c.1/strings/0x409" "${GADGET_PATH}/configs/c.1" \
              "${GADGET_PATH}/functions/ffs.keyglass" "${GADGET_PATH}/strings/0x409" \
              "$GADGET_PATH" 2>/dev/null || true
    fi
    umount -l "/dev/ffs-keyglass" 2>/dev/null || true
fi

opkg remove keyglass-display keyglass-bridge 2>/dev/null || true

if ! opkg install --force-downgrade --force-reinstall "$BRIDGE_IPK" "$DISPLAY_IPK"; then
    echo "ERROR: factory package install failed; reinstating rescued Keyglass bridge" >&2
    reinstate_keyglass_rescue || true
    exit 1
fi

# Always install & flush the verified factory boot launcher from the checksum-verified bridge IPK before disarming rescue.
if ! flush_fs || ! extract_factory_launcher_from_ipk "$BRIDGE_IPK" "$OS_BOOT_LAUNCHER" || ! flush_fs; then
    echo "ERROR: failed to install/flush factory boot launcher from bridge IPK; reinstating rescued Keyglass bridge" >&2
    reinstate_keyglass_rescue || true
    exit 1
fi
# The Keyglass launcher keeps the OS time service from starting at boot; put its init script back.
if [ -f "${ROOT}/etc/init.d/disabled_S49chrony" ] && [ ! -e "${ROOT}/etc/init.d/S49chrony" ]; then
    mv "${ROOT}/etc/init.d/disabled_S49chrony" "${ROOT}/etc/init.d/S49chrony" || true
fi
rm -f "$RESCUE_ARMED" "$RECOVERY_INIT"
rm -rf "$RESCUE_DIR"
flush_fs || true

rm -f "${ROOT}/tmp/keyglass_updating_bridge" "${ROOT}/tmp/keyglass_updating_display" \
      "${ROOT}/tmp/keyglass_bridge_ready" "${ROOT}/tmp/keyglass_display_active"

if [ -x "$OS_BOOT_LAUNCHER" ]; then
    mkdir -p "${ROOT}/home/root/flux/logs"
    nohup "$OS_BOOT_LAUNCHER" </dev/null > "${ROOT}/home/root/flux/logs/autolaunch.log" 2>&1 &
fi

exit 0
