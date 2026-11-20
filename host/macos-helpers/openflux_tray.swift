import AppKit
import Foundation

// openflux-tray: the OpenFlux menu-bar item on macOS, started by openfluxd.
// It is a plain AppKit process so the daemon's tray costs no WebView. The menu opens
// OpenFlux Studio (`--desktop-bin`, default: the sibling `openflux-studio`) and quits the
// daemon (`--daemon-pid`). The tray exits on its own when the daemon process goes away.
//
// The tray asks the daemon's local API whether anything needs the user: an update, a keyboard
// that needs converting, or an app waiting for approval. It then shows a dot on the icon, and
// menu items that open Studio at the page concerned (`openflux-studio --page <page>`; a running
// Studio receives the request and switches pages). It enrolls with the daemon the same way
// Studio does, with the owner-only enrollment secret, and keeps its token in
// `config/tray_token.txt` in the daemon's data directory.

/// Where the daemon publishes its discovery file and enrollment secret.
func daemonDataDir() -> URL {
    if let d = ProcessInfo.processInfo.environment["OPENFLUX_DATA_DIR"], !d.isEmpty {
        return URL(fileURLWithPath: d)
    }
    return FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".local/share/openflux")
}

/// What the tray shows.
struct Attention: Equatable {
    var updatesAvailable = false
    var needsConversion = false
    var pendingApps: [String] = []
    var runningJob: String?

    var badge: Bool { updatesAvailable || needsConversion || !pendingApps.isEmpty }
}

enum DaemonError: Error {
    case notRunning
    case failed(String)
}

/// A minimal client for the daemon's local API, trusting only the daemon's own certificate.
/// Callers make one request sequence at a time; the certificate is also read by URLSession's
/// delegate queue, so it sits behind a lock.
final class DaemonClient: NSObject, URLSessionDelegate {
    private let dataDir: URL
    private var session: URLSession!
    private var token: String?
    private let anchorLock = NSLock()
    private var trustedCert: SecCertificate?
    private var anchor: SecCertificate? {
        get { anchorLock.lock(); defer { anchorLock.unlock() }; return trustedCert }
        set { anchorLock.lock(); trustedCert = newValue; anchorLock.unlock() }
    }

    init(dataDir: URL) {
        self.dataDir = dataDir
        super.init()
        let cfg = URLSessionConfiguration.ephemeral
        cfg.timeoutIntervalForRequest = 10
        session = URLSession(configuration: cfg, delegate: self, delegateQueue: nil)
    }

    private var configDir: URL { dataDir.appendingPathComponent("config") }

    /// Reads `config/keys.json`: the port, and the certificate unless the daemon serves plain HTTP.
    private func endpoint() throws -> URL {
        let keysURL = configDir.appendingPathComponent("keys.json")
        guard let data = try? Data(contentsOf: keysURL),
              let keys = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let port = keys["openPort"] as? Int
        else { throw DaemonError.notRunning }
        if (keys["scheme"] as? String) == "http" {
            anchor = nil
            let host = (keys["host"] as? String) ?? "127.0.0.1"
            let urlHost = host.contains(":") ? "[\(host)]" : host
            guard let url = URL(string: "http://\(urlHost):\(port)") else {
                throw DaemonError.failed("The daemon's address \(host) isn't usable")
            }
            return url
        }
        guard let certPath = keys["publicKeyLocation"] as? String,
              let pem = try? String(contentsOfFile: certPath, encoding: .utf8),
              let der = DaemonClient.derFromPEM(pem),
              let cert = SecCertificateCreateWithData(nil, der as CFData)
        else { throw DaemonError.failed("The daemon's certificate couldn't be read") }
        anchor = cert
        return URL(string: "https://localhost:\(port)")!
    }

    static func derFromPEM(_ pem: String) -> Data? {
        let body = pem.components(separatedBy: "\n")
            .filter { !$0.hasPrefix("-----") }
            .joined()
        return Data(base64Encoded: body, options: .ignoreUnknownCharacters)
    }

    func urlSession(_ session: URLSession, didReceive challenge: URLAuthenticationChallenge,
                    completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void) {
        guard challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust,
              let trust = challenge.protectionSpace.serverTrust,
              let anchor = anchor
        else {
            completionHandler(.performDefaultHandling, nil)
            return
        }
        SecTrustSetAnchorCertificates(trust, [anchor] as CFArray)
        SecTrustSetAnchorCertificatesOnly(trust, true)
        if SecTrustEvaluateWithError(trust, nil) {
            completionHandler(.useCredential, URLCredential(trust: trust))
        } else {
            completionHandler(.cancelAuthenticationChallenge, nil)
        }
    }

