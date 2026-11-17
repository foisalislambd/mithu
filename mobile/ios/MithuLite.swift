import UIKit
import WebKit

/// System web view for Mithu Lite. The Servo edition would use the same
/// documents from the mithu-mobile crate once a Servo iOS embedding exists.
/// The bundle id in Info.plist is org.mithu.lite.
final class ViewController: UIViewController, WKNavigationDelegate {
    private let web = WKWebView(frame: .zero)

    override func loadView() {
        web.navigationDelegate = self
        view = web
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        if let url = Bundle.main.url(forResource: "newtab", withExtension: "html") {
            web.loadFileURL(url, allowingReadAccessTo: url.deletingLastPathComponent())
        }
    }
}
