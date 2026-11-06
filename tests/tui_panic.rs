//! The TUI panic hook restores the terminal and then runs the hook that was
//! installed before it. Kept in its own test binary because panic hooks are
//! process-global.

use std::sync::atomic::{AtomicBool, Ordering};

static PREVIOUS_RAN: AtomicBool = AtomicBool::new(false);

#[test]
fn panic_hook_chains_to_the_previous_hook() {
    std::panic::set_hook(Box::new(|_| PREVIOUS_RAN.store(true, Ordering::SeqCst)));
    diffkit::tui::terminal::install_panic_hook();
    // Installing twice must not stack two restoring hooks.
    diffkit::tui::terminal::install_panic_hook();
    let result = std::panic::catch_unwind(|| panic!("simulated failure inside the TUI"));
    assert!(result.is_err());
    assert!(
        PREVIOUS_RAN.load(Ordering::SeqCst),
        "the original hook still reports the panic"
    );
}
