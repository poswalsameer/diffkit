mod app;
fn main() {
    std::process::exit(app::cli::run(std::env::args_os().skip(1).collect()));
}
