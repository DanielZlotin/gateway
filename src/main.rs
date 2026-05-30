use gateway::cli::{parse_args_from, Mode};

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mode = parse_args_from(std::env::args_os())?;
    let cfg = gateway::config::load()?;
    match mode {
        Mode::Bot => gateway::bot::run(cfg),
        Mode::Run(args) => {
            let output = gateway::run_mode::run(args, cfg)?;
            println!("{output}");
            Ok(())
        }
    }
}
