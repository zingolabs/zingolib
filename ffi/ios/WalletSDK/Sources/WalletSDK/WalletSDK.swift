import Foundation

public enum WalletSDKError: Error {
    case underlying(String)
}

public struct Balance {
    public let confirmed: String
    public let total: String
}

public enum WalletEventPublic {
    case engineReady
    case syncStarted
    case syncProgress(walletHeight: Int, networkHeight: Int, percent: Double)
    case syncPaused
    case syncFinished
    case balanceChanged(Balance)
    case newTransaction(txid: String)
    case error(code: String, message: String)
}

public final class Wallet: WalletListener {
    private let engine: WalletEngine
    private var handler: ((WalletEventPublic) -> Void)?

    public init(onEvent: ((WalletEventPublic) -> Void)? = nil) throws {
        self.engine = try WalletEngine()
        self.handler = onEvent
        try self.engine.setListener(listener: self)
    }

    public func setEventHandler(_ h: ((WalletEventPublic) -> Void)?) {
        self.handler = h
    }

    // UniFFI callback
    public func onEvent(event: WalletEvent) {
        handler?(mapEvent(event))
    }

    public func startSync() throws {
        try engine.startSync()
    }

    public func pauseSync() throws {
        try engine.pauseSync()
    }

    public func shutdown() throws {
        try engine.clearListener()
        try engine.shutdown()
    }

    public func balance() throws -> Balance {
        let snap = try engine.getBalanceSnapshot()
        return Balance(confirmed: snap.confirmed, total: snap.total)
    }

    private func mapEvent(_ e: WalletEvent) -> WalletEventPublic {
        switch e {
        case .engineReady:
            return .engineReady
        case .syncStarted:
            return .syncStarted
        case .syncProgress(walletHeight: let h, networkHeight: let nh, percent: let p):
            return .syncProgress(walletHeight: Int(h), networkHeight: Int(nh), percent: Double(p))
        case .syncPaused:
            return .syncPaused
        case .syncFinished:
            return .syncFinished
        case .balanceChanged(snap: let s):
            return .balanceChanged(Balance(confirmed: s.confirmed, total: s.total))
        case .newTransaction(txid: let t):
            return .newTransaction(txid: t)
        case .error(code: let c, message: let m):
            return .error(code: c, message: m)
        }
    }
}
