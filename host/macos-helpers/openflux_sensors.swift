// openflux-sensors: one-shot macOS sensor sampler for openfluxd.
// Prints a single JSON object on stdout (frontmost app, input source, now-playing media,
// hardware stats, display sleep) and exits. The daemon runs it on each sampling tick.
// The media object names the track's artwork by `artworkId`; `--artwork` prints only the media
// object, with the artwork itself as base64 in `artwork`, for the daemon to fetch when the id
// changes.
// `--test-late-callback` checks that a media callback arriving after the timeout is
// handled safely: it prints the resulting snapshot, and exits with status 1 if the snapshot
// is wrong (`make check` runs it).

import AppKit
import Carbon
import CoreGraphics
import Darwin
import Foundation
import IOKit
import IOKit.ps

typealias MRGetNowPlayingInfoFn = @convention(c) (DispatchQueue, @escaping ([String: Any]?) -> Void) -> Void
typealias MRGetIsPlayingFn = @convention(c) (DispatchQueue, @escaping (Bool) -> Void) -> Void

final class MediaBox: @unchecked Sendable {
    private let lock = NSLock()
    private var playing: Bool = false
    private var title: String = ""
    private var artist: String = ""
    private var album: String = ""
    private var duration: Double = 0.0
    private var elapsed: Double = 0.0
    private var artwork: Data? = nil
    private var gotInfo: Bool = false
    private var gotPlaying: Bool = false

    func setInfo(_ info: [String: Any]?) {
        lock.lock()
        defer { lock.unlock() }
        gotInfo = true
        if let info = info {
            if let t = info["kMRMediaRemoteNowPlayingInfoTitle"] as? String { title = t }
            if let a = info["kMRMediaRemoteNowPlayingInfoArtist"] as? String { artist = a }
            if let al = info["kMRMediaRemoteNowPlayingInfoAlbum"] as? String { album = al }
            if let d = info["kMRMediaRemoteNowPlayingInfoDuration"] as? Double { duration = d }
            if let e = info["kMRMediaRemoteNowPlayingInfoElapsedTime"] as? Double { elapsed = e }
            if let art = info["kMRMediaRemoteNowPlayingInfoArtworkData"] as? Data, !art.isEmpty {
                artwork = art
            }
        }
    }

    func setPlaying(_ isPlaying: Bool) {
        lock.lock()
        defer { lock.unlock() }
        gotPlaying = true
        playing = isPlaying
    }

    /// `status` is `ok` when MediaRemote named a track, `none` when it answered without one,
    /// `timeout` when it didn't answer in time and `unavailable` when it can't be loaded. Only
    /// `ok` carries track fields; the others report nothing playing. `artworkId` is empty when
    /// the track has no artwork; with `includeArtwork` the artwork follows as base64.
    func snapshot(available: Bool, includeArtwork: Bool = false) -> [String: Any] {
        lock.lock()
        defer { lock.unlock() }
        let status: String
        if !available {
            status = "unavailable"
        } else if !(gotInfo && gotPlaying) {
            status = "timeout"
        } else if title.isEmpty {
            status = "none"
        } else {
            status = "ok"
        }
        guard status == "ok" else {
            return [
                "status": status,
                "playing": false,
                "title": "",
                "artist": "",
                "album": "",
                "duration": 0.0,
                "elapsed": 0.0,
                "artworkId": "",
            ]
        }
        var out: [String: Any] = [
            "status": status,
            "playing": playing,
            "title": title,
            "artist": artist,
            "album": album,
            "duration": duration,
            "elapsed": elapsed,
            "artworkId": artwork.map(artworkId) ?? "",
        ]
        if includeArtwork, let art = artwork {
            out["artwork"] = art.base64EncodedString()
        }
        return out
    }
}

/// A content id for artwork: FNV-1a over its bytes, and its length.
func artworkId(_ data: Data) -> String {
    var hash: UInt64 = 0xcbf2_9ce4_8422_2325
    for byte in data {
        hash ^= UInt64(byte)
        hash = hash &* 0x0000_0100_0000_01b3
    }
    return String(format: "%016llx-%d", hash, data.count)
}

func getFrontmostApp() -> [String: Any] {
    if let app = NSWorkspace.shared.frontmostApplication {
        return [
            "bundleId": app.bundleIdentifier ?? "",
            "name": app.localizedName ?? "",
            "pid": Int(app.processIdentifier),
        ]
    }
    return ["bundleId": "", "name": "", "pid": 0]
}

