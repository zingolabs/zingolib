use super::*;

const GUARDED_PANIC: &str = "a guarded call panics";
const FOREIGN_PANIC: &str = "another thread panics";

#[test]
fn a_guarded_panic_reports_its_message_and_location() {
    let outcome: Result<(), ZingolibError> = with_panic_guard(|| {
        let panics = true;
        if panics {
            panic!("{GUARDED_PANIC}");
        }
        Ok(())
    });
    let Err(ZingolibError::Panic(text)) = outcome else {
        panic!("a panic under the guard must come back as the typed Panic error");
    };
    assert!(text.contains(GUARDED_PANIC), "{text}");
    assert!(text.contains("panic_hook_tests.rs"), "{text}");
}

/// - Spawns a thread that panics, so the hook runs on that thread.
#[test]
fn a_panic_on_another_thread_leaves_this_threads_report_untouched() {
    install_panic_hook_once();
    let _cleared = take_last_panic();
    let joined = std::thread::spawn(|| {
        let panics = true;
        if panics {
            panic!("{FOREIGN_PANIC}");
        }
    })
    .join();
    assert!(joined.is_err(), "the spawned thread must end in its panic");
    let report = take_last_panic();
    assert!(
        report.msg.is_empty(),
        "the other thread's panic reached this thread's report: {}",
        report.msg
    );
}
