import Foundation
import WalletSDK

final class Printer {
    static func run() throws {
        let wallet = try WalletSDK.Wallet(onEvent: { e in
            print("EVENT:", e)
        })

        let bal = try wallet.balance()
        print("BALANCE:", bal)

        try wallet.startSync()

        RunLoop.current.run(until: Date().addingTimeInterval(2.0))

        try wallet.pauseSync()
        try wallet.shutdown()
    }
}

do { try Printer.run() } catch { print("ERROR:", error) }