func getInputSource() -> [String: Any] {
    var sourceId = ""
    var lang = ""
    if let src = TISCopyCurrentKeyboardInputSource()?.takeRetainedValue() {
        if let ptr = TISGetInputSourceProperty(src, kTISPropertyInputSourceID) {
            sourceId = Unmanaged<CFString>.fromOpaque(ptr).takeUnretainedValue() as String
        }
        if let langsPtr = TISGetInputSourceProperty(src, kTISPropertyInputSourceLanguages) {
            let langs = Unmanaged<CFArray>.fromOpaque(langsPtr).takeUnretainedValue() as NSArray
            if let first = langs.firstObject as? String {
                lang = first
            }
        }
    }
    return ["id": sourceId, "language": lang]
}

func collectMediaWithTimeout(
    timeoutSec: TimeInterval,
    includeArtwork: Bool = false,
    getInfo: (DispatchQueue, @escaping ([String: Any]?) -> Void) -> Void,
    getPlaying: (DispatchQueue, @escaping (Bool) -> Void) -> Void
) -> [String: Any] {
    let box = MediaBox()
    let grp = DispatchGroup()
    let q = DispatchQueue.global(qos: .userInitiated)

    grp.enter()
    getInfo(q) { info in
        box.setInfo(info)
        grp.leave()
    }

    grp.enter()
    getPlaying(q) { isPlaying in
        box.setPlaying(isPlaying)
        grp.leave()
    }

    _ = grp.wait(timeout: .now() + timeoutSec)
    return box.snapshot(available: true, includeArtwork: includeArtwork)
}

/// Now playing, from the private MediaRemote framework. macOS can withhold it from a process it
/// doesn't entitle to MediaRemote: the calls then answer with no track, and the snapshot reports
/// nothing playing (`none`), the same as when no player is open.
func getMediaRemote(includeArtwork: Bool = false) -> [String: Any] {
    guard let mr = dlopen("/System/Library/PrivateFrameworks/MediaRemote.framework/MediaRemote", RTLD_LAZY),
          let infoSym = dlsym(mr, "MRMediaRemoteGetNowPlayingInfo"),
          let playSym = dlsym(mr, "MRMediaRemoteGetNowPlayingApplicationIsPlaying") else {
        return MediaBox().snapshot(available: false)
    }
    let getInfo = unsafeBitCast(infoSym, to: MRGetNowPlayingInfoFn.self)
    let getPlaying = unsafeBitCast(playSym, to: MRGetIsPlayingFn.self)
    return collectMediaWithTimeout(
        timeoutSec: 0.3,
        includeArtwork: includeArtwork,
        getInfo: { q, cb in getInfo(q, cb) },
        getPlaying: { q, cb in getPlaying(q, cb) }
    )
}

func sampleCpuTicks() -> (user: UInt64, sys: UInt64, idle: UInt64, nice: UInt64, valid: Bool) {
    var count = mach_msg_type_number_t(MemoryLayout<host_cpu_load_info_data_t>.stride / MemoryLayout<integer_t>.stride)
    var info = host_cpu_load_info_data_t()
    let rc = withUnsafeMutablePointer(to: &info) { ptr -> kern_return_t in
        ptr.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { iptr in
            host_statistics(mach_host_self(), HOST_CPU_LOAD_INFO, iptr, &count)
        }
    }
    guard rc == KERN_SUCCESS else { return (0, 0, 0, 0, false) }
    return (UInt64(info.cpu_ticks.0), UInt64(info.cpu_ticks.1), UInt64(info.cpu_ticks.2), UInt64(info.cpu_ticks.3), true)
}

func sampleNetBytes() -> (rx: UInt64, tx: UInt64) {
    var ifaddrPtr: UnsafeMutablePointer<ifaddrs>?
    guard getifaddrs(&ifaddrPtr) == 0, let firstAddr = ifaddrPtr else {
        return (0, 0)
    }
    defer { freeifaddrs(firstAddr) }
    var rxTotal: UInt64 = 0
    var txTotal: UInt64 = 0
    var ptr: UnsafeMutablePointer<ifaddrs>? = firstAddr
    while let cur = ptr {
        let flags = Int32(cur.pointee.ifa_flags)
        let isUp = (flags & IFF_UP) != 0
        let isLoopback = (flags & IFF_LOOPBACK) != 0
        if isUp && !isLoopback,
           let addr = cur.pointee.ifa_addr,
           Int32(addr.pointee.sa_family) == AF_LINK,
           let data = cur.pointee.ifa_data {
            let ifd = data.assumingMemoryBound(to: if_data.self).pointee
            rxTotal &+= UInt64(ifd.ifi_ibytes)
            txTotal &+= UInt64(ifd.ifi_obytes)
        }
        ptr = cur.pointee.ifa_next
    }
    return (rxTotal, txTotal)
}

