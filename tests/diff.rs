//! The diff engine (`diffkit::diff`): discovery for every comparison,
//! lazy loading, patch parsing, hunks, alignment, and special files.

mod support;

mod diff {
    mod alignment;
    mod binary;
    mod commands;
    mod comparisons;
    mod discovery;
    mod errors;
    mod hunk;
    mod lazy_loading;
    mod parser;
    mod paths;
    mod read_only;
    mod special_files;
    mod untracked;
}
