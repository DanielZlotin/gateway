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
        Mode::Logs(lines) => {
            let output = gateway::logs::read_gateway_logs(&gateway::config::current_env(), lines)?;
            println!("{output}");
            Ok(())
        }
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
        Mode::Version => {
            println!("gateway {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }
}
