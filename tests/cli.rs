//! The command line (`diffkit::cli` and the `diffkit` binary): argument
//! parsing and the plain-text commands.

mod support;

mod cli {
    mod args;
    mod diff_stats;
    mod inspect;
    mod inspect_report;
}
