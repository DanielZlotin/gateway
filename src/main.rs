use gateway::cli::{parse_args_from, Mode};

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mode = parse_args_from(std::env::args_os())?;
    match mode {
        Mode::Bot => gateway::bot::run(gateway::config::load()?),
        Mode::Paths => {
            println!("{}", gateway::config::load()?.paths_report());
            Ok(())
        }
        Mode::Run(args) => {
            let output = gateway::run_mode::run(args, gateway::config::load()?)?;
            println!("{output}");
            Ok(())
        }
        Mode::Uninstall => {
            println!("{}", gateway::launchd::uninstall()?);
            Ok(())
        }
    }
}