    private func send(_ base: URL, _ method: String, _ path: String, token: String?, body: [String: Any]?) async throws -> (Int, Data) {
        var req = URLRequest(url: base.appendingPathComponent(path))
        req.httpMethod = method
        if let token = token { req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization") }
        if let body = body {
            req.setValue("application/json", forHTTPHeaderField: "Content-Type")
            req.httpBody = try JSONSerialization.data(withJSONObject: body)
        }
        do {
            let (data, resp) = try await session.data(for: req)
            return ((resp as? HTTPURLResponse)?.statusCode ?? 0, data)
        } catch let e as URLError where e.code == .cannotConnectToHost {
            throw DaemonError.notRunning
        }
    }

    private func enroll(_ base: URL) async throws -> String {
        let tokenURL = configDir.appendingPathComponent("tray_token.txt")
        if let saved = try? String(contentsOf: tokenURL, encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines),
           !saved.isEmpty,
           (try? await send(base, "GET", "/v1/authentication/check", token: saved, body: nil).0) == 200 {
            return saved
        }
        guard let secret = try? String(contentsOf: configDir.appendingPathComponent("enrollment.secret"), encoding: .utf8)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        else { throw DaemonError.failed("No enrollment secret") }
        let (status, data) = try await send(base, "POST", "/v1/authentication/register", token: nil,
                                            body: ["clientName": "openflux-tray", "enrollmentSecret": secret])
        guard status == 200,
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let tok = obj["message"] as? String
        else { throw DaemonError.failed("Enrollment was refused (\(status))") }
        FileManager.default.createFile(atPath: tokenURL.path, contents: Data((tok + "\n").utf8),
                                       attributes: [.posixPermissions: 0o600])
        return tok
    }

    /// GET `path` as JSON, enrolling first if needed.
    func get(_ path: String) async throws -> [String: Any] {
        let base = try endpoint()
        if token == nil { token = try await enroll(base) }
        var (status, data) = try await send(base, "GET", path, token: token, body: nil)
        if status == 401 {
            token = try await enroll(base)
            (status, data) = try await send(base, "GET", path, token: token, body: nil)
        }
        guard status == 200, let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw DaemonError.failed("GET \(path) answered \(status)")
        }
        return obj
    }

    func attention() async -> Attention? {
        var a = Attention()
        var reached = false
        if let u = try? await get("/v1/updates") {
            reached = true
            let host = u["host"] as? [String: Any]
            let kb = u["keyboard"] as? [String: Any]
            let comps = kb?["components"] as? [[String: Any]] ?? []
            a.updatesAvailable = (host?["state"] as? String) == "updateAvailable"
                || comps.contains { ($0["state"] as? String) == "updateAvailable" }
            a.needsConversion = (kb?["mode"] as? String) == "needsConversion"
            if let job = u["job"] as? [String: Any], (job["state"] as? String) == "running" {
                let step = job["step"] as? [String: Any]
                a.runningJob = (step?["label"] as? String) ?? "Working on the keyboard"
            }
        }
        if let r = try? await get("/v1/third-party/registrations") {
            reached = true
            a.pendingApps = (r["pending"] as? [[String: Any]] ?? []).compactMap { $0["app"] as? String }
        }
        return reached ? a : nil
    }
}

final class OpenFluxTrayApp: NSObject, NSApplicationDelegate, NSMenuDelegate {
    private var statusItem: NSStatusItem!
    private let desktopPath: String
    private let daemonPid: Int32
    private let client = DaemonClient(dataDir: daemonDataDir())
    private var attention = Attention()
    private var pollTimer: Timer?
    private var refreshing = false

    init(desktopPath: String, daemonPid: Int32) {
        self.desktopPath = desktopPath
        self.daemonPid = daemonPid
    }

