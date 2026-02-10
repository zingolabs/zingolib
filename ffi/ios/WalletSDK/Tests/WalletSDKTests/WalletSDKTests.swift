import XCTest

@testable import WalletSDK

final class WalletSDKTests: XCTestCase {
    func testCreateWalletAndGetBalance() throws {
        let wallet = try Wallet(onEvent: { event in
            print("EVENT:", event)
        })

        let bal = try wallet.balance()
        print("BAL:", bal)

        try wallet.startSync()
        RunLoop.current.run(until: Date().addingTimeInterval(1.0))
        try wallet.pauseSync()
        try wallet.shutdown()
    }
}
