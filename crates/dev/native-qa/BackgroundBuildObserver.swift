import AppKit
import CoreGraphics
import Foundation

// Independent developer observation. This does not replace the native host's
// in-process presentation guard, which also catches activity between samples.
private struct ApplicationSample {
    var pid: pid_t
    var policy: Int
    var active = false
    var terminated = false
    var visibleWindows: Int? = 0
    var bundle: String? = "/synthetic/Self Test.app"
}

private struct ObservedApplication {
    var pid: pid_t?
    var bundle: String?
    var samples = 0
    var terminalSamples = 0
    var policies = Set<Int>()
    var maximumVisibleWindows = 0
    var everActive = false

    var report: [String: Any] {
        ["pid": pid.map { $0 as Any } ?? NSNull(), "bundle": bundle.map { $0 as Any } ?? NSNull(),
         "samples": samples, "terminal_samples": terminalSamples, "policies": policies.sorted(),
         "maximum_visible_windows": maximumVisibleWindows, "ever_active": everActive]
    }
}

private struct Observation {
    var applications: [Int: ObservedApplication] = [:]
    var errors = Set<String>()

    mutating func record(_ sample: ApplicationSample, identity: Int) {
        var row = applications[identity] ?? ObservedApplication()
        defer { applications[identity] = row }
        row.everActive = row.everActive || sample.active
        if let visible = sample.visibleWindows {
            row.maximumVisibleWindows = max(row.maximumVisibleWindows, visible)
        } else {
            errors.insert("Window capture unavailable for application \(identity)")
        }
        if sample.terminated {
            // Exit may remove properties, but cannot erase positive violations
            // or change any identity properties that are still available.
            if (0...1).contains(sample.policy) { row.policies.insert(sample.policy) }
            if sample.pid > 0, let pid = row.pid, sample.pid != pid {
                errors.insert("Terminated application \(identity) changed PID")
            }
            if let bundle = sample.bundle, let previous = row.bundle, bundle != previous {
                errors.insert("Terminated application \(identity) changed bundle")
            }
            guard row.samples > 0 else {
                errors.insert("Application \(identity) terminated before a valid sample")
                return
            }
            row.terminalSamples += 1
            return
        }
        guard sample.pid > 0, (0...2).contains(sample.policy), let bundle = sample.bundle, !bundle.isEmpty else {
            errors.insert("Live application \(identity) has unavailable process properties")
            return
        }
        guard row.terminalSamples == 0, row.pid == nil || row.pid == sample.pid,
              row.bundle == nil || row.bundle == bundle else {
            errors.insert("Application \(identity) changed identity or resumed after termination")
            return
        }
        row.pid = sample.pid
        row.bundle = bundle
        row.samples += 1
        row.policies.insert(sample.policy)
    }

    mutating func activated(identity: Int) {
        var row = applications[identity] ?? ObservedApplication()
        row.everActive = true
        applications[identity] = row
    }

    var passed: Bool {
        !applications.isEmpty && errors.isEmpty && applications.values.allSatisfy {
            $0.samples > 0 && $0.terminalSamples > 0 && ($0.pid ?? -1) > 0 && $0.bundle != nil
                && $0.policies == [NSApplication.ActivationPolicy.prohibited.rawValue]
                && $0.maximumVisibleWindows == 0 && !$0.everActive
        }
    }
}

private func require(_ condition: Bool, _ message: String) throws {
    if !condition { throw NSError(domain: "MFBBackgroundObserver", code: 1,
                                  userInfo: [NSLocalizedDescriptionKey: message]) }
}

private func runRegression() throws {
    var observed = Observation()
    observed.record(ApplicationSample(pid: 42, policy: 2), identity: 0)
    observed.record(ApplicationSample(pid: -1, policy: -1, terminated: true, bundle: nil), identity: 0)
    try require(observed.passed && observed.applications[0]?.pid == 42
                && observed.applications[0]?.samples == 1 && observed.applications[0]?.terminalSamples == 1,
                "A known application's exit was recorded as invalid live-process evidence")
    let exited = ApplicationSample(pid: -1, policy: -1, terminated: true, bundle: nil)
    observed.record(exited, identity: 0)
    try require(observed.passed && observed.applications.count == 1,
                "Repeated terminal samples created a fictitious process")
    try require(!Observation().passed, "No observations must not pass")
    var unobserved = Observation()
    unobserved.record(exited, identity: 0)
    try require(!unobserved.passed && !unobserved.errors.isEmpty,
                "An application never observed alive was silently ignored")
    for bad in [ApplicationSample(pid: -1, policy: -1, bundle: nil),
                ApplicationSample(pid: 42, policy: 2, visibleWindows: nil),
                ApplicationSample(pid: 42, policy: 2, visibleWindows: 1),
                ApplicationSample(pid: 42, policy: 2, active: true),
                ApplicationSample(pid: 42, policy: 0),
                ApplicationSample(pid: -1, policy: 0, terminated: true, bundle: nil),
                ApplicationSample(pid: 43, policy: -1, terminated: true, bundle: nil),
                ApplicationSample(pid: -1, policy: -1, terminated: true, bundle: "/synthetic/Different.app")] {
        var failure = Observation()
        failure.record(ApplicationSample(pid: 42, policy: 2), identity: 0)
        failure.record(bad, identity: 0)
        failure.record(exited, identity: 0)
        try require(!failure.passed, "Termination erased a real violation or missing observation")
    }
    var activation = observed
    activation.activated(identity: 0)
    activation.record(exited, identity: 0)
    try require(!activation.passed, "An activation notification was lost after exit")
    var identityChange = Observation()
    identityChange.record(ApplicationSample(pid: 42, policy: 2), identity: 0)
    identityChange.record(ApplicationSample(pid: 43, policy: 2), identity: 0)
    identityChange.record(exited, identity: 0)
    try require(!identityChange.passed && identityChange.applications[0]?.pid == 42,
                "A PID change replaced the recorded identity")
    var running = Observation()
    running.record(ApplicationSample(pid: 42, policy: 2), identity: 0)
    try require(!running.passed, "An application still running at command completion must not pass")
    print("background observer regression passed")
}