func getHardwareStats() -> [String: Any] {
    let t1 = sampleCpuTicks()
    let n1 = sampleNetBytes()
    let ns1 = DispatchTime.now().uptimeNanoseconds
    usleep(180_000)
    let t2 = sampleCpuTicks()
    let n2 = sampleNetBytes()
    let ns2 = DispatchTime.now().uptimeNanoseconds
    let dtSec = max(0.001, Double(ns2 &- ns1) / 1_000_000_000.0)
    let rxRate = n2.rx >= n1.rx ? Double(n2.rx - n1.rx) / dtSec : 0.0
    let txRate = n2.tx >= n1.tx ? Double(n2.tx - n1.tx) / dtSec : 0.0

    var measuredCpuPct: Double? = nil
    if t1.valid && t2.valid &&
       t2.user >= t1.user && t2.sys >= t1.sys && t2.idle >= t1.idle && t2.nice >= t1.nice {
        let du = t2.user - t1.user
        let ds = t2.sys - t1.sys
        let di = t2.idle - t1.idle
        let dn = t2.nice - t1.nice
        let total = du + ds + di + dn
        if total > 0 {
            measuredCpuPct = Double(du + ds + dn) * 100.0 / Double(total)
        }
    }

    let totalMemBytes = ProcessInfo.processInfo.physicalMemory
    var vmStats = vm_statistics64_data_t()
    var vmCount = mach_msg_type_number_t(MemoryLayout<vm_statistics64_data_t>.stride / MemoryLayout<integer_t>.stride)
    let vmRc = withUnsafeMutablePointer(to: &vmStats) { ptr -> kern_return_t in
        ptr.withMemoryRebound(to: integer_t.self, capacity: Int(vmCount)) { iptr in
            host_statistics64(mach_host_self(), HOST_VM_INFO64, iptr, &vmCount)
        }
    }
    var usedMemMB = 0
    let pageSize = UInt64(vm_kernel_page_size)
    if vmRc == KERN_SUCCESS {
        let activeBytes = (UInt64(vmStats.active_count) + UInt64(vmStats.wire_count) + UInt64(vmStats.compressor_page_count)) * pageSize
        usedMemMB = Int(activeBytes / (1024 * 1024))
    }

    // Percent charge of the internal battery; nil on a Mac without one.
    var batteryPct: Int? = nil
    if let snap = IOPSCopyPowerSourcesInfo()?.takeRetainedValue(),
       let list = IOPSCopyPowerSourcesList(snap)?.takeRetainedValue() as? [CFTypeRef] {
        for ps in list {
            if let desc = IOPSGetPowerSourceDescription(snap, ps)?.takeUnretainedValue() as? [String: Any],
               (desc[kIOPSTypeKey as String] as? String) == (kIOPSInternalBatteryType as String),
               let cap = desc[kIOPSCurrentCapacityKey as String] as? Int,
               let max = desc[kIOPSMaxCapacityKey as String] as? Int, max > 0 {
                batteryPct = min(100, Int((Double(cap) * 100.0 / Double(max)).rounded()))
                break
            }
        }
    }

    var gpuPct = 0.0
    var gpuMemUsedMB: Double? = nil
    var gpuMemTotalMB: Double? = nil
    var gpuUnified = false
    var it: io_iterator_t = 0
    if IOServiceGetMatchingServices(kIOMainPortDefault, IOServiceMatching("IOAccelerator"), &it) == KERN_SUCCESS {
        defer { IOObjectRelease(it) }
        while case let svc = IOIteratorNext(it), svc != 0 {
            defer { IOObjectRelease(svc) }
            var props: Unmanaged<CFMutableDictionary>?
            if IORegistryEntryCreateCFProperties(svc, &props, kCFAllocatorDefault, 0) == KERN_SUCCESS,
               let dict = props?.takeRetainedValue() as? [String: Any],
               let stats = dict["PerformanceStatistics"] as? [String: Any] {
                if let util = (stats["Device Utilization %"] as? NSNumber)?.doubleValue {
                    gpuPct = util
                }
                if let vramUsed = (stats["vramUsedBytes"] as? NSNumber)?.doubleValue,
                   let vramFree = (stats["vramFreeBytes"] as? NSNumber)?.doubleValue,
                   (vramUsed + vramFree) > 0 {
                    gpuMemUsedMB = vramUsed / (1024.0 * 1024.0)
                    gpuMemTotalMB = (vramUsed + vramFree) / (1024.0 * 1024.0)
                    gpuUnified = false
                } else if let inUse = (stats["In use system memory"] as? NSNumber)?.doubleValue, inUse >= 0 {
                    gpuMemUsedMB = inUse / (1024.0 * 1024.0)
                    gpuUnified = true
                }
                break
            }
        }
    }

    var out: [String: Any] = [
        "gpuPct": round(gpuPct * 10.0) / 10.0,
        "memUsedMB": usedMemMB,
        "memTotalMB": Int(totalMemBytes / (1024 * 1024)),
        "netRxBytes": n2.rx,
        "netTxBytes": n2.tx,
        "netRxBps": UInt64(rxRate.rounded()),
        "netTxBps": UInt64(txRate.rounded()),
        "gpuUnified": gpuUnified,
    ]
    if let pct = batteryPct {
        out["batteryPct"] = pct
    }
    if let used = gpuMemUsedMB {
        out["vramUsedMB"] = Int(used.rounded())
    }
    if let tot = gpuMemTotalMB {
        out["vramTotalMB"] = Int(tot.rounded())
    }
    if let cpuPct = measuredCpuPct {
        out["cpuPct"] = round(cpuPct * 10.0) / 10.0
    }
    if t2.valid {
        out["cpuTicks"] = [t2.user, t2.sys, t2.idle, t2.nice]
    }
    return out
}

