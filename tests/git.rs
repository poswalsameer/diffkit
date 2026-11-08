//! Repository inspection (`diffkit::git`): discovery, status, branches,
//! upstreams, ahead/behind, operations in progress, history, errors, and the
//! read-only and offline guarantees, against real repositories and fixtures.

mod support;

mod git {
    mod ahead_behind;
    mod branch_headers;
    mod branches;
    mod commands;
    mod conflicts;
    mod discovery;
    mod error_messages;
    mod errors;
    mod filenames;
    mod history;
    mod log_parsing;
    mod operation_files;
    mod operations;
    mod read_only;
    mod renames;
    mod states;
    mod status;
    mod status_records;
    mod submodules;
    mod symlinks;
    mod tracking;
    mod upstream;
}
