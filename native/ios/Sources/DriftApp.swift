import SwiftUI
import WebKit
import UIKit

@main
struct DriftApp: App {
    @StateObject private var engine = NativeEngine()
    var body: some Scene {
        WindowGroup {
            Group {
                if let url = engine.url { DriftWebView(url: url, invitation: engine.invitation) }
                else { VStack(spacing: 18) { Text("Drift").font(.largeTitle); Text(engine.error ?? "Starting transfer engine…").multilineTextAlignment(.center) }.padding() }
            }.background(Color.black.ignoresSafeArea()).preferredColorScheme(.dark).onOpenURL { url in
                if url.scheme == "drift", url.host == "pair", url.absoluteString.utf8.count <= 8192 { engine.invitation = url.absoluteString }
            }
        }
    }
}

final class NativeEngine: ObservableObject {
    @Published var url: URL?
    @Published var error: String?
    @Published var invitation: String?
    private var termination: NSObjectProtocol?
    init() {
        do {
            var root = try FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true).appendingPathComponent("Drift", isDirectory: true)
            try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
            var values = URLResourceValues(); values.isExcludedFromBackup = true; try root.setResourceValues(values)
            let response = root.path.withCString { directory in UIDevice.current.model.withCString { name in drift_start(directory, name) } }
            let result = try nativeResult(response)
            guard let string = result["url"] as? String, let url = URL(string: string), url.scheme == "http", url.host == "127.0.0.1", url.port != nil else { throw NativeError.message("Invalid engine address") }
            self.url = url
            termination = NotificationCenter.default.addObserver(forName: UIApplication.willTerminateNotification, object: nil, queue: nil) { _ in drift_stop() }
        } catch { self.error = error.localizedDescription }
    }
    deinit { if let termination { NotificationCenter.default.removeObserver(termination) }; drift_stop() }
}

enum NativeError: LocalizedError {
    case message(String)
    var errorDescription: String? { switch self { case let .message(value): return value } }
}
func nativeResult(_ reply: UnsafeMutablePointer<CChar>?) throws -> [String: Any] {
    guard let reply else { throw NativeError.message("Native engine returned no response") }
    defer { drift_string_free(reply) }
    let bytes = Data(String(cString: reply).utf8)
    guard let result = try JSONSerialization.jsonObject(with: bytes) as? [String: Any] else { throw NativeError.message("Invalid engine response") }
    guard result["ok"] as? Bool == true else { throw NativeError.message(result["error"] as? String ?? "Engine request failed") }
    return result["result"] as? [String: Any] ?? [:]
}

struct DriftWebView: UIViewRepresentable {
    let url: URL
    let invitation: String?
    func makeCoordinator() -> Coordinator { Coordinator(url: url) }
    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.userContentController.add(context.coordinator, name: "driftSave")
        configuration.userContentController.addUserScript(WKUserScript(source: "window.driftMobile={saveReceived:(id,index)=>window.webkit.messageHandlers.driftSave.postMessage({id,index})};", injectionTime: .atDocumentStart, forMainFrameOnly: true))
        let view = WKWebView(frame: .zero, configuration: configuration)
        context.coordinator.view = view
        view.navigationDelegate = context.coordinator
        view.isOpaque = false
        view.backgroundColor = .black
        view.scrollView.backgroundColor = .black
        view.load(URLRequest(url: url))
        return view
    }
    func updateUIView(_ view: WKWebView, context: Context) { context.coordinator.invitation = invitation; context.coordinator.deliverInvitation() }
    final class Coordinator: NSObject, WKNavigationDelegate, WKScriptMessageHandler, UIDocumentPickerDelegate {
        weak var view: WKWebView?
        let origin: URL
        var invitation: String?
        var delivered: String?
        var loaded = false
        init(url: URL) { origin = url }
        func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction, decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
            let url = action.request.url
            let allowed = url?.scheme == "http" && url?.host == "127.0.0.1" && url?.port == origin.port
            decisionHandler(allowed ? .allow : .cancel)
        }
        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) { loaded = true; deliverInvitation() }
        func deliverInvitation() {
            guard loaded, let invitation, invitation != delivered, invitation.hasPrefix("drift://pair?"), invitation.utf8.count <= 8192 else { return }
            guard let data = try? JSONSerialization.data(withJSONObject: [invitation]), let array = String(data: data, encoding: .utf8) else { return }
            delivered = invitation
            view?.evaluateJavaScript("window.__driftInvitation=\(array)[0];window.dispatchEvent(new Event('drift-invitation'));")
        }
        func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
            guard message.frameInfo.isMainFrame, message.frameInfo.request.url?.host == "127.0.0.1", message.frameInfo.request.url?.port == origin.port,
                  let body = message.body as? [String: Any], let id = body["id"] as? String, let index = body["index"] as? Int, index >= 0, index < 10000 else { return }
            do {
                let result = try id.withCString { pointer in try nativeResult(drift_export_path(pointer, UInt32(index))) }
                guard let path = result["path"] as? String else { throw NativeError.message("Received file unavailable") }
                let picker = UIDocumentPickerViewController(forExporting: [URL(fileURLWithPath: path)], asCopy: true)
                picker.delegate = self
                guard let scene = UIApplication.shared.connectedScenes.first as? UIWindowScene, var presenter = scene.windows.first(where: { $0.isKeyWindow })?.rootViewController else { throw NativeError.message("Save sheet is unavailable") }
                while let next = presenter.presentedViewController { presenter = next }
                presenter.present(picker, animated: true)
            } catch { saveResult(ok: false, error: error.localizedDescription) }
        }
        func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) { saveResult(ok: true) }
        private func saveResult(ok: Bool, error: String = "") {
            guard let data = try? JSONSerialization.data(withJSONObject: ["ok": ok, "error": error]), let json = String(data: data, encoding: .utf8) else { return }
            view?.evaluateJavaScript("window.dispatchEvent(new CustomEvent('drift-save-result',{detail:\(json)}));")
        }
    }
}