@MainActor
private final class ApplicationRegistry {
    private var applications: [NSRunningApplication] = []
    var observation = Observation()

    func identity(of application: NSRunningApplication) -> Int {
        // Apple documents isEqual, rather than PID, as application identity.
        if let index = applications.firstIndex(where: { $0.isEqual(application) }) { return index }
        applications.append(application)
        return applications.count - 1
    }

    func targetIdentity(of application: NSRunningApplication) -> Int? {
        // A queued activation may arrive after a known application's metadata
        // disappears. Match its recorded lifecycle before consulting metadata.
        if let index = applications.firstIndex(where: { $0.isEqual(application) }) { return index }
        guard application.bundleIdentifier == "com.modernformatboost.self-test" else { return nil }
        return identity(of: application)
    }

    func sample() {
        for app in NSRunningApplication.runningApplications(withBundleIdentifier: "com.modernformatboost.self-test") {
            _ = identity(of: app)
        }
        let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
            as? [[String: Any]]
        for (identity, app) in applications.enumerated() {
            let terminated = app.isTerminated
            let pid = app.processIdentifier
            let knownPID = observation.applications[identity]?.pid
            let windowPID = pid > 0 ? pid : knownPID
            let visible = windows.map { rows in
                rows.filter {
                    ($0[kCGWindowOwnerPID as String] as? pid_t) == windowPID
                        && ($0[kCGWindowLayer as String] as? Int) == 0
                        && ($0[kCGWindowAlpha as String] as? Double ?? 1) > 0
                }.count
            }
            let policy = app.activationPolicy.rawValue
            let active = app.isActive
            let bundle = app.bundleURL?.path
            observation.record(ApplicationSample(pid: pid, policy: policy, active: active,
                terminated: terminated || app.isTerminated, visibleWindows: visible, bundle: bundle), identity: identity)
        }
    }
}

@MainActor
private func observeCommand(_ arguments: [String]) throws -> Int32 {
    try require(arguments.count >= 2, "Usage: observer REPORT_DIRECTORY COMMAND [ARGUMENT ...], or observer --self-test")
    let manager = FileManager.default
    let root = URL(fileURLWithPath: arguments[0], isDirectory: true)
    try require(!manager.fileExists(atPath: root.path), "Use a new report directory; existing evidence is never overwritten")
    try require(NSRunningApplication.runningApplications(withBundleIdentifier: "com.modernformatboost.self-test").isEmpty,
                "Another native self-test is running; observation would be ambiguous")
    try manager.createDirectory(at: root, withIntermediateDirectories: false,
                                attributes: [.posixPermissions: 0o700])
    let outputURL = root.appendingPathComponent("build.log")
    try Data().write(to: outputURL, options: .withoutOverwriting)
    let output = try FileHandle(forWritingTo: outputURL)
    let command = Process()
    command.executableURL = URL(fileURLWithPath: arguments[1])
    command.arguments = Array(arguments.dropFirst(2))
    command.standardOutput = output
    command.standardError = output
    let registry = ApplicationRegistry()
    let center = NSWorkspace.shared.notificationCenter
    let observers = [NSWorkspace.didLaunchApplicationNotification, NSWorkspace.didActivateApplicationNotification].map { name in
        let isActivation = name == NSWorkspace.didActivateApplicationNotification
        return center.addObserver(forName: name, object: nil, queue: .main) { notification in
            guard let app = notification.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication else { return }
            MainActor.assumeIsolated {
                guard let identity = registry.targetIdentity(of: app) else { return }
                if isActivation { registry.observation.activated(identity: identity) }
            }
        }
    }
    defer { for observer in observers { center.removeObserver(observer) } }
    try command.run()
    while command.isRunning {
        registry.sample()
        RunLoop.main.run(until: Date().addingTimeInterval(0.01))
    }
    command.waitUntilExit()
    // Deliver the final lifecycle change before reading cached AppKit properties.
    RunLoop.main.run(until: Date().addingTimeInterval(0.01))
    registry.sample()
    try output.synchronize()
    try output.close()
    let log = try String(contentsOf: outputURL, encoding: .utf8)
    let passed = command.terminationReason == .exit && command.terminationStatus == 0
        && registry.observation.passed && log.contains("native-host self-test passed")
    let report: [String: Any] = ["schema": 1, "command_exit": command.terminationStatus,
        "command_exit_reason": command.terminationReason == .exit ? "exit" : "signal",
        "self_test_processes": registry.observation.applications.keys.sorted().map {
            registry.observation.applications[$0]!.report
        }, "monitoring_errors": registry.observation.errors.sorted(), "passed": passed, "log": outputURL.path]
    let json = try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
    try json.write(to: root.appendingPathComponent("observation.json"), options: .withoutOverwriting)
    try FileHandle.standardOutput.write(contentsOf: json + Data([10]))
    return passed ? 0 : 1
}

do {
    if CommandLine.arguments.dropFirst() == ["--self-test"] {
        try runRegression()
    } else {
        exit(try observeCommand(Array(CommandLine.arguments.dropFirst())))
    }
} catch {
    try FileHandle.standardError.write(contentsOf: Data("background observer: \(error.localizedDescription)\n".utf8))
    exit(1)
}
