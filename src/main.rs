use gateway::cli::{parse_cli_from, CliAction, Mode};

fn main() {
    if let Err(err) = run() {
        gateway::logs::error(format_args!("{err}"));
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mode = match parse_cli_from(std::env::args_os())? {
        CliAction::Execute(mode) => mode,
        CliAction::Help(help) => {
            print!("{help}");
            return Ok(());
        }
    };
    match mode {
        Mode::Bot => gateway::bot::run(gateway::config::load()?),
        Mode::Heartbeat => print_output(gateway::heartbeat::run(gateway::config::load()?)),
        Mode::List(args) => {
            print_output(gateway::cli_commands::list(args, gateway::config::load()?))
        }
        Mode::Logs(lines) => print_output(gateway::logs::read_gateway_logs(
            &gateway::config::current_env(),
            lines,
        )),
        Mode::Run(args) => print_output(gateway::run_mode::run(args, gateway::config::load()?)),
        Mode::Status(args) => print_output(gateway::cli_commands::status(
            args,
            gateway::config::load()?,
        )),
        Mode::Update => print_output(gateway::cli_commands::update(gateway::config::load()?)),
        Mode::Uninstall => print_output(gateway::launchd::uninstall()),
        Mode::Version => print_output(Ok(format!("gateway {}", env!("CARGO_PKG_VERSION")))),
    }
}

fn print_output(output: Result<String, String>) -> Result<(), String> {
    println!("{}", output?);
    Ok(())
}
