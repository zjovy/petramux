fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| petramux_lib::cli::is_cli(a)) {
        std::process::exit(petramux_lib::cli::run(&args));
    }
    petramux_lib::run_app();
}
