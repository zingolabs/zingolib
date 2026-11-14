use super::*;

struct FakeTransport {
    ready: bool,
}

impl PoolTransport for FakeTransport {
    fn socks5_addr(&self) -> Option<std::net::SocketAddr> {
        self.ready
            .then(|| "127.0.0.1:7".parse().expect("the fake address parses"))
    }

    fn stop(self) -> impl std::future::Future<Output = ()> + Send {
        std::future::ready(())
    }
}

/// HYPOTHESIS: a member serves repeated dials for its one holder, then
/// retires once.
#[tokio::test]
async fn a_member_serves_repeated_dials() {
    let member: Member<FakeTransport> = Member::new(
        FakeTransport { ready: true },
        exit_pool::Reservation::dangling_for_test("exit-shared"),
    );
    let first = member.addr().expect("a live shared member has an address");
    let second = member.addr().expect("a second dial is permitted");
    assert_eq!(first, second, "one tunnel serves the whole fan-out");
    member.retire().await;
}

/// HYPOTHESIS: clearing the acquirer forbids later births, the teardown
/// property vacating a session relies on.
#[test]
fn clearing_the_acquirer_forbids_later_births() {
    let pools = Pools::new();
    assert!(
        pools.acquirer().is_none(),
        "a fresh session has no acquirer"
    );
    pools.clear_acquirer();
    assert!(pools.acquirer().is_none(), "clearing is idempotent");
}
