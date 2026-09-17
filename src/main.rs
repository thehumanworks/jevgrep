fn main() {
    std::process::exit(jevgrep::cli::run(std::env::args_os().skip(1).collect()));
}
