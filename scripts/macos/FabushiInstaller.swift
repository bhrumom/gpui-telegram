import Cocoa
import Foundation

final class InstallerWindowController: NSWindowController {
    private let statusLabel = NSTextField(labelWithString: "点击下方按钮将 Fabushi 安装到「应用程序」并立即打开。")
    private let progressBar = NSProgressIndicator()
    private let installButton = NSButton(title: "立即安装并打开", target: nil, action: nil)
    private let cancelButton = NSButton(title: "取消", target: nil, action: nil)
    private let openOnlyCheckbox = NSButton(checkboxWithTitle: "安装完成后立即打开 Fabushi", target: nil, action: nil)
    private var isInstalling = false

    private let appVersion: String = {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "1.8.0"
    }()

    init() {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 460, height: 290),
            styleMask: [.titled, .closable, .miniaturizable],
            backing: .buffered,
            defer: false
        )
        window.title = "安装 Fabushi"
        window.isReleasedWhenClosed = false
        window.center()
        super.init(window: window)
        setupUI()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    private func setupUI() {
        guard let contentView = window?.contentView else { return }

        let iconView = NSImageView(frame: NSRect(x: 32, y: 176, width: 76, height: 76))
        iconView.image = NSApp.applicationIconImage
        iconView.imageScaling = .scaleProportionallyUpOrDown
        contentView.addSubview(iconView)

        let titleLabel = NSTextField(labelWithString: "安装法布施 (Fabushi)")
        titleLabel.font = NSFont.systemFont(ofSize: 20, weight: .semibold)
        titleLabel.frame = NSRect(x: 126, y: 220, width: 304, height: 28)
        contentView.addSubview(titleLabel)

        let versionLabel = NSTextField(labelWithString: "版本 v\(appVersion)  •  将安装至 /Applications/Fabushi.app")
        versionLabel.font = NSFont.systemFont(ofSize: 12.5, weight: .regular)
        versionLabel.textColor = .secondaryLabelColor
        versionLabel.frame = NSRect(x: 126, y: 194, width: 304, height: 20)
        contentView.addSubview(versionLabel)

        let divider = NSBox(frame: NSRect(x: 28, y: 156, width: 404, height: 1))
        divider.boxType = .separator
        contentView.addSubview(divider)

        statusLabel.font = NSFont.systemFont(ofSize: 13)
        statusLabel.lineBreakMode = .byWordWrapping
        statusLabel.maximumNumberOfLines = 2
        statusLabel.frame = NSRect(x: 32, y: 106, width: 396, height: 38)
        contentView.addSubview(statusLabel)

        progressBar.style = .bar
        progressBar.isIndeterminate = false
        progressBar.minValue = 0
        progressBar.maxValue = 100
        progressBar.doubleValue = 0
        progressBar.frame = NSRect(x: 32, y: 80, width: 396, height: 16)
        contentView.addSubview(progressBar)

        openOnlyCheckbox.state = .on
        openOnlyCheckbox.font = NSFont.systemFont(ofSize: 12.5)
        openOnlyCheckbox.frame = NSRect(x: 32, y: 50, width: 260, height: 20)
        contentView.addSubview(openOnlyCheckbox)

        cancelButton.bezelStyle = .rounded
        cancelButton.target = self
        cancelButton.action = #selector(cancelTapped)
        cancelButton.frame = NSRect(x: 214, y: 16, width: 88, height: 32)
        contentView.addSubview(cancelButton)

        installButton.bezelStyle = .rounded
        installButton.keyEquivalent = "\r"
        installButton.target = self
        installButton.action = #selector(installTapped)
        installButton.frame = NSRect(x: 310, y: 16, width: 122, height: 32)
        contentView.addSubview(installButton)
    }

    @objc private func cancelTapped() {
        guard !isInstalling else { return }
        NSApp.terminate(nil)
    }

    @objc private func installTapped() {
        guard !isInstalling else { return }
        isInstalling = true
        installButton.isEnabled = false
        cancelButton.isEnabled = false
        openOnlyCheckbox.isEnabled = false
        let shouldLaunch = (openOnlyCheckbox.state == .on)

        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            self?.performInstall(launchAfter: shouldLaunch)
        }
    }

    private func updateProgress(_ value: Double, message: String) {
        DispatchQueue.main.async {
            self.progressBar.doubleValue = value
            self.statusLabel.stringValue = message
        }
    }

    private func locateSourceApp() -> URL? {
        let installerURL = Bundle.main.bundleURL
        let parentDir = installerURL.deletingLastPathComponent()
        let directSibling = parentDir.appendingPathComponent("Fabushi.app")
        if FileManager.default.fileExists(atPath: directSibling.path) {
            return directSibling
        }
        let embedded = Bundle.main.bundleURL
            .appendingPathComponent("Contents/Resources/Fabushi.app")
        if FileManager.default.fileExists(atPath: embedded.path) {
            return embedded
        }
        return nil
    }

    @discardableResult
    private func runProcess(_ executable: String, _ arguments: [String]) -> (Int32, String) {
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: executable)
        proc.arguments = arguments
        let pipe = Pipe()
        proc.standardOutput = pipe
        proc.standardError = pipe
        do {
            try proc.run()
            proc.waitUntilExit()
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            let output = String(data: data, encoding: .utf8) ?? ""
            return (proc.terminationStatus, output)
        } catch {
            return (-1, error.localizedDescription)
        }
    }

    private func copyWithAdminPrivileges(sourcePath: String, targetPath: String) -> (Int32, String) {
        let escapedSrc = sourcePath.replacingOccurrences(of: "'", with: "'\\''")
        let escapedDst = targetPath.replacingOccurrences(of: "'", with: "'\\''")
        let cliPath = "\(targetPath)/Contents/Helpers/fabushi"
        let escapedCli = cliPath.replacingOccurrences(of: "'", with: "'\\''")
        let shellCmd = "rm -rf '\(escapedDst)' && /usr/bin/ditto '\(escapedSrc)' '\(escapedDst)' && (xattr -dr com.apple.quarantine '\(escapedDst)' 2>/dev/null || true) && ([ ! -x '\(escapedCli)' ] || (mkdir -p /usr/local/bin && ln -sf '\(escapedCli)' /usr/local/bin/fabushi))"
        let appleScript = "do shell script \"\(shellCmd.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\""))\" with administrator privileges"
        return runProcess("/usr/bin/osascript", ["-e", appleScript])
    }

    private func ensureSymlink(at linkURL: URL, targetPath: String) -> Bool {
        let fm = FileManager.default
        let dirURL = linkURL.deletingLastPathComponent()
        do {
            try fm.createDirectory(at: dirURL, withIntermediateDirectories: true, attributes: nil)
            if (try? fm.attributesOfItem(atPath: linkURL.path)) != nil {
                try fm.removeItem(at: linkURL)
            }
            try fm.createSymbolicLink(atPath: linkURL.path, withDestinationPath: targetPath)
            return true
        } catch {
            return false
        }
    }

    @discardableResult
    private func installCliSymlink(targetApp: URL) -> String? {
        let fm = FileManager.default
        let cliBinary = targetApp.appendingPathComponent("Contents/Helpers/fabushi")
        guard fm.fileExists(atPath: cliBinary.path) else {
            return nil
        }

        let systemBinDir = URL(fileURLWithPath: "/usr/local/bin")
        let systemCliLink = systemBinDir.appendingPathComponent("fabushi")

        if let existingDest = try? fm.destinationOfSymbolicLink(atPath: systemCliLink.path),
           existingDest == cliBinary.path {
            return systemCliLink.path
        }

        let canWriteSystemBin = fm.isWritableFile(atPath: systemBinDir.path)
            || (!fm.fileExists(atPath: systemBinDir.path) && fm.isWritableFile(atPath: "/usr/local"))
        if canWriteSystemBin && ensureSymlink(at: systemCliLink, targetPath: cliBinary.path) {
            return systemCliLink.path
        }

        let escapedCli = cliBinary.path.replacingOccurrences(of: "'", with: "'\\''")
        let shellCmd = "mkdir -p /usr/local/bin && ln -sf '\(escapedCli)' /usr/local/bin/fabushi"
        let appleScript = "do shell script \"\(shellCmd.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\""))\" with administrator privileges"
        let (code, _) = runProcess("/usr/bin/osascript", ["-e", appleScript])
        if code == 0 {
            return systemCliLink.path
        }

        let userLocalLink = fm.homeDirectoryForCurrentUser
            .appendingPathComponent(".local/bin/fabushi")
        if ensureSymlink(at: userLocalLink, targetPath: cliBinary.path) {
            return userLocalLink.path
        }
        return nil
    }

    private func deployUserLaunchAgent(targetApp: URL) {
        let fm = FileManager.default
        let srcPlist = targetApp.appendingPathComponent("Contents/Resources/com.ombhrum.fabushi.agent.plist")
        guard fm.fileExists(atPath: srcPlist.path) else {
            return
        }

        let launchAgentsDir = fm.homeDirectoryForCurrentUser.appendingPathComponent("Library/LaunchAgents")
        let destPlist = launchAgentsDir.appendingPathComponent("com.ombhrum.fabushi.agent.plist")
        do {
            try fm.createDirectory(at: launchAgentsDir, withIntermediateDirectories: true, attributes: nil)
            if fm.fileExists(atPath: destPlist.path) {
                try fm.removeItem(at: destPlist)
            }
            try fm.copyItem(at: srcPlist, to: destPlist)
            try fm.setAttributes([.posixPermissions: 0o644], ofItemAtPath: destPlist.path)
        } catch {
            return
        }

        let daemonBinary = targetApp.appendingPathComponent("Contents/Helpers/fabushi-remote-daemon")
        if fm.fileExists(atPath: daemonBinary.path) {
            let uid = getuid()
            _ = runProcess("/bin/launchctl", ["bootout", "gui/\(uid)/com.ombhrum.fabushi.agent"])
            _ = runProcess("/bin/launchctl", ["bootstrap", "gui/\(uid)", destPlist.path])
            _ = runProcess("/bin/launchctl", ["kickstart", "-k", "gui/\(uid)/com.ombhrum.fabushi.agent"])
        }
    }

    private func performInstall(launchAfter: Bool) {
        updateProgress(10, message: "正在定位 Fabushi.app 安装包...")
        guard let sourceApp = locateSourceApp() else {
            finishWithError("未在安装包目录中找到 Fabushi.app，请确认从完整的 DMG 磁盘镜像中运行安装程序。")
            return
        }

        let targetApp = URL(fileURLWithPath: "/Applications/Fabushi.app")

        updateProgress(25, message: "正在关闭正在运行的旧版本 Fabushi...")
        for app in NSRunningApplication.runningApplications(withBundleIdentifier: "com.ombhrum.fabushi") {
            app.terminate()
        }
        Thread.sleep(forTimeInterval: 0.4)

        updateProgress(45, message: "正在安装到 /Applications/Fabushi.app...")
        let fm = FileManager.default
        var copiedSuccessfully = false
        var lastError = ""

        if fm.isWritableFile(atPath: "/Applications") {
            do {
                if fm.fileExists(atPath: targetApp.path) {
                    try fm.removeItem(at: targetApp)
                }
                let (code, out) = runProcess("/usr/bin/ditto", [sourceApp.path, targetApp.path])
                if code == 0 {
                    copiedSuccessfully = true
                } else {
                    lastError = out
                }
            } catch {
                lastError = error.localizedDescription
            }
        }

        if !copiedSuccessfully {
            updateProgress(55, message: "需要管理员权限写入「应用程序」，请在系统弹窗中确认...")
            let (code, out) = copyWithAdminPrivileges(sourcePath: sourceApp.path, targetPath: targetApp.path)
            if code == 0 {
                copiedSuccessfully = true
            } else {
                lastError = out
            }
        }

        if !copiedSuccessfully {
            finishWithError("安装到 /Applications 失败：\n\(lastError)")
            return
        }

        updateProgress(78, message: "正在注册系统应用与 LaunchServices...")
        _ = runProcess("/usr/bin/xattr", ["-dr", "com.apple.quarantine", targetApp.path])
        let lsregister = "/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister"
        if fm.fileExists(atPath: lsregister) {
            _ = runProcess(lsregister, ["-f", targetApp.path])
        }
        _ = runProcess("/usr/bin/mdimport", [targetApp.path])

        updateProgress(85, message: "正在安装 fabushi CLI 工具并注册后台服务...")
        _ = installCliSymlink(targetApp: targetApp)
        deployUserLaunchAgent(targetApp: targetApp)

        updateProgress(92, message: launchAfter ? "安装成功！正在启动 Fabushi..." : "安装成功！已安装至 /Applications/Fabushi.app")

        if launchAfter {
            let config = NSWorkspace.OpenConfiguration()
            config.activates = true
            let sem = DispatchSemaphore(value: 0)
            var launchError: Error?
            NSWorkspace.shared.openApplication(at: targetApp, configuration: config) { _, err in
                launchError = err
                sem.signal()
            }
            _ = sem.wait(timeout: .now() + 8.0)
            if launchError != nil {
                _ = runProcess("/usr/bin/open", ["-a", targetApp.path])
            }
        }

        updateProgress(100, message: "Fabushi 已安装到「应用程序」并准备就绪！")
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.2) {
            NSApp.terminate(nil)
        }
    }

    private func finishWithError(_ message: String) {
        DispatchQueue.main.async {
            self.isInstalling = false
            self.installButton.isEnabled = true
            self.cancelButton.isEnabled = true
            self.openOnlyCheckbox.isEnabled = true
            self.progressBar.doubleValue = 0
            self.statusLabel.stringValue = "安装未完成，请重试或手动将 Fabushi.app 拖入 Applications。"
            let alert = NSAlert()
            alert.alertStyle = .warning
            alert.messageText = "安装未能完成"
            alert.informativeText = message
            alert.addButton(withTitle: "好")
            alert.runModal()
        }
    }
}

final class InstallerAppDelegate: NSObject, NSApplicationDelegate {
    private var controller: InstallerWindowController?

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        let ctrl = InstallerWindowController()
        controller = ctrl
        ctrl.showWindow(nil)
        ctrl.window?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }
}

let app = NSApplication.shared
let delegate = InstallerAppDelegate()
app.delegate = delegate
app.run()
