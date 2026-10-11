fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| roost_lib::cli::is_cli(a)) {
        std::process::exit(roost_lib::cli::run(&args));
    }
    roost_lib::run_app();
}
