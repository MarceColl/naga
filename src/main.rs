use std::process::ExitCode;

const USAGE: &str = "usage: naga run <file.nag>\n       naga serve <file.nag> [--port 4170]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let res = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [_, "run", file] => read(file).map(|src| print!("{}", naga::report::report(&src))),
        [_, "serve", file] => serve(file, 4170),
        [_, "serve", file, "--port", port] => port.parse().map_err(|_| format!("naga: bad port {port}")).and_then(|port| serve(file, port)),
        _ => Err(USAGE.into()),
    };
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn read(file: &str) -> Result<String, String> {
    std::fs::read_to_string(file).map_err(|e| format!("naga: {file}: {e}"))
}

/// The database sits beside the source: `ledger.nag` keeps its tables in `ledger.db`.
fn serve(file: &str, port: u16) -> Result<(), String> {
    let db = std::path::Path::new(file).with_extension("db");
    let router = naga::serve::start(read(file)?, db.clone())?;
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    rt.block_on(async {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.map_err(|e| format!("naga: port {port}: {e}"))?;
        println!("naga: {file} on http://127.0.0.1:{port}, tables in {}", db.display());
        axum::serve(listener, router).await.map_err(|e| e.to_string())
    })
}
