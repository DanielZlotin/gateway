use gateway::cli::{parse_args_from, Mode};

fn main() {
    match parse_args_from(std::env::args_os()) {
        Ok(Mode::Bot) => {
            eprintln!("gateway bot is not wired yet");
            std::process::exit(2);
        }
        Ok(Mode::Run(_args)) => {
            eprintln!("gateway run is not wired yet");
            std::process::exit(2);
        }
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    }
}
