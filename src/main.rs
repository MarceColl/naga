use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.as_slice() {
        [_, cmd, file] if cmd == "run" => match std::fs::read_to_string(file) {
            Ok(src) => {
                print!("{}", naga::report::report(&src));
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("naga: {file}: {e}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("usage: naga run <file.nag>");
            ExitCode::FAILURE
        }
    }
}