if CommandLine.arguments.contains("--test-late-callback") {
    let snap = collectMediaWithTimeout(
        timeoutSec: 0.03,
        getInfo: { q, cb in
            q.asyncAfter(deadline: .now() + 0.12) {
                cb(["kMRMediaRemoteNowPlayingInfoTitle": "LateTrack"])
            }
        },
        getPlaying: { q, cb in
            q.async { cb(true) }
        }
    )
    usleep(180_000) // let the late callback run before the process exits
    if let data = try? JSONSerialization.data(withJSONObject: snap, options: [.sortedKeys]),
       let str = String(data: data, encoding: .utf8) {
        print(str)
    }
    guard (snap["status"] as? String) == "timeout", (snap["title"] as? String) == "" else {
        fputs("late callback: expected a timeout snapshot with no track\n", stderr)
        exit(1)
    }
    exit(0)
}

if CommandLine.arguments.contains("--watch-focus") {
    // True event-driven macOS focus stream (<1ms latency, 0% CPU idle — zero polling!).
    func emitApp(_ app: NSRunningApplication?) {
        let a = app ?? NSWorkspace.shared.frontmostApplication
        let obj: [String: Any] = [
            "bundleId": a?.bundleIdentifier ?? "",
            "name": a?.localizedName ?? "",
            "pid": Int(a?.processIdentifier ?? 0),
        ]
        if let data = try? JSONSerialization.data(withJSONObject: obj, options: [.sortedKeys]),
           let str = String(data: data, encoding: .utf8) {
            print(str)
            fflush(stdout)
        }
    }
    emitApp(nil)
    let nc = NSWorkspace.shared.notificationCenter
    nc.addObserver(
        forName: NSWorkspace.didActivateApplicationNotification,
        object: nil,
        queue: .main
    ) { note in
        let app = note.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication
        emitApp(app)
    }
    nc.addObserver(
        forName: NSWorkspace.activeSpaceDidChangeNotification,
        object: nil,
        queue: .main
    ) { _ in
        emitApp(nil)
    }
    RunLoop.main.run()
    exit(0)
}

if CommandLine.arguments.contains("--artwork") {
    let media: [String: Any] = ["media": getMediaRemote(includeArtwork: true)]
    if let data = try? JSONSerialization.data(withJSONObject: media, options: [.sortedKeys]),
       let str = String(data: data, encoding: .utf8) {
        print(str)
        exit(0)
    }
    fputs("{\"error\":\"serialization\"}\n", stderr)
    exit(1)
}

let payload: [String: Any] = [
    "frontmostApp": getFrontmostApp(),
    "inputSource": getInputSource(),
    "media": getMediaRemote(),
    "hardware": getHardwareStats(),
    "displayAsleep": CGDisplayIsAsleep(CGMainDisplayID()) != 0,
]

if let data = try? JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys]),
   let str = String(data: data, encoding: .utf8) {
    print(str)
} else {
    fputs("{\"error\":\"serialization\"}\n", stderr)
    exit(1)
}
