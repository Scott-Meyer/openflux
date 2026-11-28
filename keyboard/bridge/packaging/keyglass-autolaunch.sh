#!/bin/sh
# Keyglass keyboard boot launcher (installed into the base OS autolaunch hook).

export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run}"
export WAYLAND_DISPLAY="${WAYLAND_DISPLAY:-wayland-0}"
export HOME="${HOME:-/root}"
export SHELL="${SHELL:-/bin/sh}"

ROOT="${KEYGLASS_SYSROOT:-}"

# The base OS's package installer, which installs .ipk files dropped into its folder; the launcher
# keeps it running.
OS_IPK_MONITOR="${ROOT}/usr/bin/flux-ipk-monitor.sh"
OS_BOOT_LAUNCHER="${ROOT}/usr/bin/flux-weston-autolaunch.sh"

BRIDGE_BIN="${ROOT}/usr/lib/keyglass/bridge/keyglass-bridge"
BRIDGE_LAUNCHER="${ROOT}/usr/lib/keyglass/bridge/keyglass-autolaunch.sh"
BRIDGE_FFS="${ROOT}/dev/ffs-keyglass"
DISPLAY_DIR="${ROOT}/usr/lib/keyglass/display"
DISPLAY_BIN="${DISPLAY_DIR}/keyglass-display"
ROLLBACK_DIR="${ROOT}/home/root/.keyglass/rollback/bridge"
ARMED_FILE="${ROLLBACK_DIR}/armed"
BRIDGE_VERSION_FILE="${ROOT}/home/root/.keyglass/versions/bridge"
LOG_DIR="${ROOT}/home/root/keyglass/logs"
AUTOLAUNCH_LOG="${LOG_DIR}/autolaunch.log"
HEARTBEAT_FILE="${LOG_DIR}/display_heartbeat.log"
BRIDGE_SEM="${ROOT}/tmp/keyglass_updating_bridge"
DISPLAY_SEM="${ROOT}/tmp/keyglass_updating_display"

mkdir -p "$LOG_DIR"

flush_fs() {
    [ "${KEYGLASS_SKIP_SYNC:-0}" = "1" ] && return 0
    sync
}