    func makeTrayIconImage(badge: Bool) -> NSImage {
        let size = NSSize(width: 18, height: 18)
        let img = NSImage(size: size, flipped: true) { _ in
            let s = 18.0 / 128.0
            NSColor.black.setStroke()
            NSColor.black.setFill()

            let cap = NSBezierPath()
            cap.lineWidth = 11.5 * s
            cap.lineCapStyle = .round
            cap.lineJoinStyle = .round
            cap.move(to: NSPoint(x: 78 * s, y: 24 * s))
            cap.line(to: NSPoint(x: 46 * s, y: 24 * s))
            cap.curve(to: NSPoint(x: 24.5 * s, y: 42 * s), controlPoint1: NSPoint(x: 35 * s, y: 24 * s), controlPoint2: NSPoint(x: 27 * s, y: 31 * s))
            cap.line(to: NSPoint(x: 15.5 * s, y: 84 * s))
            cap.curve(to: NSPoint(x: 33 * s, y: 104 * s), controlPoint1: NSPoint(x: 13 * s, y: 96 * s), controlPoint2: NSPoint(x: 21 * s, y: 104 * s))
            cap.line(to: NSPoint(x: 95 * s, y: 104 * s))
            cap.curve(to: NSPoint(x: 112.5 * s, y: 84 * s), controlPoint1: NSPoint(x: 107 * s, y: 104 * s), controlPoint2: NSPoint(x: 115 * s, y: 96 * s))
            cap.line(to: NSPoint(x: 106.5 * s, y: 56 * s))
            cap.stroke()

            // Lowercase "f" inside the open keycap outline.
            let f = NSBezierPath()
            f.move(to: NSPoint(x: 41.5 * s, y: 86.5 * s))
            f.curve(to: NSPoint(x: 39.8 * s, y: 81.8 * s), controlPoint1: NSPoint(x: 40 * s, y: 86.5 * s), controlPoint2: NSPoint(x: 39 * s, y: 84.5 * s))
            f.curve(to: NSPoint(x: 48.2 * s, y: 52.5 * s), controlPoint1: NSPoint(x: 42.8 * s, y: 71.5 * s), controlPoint2: NSPoint(x: 45.2 * s, y: 61.5 * s))
            f.curve(to: NSPoint(x: 76.5 * s, y: 37.8 * s), controlPoint1: NSPoint(x: 52.6 * s, y: 39.5 * s), controlPoint2: NSPoint(x: 63.4 * s, y: 36.2 * s))
            f.curve(to: NSPoint(x: 93.4 * s, y: 34.4 * s), controlPoint1: NSPoint(x: 84.2 * s, y: 38.7 * s), controlPoint2: NSPoint(x: 90.2 * s, y: 35.8 * s))
            f.curve(to: NSPoint(x: 95.8 * s, y: 37.4 * s), controlPoint1: NSPoint(x: 95.4 * s, y: 33.5 * s), controlPoint2: NSPoint(x: 96.8 * s, y: 35.2 * s))
            f.curve(to: NSPoint(x: 72.5 * s, y: 48.0 * s), controlPoint1: NSPoint(x: 92.4 * s, y: 45.0 * s), controlPoint2: NSPoint(x: 83.2 * s, y: 48.2 * s))
            f.curve(to: NSPoint(x: 58.0 * s, y: 58.2 * s), controlPoint1: NSPoint(x: 65.2 * s, y: 47.9 * s), controlPoint2: NSPoint(x: 60.4 * s, y: 51.0 * s))
            f.line(to: NSPoint(x: 57.2 * s, y: 60.6 * s))
            f.curve(to: NSPoint(x: 76.2 * s, y: 57.4 * s), controlPoint1: NSPoint(x: 62.5 * s, y: 56.8 * s), controlPoint2: NSPoint(x: 70.4 * s, y: 56.8 * s))
            f.curve(to: NSPoint(x: 79.4 * s, y: 63.0 * s), controlPoint1: NSPoint(x: 79.4 * s, y: 57.7 * s), controlPoint2: NSPoint(x: 80.5 * s, y: 60.2 * s))
            f.curve(to: NSPoint(x: 71.8 * s, y: 66.8 * s), controlPoint1: NSPoint(x: 78.2 * s, y: 65.8 * s), controlPoint2: NSPoint(x: 75.5 * s, y: 66.8 * s))
            f.curve(to: NSPoint(x: 54.0 * s, y: 74.2 * s), controlPoint1: NSPoint(x: 64.2 * s, y: 66.8 * s), controlPoint2: NSPoint(x: 58.5 * s, y: 68.4 * s))
            f.curve(to: NSPoint(x: 44.6 * s, y: 85.4 * s), controlPoint1: NSPoint(x: 50.6 * s, y: 78.6 * s), controlPoint2: NSPoint(x: 47.8 * s, y: 83.4 * s))
            f.curve(to: NSPoint(x: 41.5 * s, y: 86.5 * s), controlPoint1: NSPoint(x: 43.5 * s, y: 86.1 * s), controlPoint2: NSPoint(x: 42.5 * s, y: 86.5 * s))
            f.close()
            f.fill()

            if badge {
                // A dot at the top right: an update, a conversion or an app approval is waiting.
                let r = 30.0 * s
                NSBezierPath(ovalIn: NSRect(x: 128 * s - r - 2 * s, y: 2 * s, width: r, height: r)).fill()
            }
            return true
        }
        img.isTemplate = true
        return img
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        statusItem.button?.title = ""
        let menu = NSMenu()
        menu.delegate = self
        statusItem.menu = menu
        render()

        // Exit when the daemon is gone.
        if daemonPid > 0 {
            Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true) { _ in
                if kill(self.daemonPid, 0) != 0 {
                    NSApp.terminate(nil)
                }
            }
        }
        // The daemon starts the tray before its API is up, so the first look is a little later.
        schedulePoll(after: 3)
    }

    private func schedulePoll(after seconds: TimeInterval) {
        pollTimer?.invalidate()
        pollTimer = Timer.scheduledTimer(withTimeInterval: seconds, repeats: false) { [weak self] _ in
            self?.refresh()
        }
    }

    /// One refresh at a time: opening the menu during a poll leaves that poll to finish.
    private func refresh() {
        guard !refreshing else { return }
        refreshing = true
        Task { @MainActor in
            defer { refreshing = false }
            let found = await client.attention()
            if let found = found, found != attention {
                attention = found
                render()
            }
            // Check often while a job runs or while the daemon is starting.
            schedulePoll(after: found == nil ? 10 : (found?.runningJob != nil ? 5 : 60))
        }
    }

    func menuWillOpen(_ menu: NSMenu) {
        refresh()
    }

    private func item(_ title: String, _ action: Selector?, page: String? = nil) -> NSMenuItem {
        let i = NSMenuItem(title: title, action: action, keyEquivalent: "")
        i.target = self
        i.representedObject = page
        i.isEnabled = action != nil
        return i
    }

    private func render() {
        statusItem.button?.image = makeTrayIconImage(badge: attention.badge)
        var tip = ["OpenFlux"]
        if attention.updatesAvailable { tip.append("Updates available") }
        if attention.needsConversion { tip.append("Keyboard can be converted to OpenFlux") }
        if !attention.pendingApps.isEmpty { tip.append("Apps waiting for approval") }
        statusItem.button?.toolTip = tip.joined(separator: " · ")

        guard let menu = statusItem.menu else { return }
        menu.removeAllItems()
        menu.addItem(item("Open OpenFlux Studio", #selector(openStudio(_:))))
        var extra: [NSMenuItem] = []
        if let job = attention.runningJob {
            extra.append(item(job, nil))
        }
        if attention.needsConversion {
            extra.append(item("Convert the keyboard to OpenFlux…", #selector(openStudio(_:)), page: "updates"))
        } else if attention.updatesAvailable {
            extra.append(item("Updates available…", #selector(openStudio(_:)), page: "updates"))
        }
        let pending = attention.pendingApps
        if pending.count == 1 {
            extra.append(item("\(pending[0]) is asking for access…", #selector(openStudio(_:)), page: "apps"))
        } else if pending.count > 1 {
            extra.append(item("\(pending.count) apps are asking for access…", #selector(openStudio(_:)), page: "apps"))
        }
        if !extra.isEmpty {
            menu.addItem(NSMenuItem.separator())
            extra.forEach { menu.addItem($0) }
        }
        menu.addItem(NSMenuItem.separator())
        let quitItem = NSMenuItem(title: "Quit OpenFlux Daemon", action: #selector(quitDaemon), keyEquivalent: "q")
        quitItem.target = self
        menu.addItem(quitItem)
    }

    @objc private func openStudio(_ sender: NSMenuItem) {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: desktopPath)
        if let page = sender.representedObject as? String {
            p.arguments = ["--page", page]
        }
        try? p.run()
    }

    @objc private func quitDaemon() {
        if daemonPid > 0 {
            kill(daemonPid, SIGTERM)
        }
        NSApp.terminate(nil)
    }
}

let args = CommandLine.arguments
var desktopBin = URL(fileURLWithPath: args[0]).deletingLastPathComponent().appendingPathComponent("openflux-studio").path
var daemonPid: Int32 = 0
var i = 1
while i < args.count {
    if args[i] == "--desktop-bin", i + 1 < args.count {
        desktopBin = args[i + 1]
        i += 2
    } else if args[i] == "--daemon-pid", i + 1 < args.count {
        daemonPid = Int32(args[i + 1]) ?? 0
        i += 2
    } else {
        i += 1
    }
}

// `--check` prints what the tray would show and exits, for diagnosing the daemon connection.
if args.contains("--check") {
    let done = DispatchSemaphore(value: 0)
    Task {
        let client = DaemonClient(dataDir: daemonDataDir())
        if let a = await client.attention() {
            print("updatesAvailable=\(a.updatesAvailable) needsConversion=\(a.needsConversion) pendingApps=\(a.pendingApps) runningJob=\(a.runningJob ?? "none")")
        } else {
            print("daemon not reachable")
        }
        done.signal()
    }
    done.wait()
    exit(0)
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let delegate = OpenFluxTrayApp(desktopPath: desktopBin, daemonPid: daemonPid)
app.delegate = delegate
app.run()
