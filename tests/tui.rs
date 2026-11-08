//! The interactive interface (`diffkit::tui`): application state, keys,
//! layout, rendering, history, synchronization results, and real
//! pseudo-terminal sessions.

mod support;

mod tui {
    mod app;
    mod history;
    mod keymap;
    mod layout;
    mod render;
    mod states;
    mod sync;
    mod terminal;
}
