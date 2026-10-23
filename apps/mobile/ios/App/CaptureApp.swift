import SwiftUI

@main
struct CaptureApp: App {
    @StateObject private var tunnel = TunnelController()

    var body: some Scene {
        WindowGroup {
            ContentView().environmentObject(tunnel)
        }
    }
}