atomic_restore_file() {
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

# Recover from an interrupted or power-lost bridge upgrade if host USB confirmation never arrived.
# postinst sets KEYGLASS_TRIAL_LAUNCH=1 during its intentional candidate trial launch.
if [ "${KEYGLASS_TRIAL_LAUNCH:-0}" != "1" ] && [ -f "$ARMED_FILE" ]; then
    rollback_ok=0
    if [ -f "${ROLLBACK_DIR}/keyglass-bridge" ]; then
        echo "$(date '+%Y-%m-%d %H:%M:%S') - Armed bridge rollback detected at boot; restoring previous bridge" >> "$AUTOLAUNCH_LOG"
        rollback_ok=1
        atomic_restore_file "${ROLLBACK_DIR}/keyglass-bridge" "$BRIDGE_BIN" 1 || rollback_ok=0
        if [ "$rollback_ok" -eq 1 ] && [ -f "${ROLLBACK_DIR}/bridge.version" ]; then
            atomic_restore_file "${ROLLBACK_DIR}/bridge.version" "$BRIDGE_VERSION_FILE" 0 || rollback_ok=0
        fi
        if [ "$rollback_ok" -eq 1 ] && [ -f "${ROLLBACK_DIR}/keyglass-autolaunch.sh" ]; then
            atomic_restore_file "${ROLLBACK_DIR}/keyglass-autolaunch.sh" "$BRIDGE_LAUNCHER" 1 || rollback_ok=0
        fi
        if [ "$rollback_ok" -ne 1 ] || ! flush_fs; then
            echo "$(date '+%Y-%m-%d %H:%M:%S') - ERROR: armed boot rollback payload copy/flush failed; keeping armed sentinel" >> "$AUTOLAUNCH_LOG"
            exit 1
        fi
        if [ -f "${ROLLBACK_DIR}/keyglass-autolaunch.sh" ]; then
            if ! atomic_restore_file "$BRIDGE_LAUNCHER" "$OS_BOOT_LAUNCHER" 1 || ! flush_fs; then
                echo "$(date '+%Y-%m-%d %H:%M:%S') - ERROR: armed boot launcher copy/flush failed; keeping armed sentinel" >> "$AUTOLAUNCH_LOG"
                exit 1
            fi
        fi
    fi

    if [ "$rollback_ok" -eq 1 ]; then
        rm -f "$ARMED_FILE"
        flush_fs || true
        if [ -x "$OS_BOOT_LAUNCHER" ]; then
            exec "$OS_BOOT_LAUNCHER"
        fi
    else
        # Nothing to roll back to and nothing has been changed: start the installed bridge so the
        # host can still reach the keyboard. The armed sentinel stays for the host to see.
        echo "$(date '+%Y-%m-%d %H:%M:%S') - ERROR: armed boot rollback snapshot missing; keeping armed sentinel and starting the installed bridge" >> "$AUTOLAUNCH_LOG"
    fi
fi

# Some RK356x units (pvtm-volt-sel=1) can freeze at higher GPU clocks, so the GPU is capped at
# 800 MHz for stability.
cap_gpu_frequency() {
    [ -z "$ROOT" ] || return 0
    gpu_devfreq="/sys/class/devfreq/fde60000.gpu"
    target_freq="800000000"
    if [ -f "${gpu_devfreq}/max_freq" ]; then
        current_max=$(cat "${gpu_devfreq}/max_freq" 2>/dev/null || echo "")
        if [ "$current_max" != "$target_freq" ]; then
            echo "$target_freq" > "${gpu_devfreq}/max_freq" 2>/dev/null || true
        fi
    fi
}

# The host sets the keyboard's clock over USB (SET_SYSTEM_TIME), so the OS's own time service is
# stopped and kept from starting at boot. A factory restore puts its init script back.
disable_ntp() {
    [ -z "$ROOT" ] || return 0
    if pidof chronyd >/dev/null 2>&1; then
        /etc/init.d/S49chrony stop 2>/dev/null || killall chronyd 2>/dev/null || true
    fi
    if [ -f /etc/init.d/S49chrony ]; then
        mv /etc/init.d/S49chrony /etc/init.d/disabled_S49chrony 2>/dev/null || true
    fi
}

cap_gpu_frequency
disable_ntp

if [ -f "$AUTOLAUNCH_LOG" ]; then
    tail -n 500 "$AUTOLAUNCH_LOG" > "${AUTOLAUNCH_LOG}.tmp" 2>/dev/null && mv "${AUTOLAUNCH_LOG}.tmp" "$AUTOLAUNCH_LOG"
fi

rm -f "$BRIDGE_SEM" "$DISPLAY_SEM"

# Keeps the keyboard OS's package installer running. A delayed update from the OS stages a
# self-deleting update-manifest script in /tmp/ipk-updates; it runs when present, before the
# OS package monitor is (re)started, so checking for it on every pass is intended.
start_ipk_monitor_watchdog() {
    while true; do
        if [ -f "${ROOT}/tmp/ipk-updates/update-manifest" ]; then
            "${ROOT}/tmp/ipk-updates/update-manifest" >> "${LOG_DIR}/ipk_monitor.log" 2>&1 || true
        fi
        if pgrep -f "$OS_IPK_MONITOR" >/dev/null 2>&1; then
            sleep 10
            continue
        fi
        "$OS_IPK_MONITOR" >/dev/null 2>&1 || true
        sleep 5
    done
}

start_bridge_watchdog() {
    last_restart_time=0
    restart_cooldown=10
    while true; do
        if [ -f "$BRIDGE_SEM" ]; then
            sleep 5
            continue
        fi
        if pgrep -f "$BRIDGE_BIN" >/dev/null 2>&1; then
            sleep 10
            continue
        fi
        now=$(date +%s)
        since=$((now - last_restart_time))
        if [ "$since" -lt 0 ] || [ "$since" -gt 3600 ]; then
            last_restart_time=0
            since="$restart_cooldown"
        fi
        if [ "$since" -lt "$restart_cooldown" ] && [ "$last_restart_time" -gt 0 ]; then
            sleep $((restart_cooldown - since))
        fi
        echo "$(date '+%Y-%m-%d %H:%M:%S') - Starting keyglass-bridge" >> "$AUTOLAUNCH_LOG"
        last_restart_time=$(date +%s)
        "$BRIDGE_BIN" "$BRIDGE_FFS" > "${LOG_DIR}/bridge.log" 2>&1 || true
        sleep 5
    done
}

start_display_watchdog() {
    last_restart_time=0
    restart_cooldown=10
    warmup_count=0
    monitoring=0
    last_hb=""
    frozen_count=0

    while true; do
        if [ -f "$DISPLAY_SEM" ]; then
            warmup_count=0
            monitoring=0
            last_hb=""
            frozen_count=0
            sleep 5
            continue
        fi
        if pgrep -f "$DISPLAY_BIN" >/dev/null 2>&1; then
            if [ -f "$HEARTBEAT_FILE" ]; then
                hb=$(cat "$HEARTBEAT_FILE" 2>/dev/null | tr -d '\n')
                case "$hb" in
                    ""|*[!0-9]*) ;;
                    *)
                        if [ "$monitoring" -eq 0 ]; then
                            if [ "$hb" != "$last_hb" ]; then
                                last_hb="$hb"
                                warmup_count=$((warmup_count + 1))
                                if [ "$warmup_count" -ge 5 ]; then
                                    monitoring=1
                                fi
                            fi
                        else
                            if [ "$hb" != "$last_hb" ]; then
                                diff=$((hb - last_hb))
                                if [ "$diff" -lt -10 ] || [ "$diff" -gt 60 ]; then
                                    warmup_count=0
                                    monitoring=0
                                    last_hb=""
                                    frozen_count=0
                                else
                                    last_hb="$hb"
                                    frozen_count=0
                                fi
                            else
                                frozen_count=$((frozen_count + 1))
                                if [ "$frozen_count" -ge 6 ]; then
                                    echo "$(date '+%Y-%m-%d %H:%M:%S') - Display heartbeat stalled, restarting" >> "$AUTOLAUNCH_LOG"
                                    pkill -KILL -f "$DISPLAY_BIN" 2>/dev/null || true
                                    warmup_count=0
                                    monitoring=0
                                    last_hb=""
                                    frozen_count=0
                                    sleep 2
                                    continue
                                fi
                            fi
                        fi
                        ;;
                esac
            fi
            sleep 0.5
            continue
        fi

        now=$(date +%s)
        since=$((now - last_restart_time))
        if [ "$since" -lt 0 ] || [ "$since" -gt 3600 ]; then
            last_restart_time=0
            since="$restart_cooldown"
        fi
        if [ "$since" -lt "$restart_cooldown" ] && [ "$last_restart_time" -gt 0 ]; then
            sleep $((restart_cooldown - since))
        fi

        warmup_count=0
        monitoring=0
        last_hb=""
        frozen_count=0
        if [ -x "$DISPLAY_BIN" ]; then
            echo "$(date '+%Y-%m-%d %H:%M:%S') - Starting keyglass-display" >> "$AUTOLAUNCH_LOG"
            cd "$DISPLAY_DIR" || exit 1
            last_restart_time=$(date +%s)
            "$DISPLAY_BIN" --width=1920 --height=1080 > "${LOG_DIR}/display.log" 2>&1 &
        fi
        sleep 1
    done
}

if [ -x "$OS_IPK_MONITOR" ]; then
    start_ipk_monitor_watchdog &
fi

start_bridge_watchdog &
start_display_watchdog
