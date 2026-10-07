use std::process::ExitCode;

const USAGE: &str = "usage: naga init                    a new project here: app.nag
       naga start [file.nag] [--port N]  the editor, serving the program as you change it
       naga edit <file.nag> [--shot out.png …]  the editor alone
       naga serve <file.nag> [--port N]  the server alone
       naga run <file.nag>               run main, check, and run the runs";

const STARTER: &str = r#"-- A naga program. `naga start` opens it and serves it on :4170 while you change it.
-- Routes hand their word the path params and query at the top level and the JSON body in
-- `body`; every route word leaves {status, json}.

table notes :: {title: Text, done: Bool}

seed notes = [{title: "Try naga", done: false},
              {title: "Change a word while it runs", done: false}]

route GET  /notes                   = list-notes
route GET  /notes/:id:Int           = get-note
route POST /notes {title: Text}     = add-note

reply = .{status json}

list-notes = notes.all +{status: 200, json: notes} reply

get-note = notes.get found reply

found {-missing} = +{status: 404, json: "no such note"}
found {note}     = +{status: 200, json: note}

add-note = +{note: {title: body.title, done: false}} notes.insert
           +{status: 201, json: note} reply

run "list the notes"
  GET /notes → 200

run "add one, then read it back"
  POST /notes {title: "Ship it"} → 201 {id: 3, title: "Ship it", done: false}
  GET /notes/3 → 200 {id: 3, title: "Ship it", done: false}
"#;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    let port = |rest: &[&str], default: u16| match rest {
        ["--port", p, ..] => p.parse().map_err(|_| format!("naga: bad port {p}")),
        _ => Ok(default),
    };
    let res = match a.as_slice() {
        ["init"] => init(),
        ["start"] => port(&[], 4170).and_then(|p| naga::editor::start("app.nag", p)),
        ["start", "--port", ..] => port(&a[1..], 4170).and_then(|p| naga::editor::start("app.nag", p)),
        ["start", file, rest @ ..] => port(rest, 4170).and_then(|p| naga::editor::start(file, p)),
        ["edit", file, rest @ ..] => naga::editor::edit(file, &args[2..2 + rest.len()]),
        ["run", file] => read(file).map(|src| print!("{}", naga::report::report(&src))),
        ["serve", file, rest @ ..] => port(rest, 4170).and_then(|p| serve(file, p)),
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

/// A starter program and a .gitignore for its database, refusing to overwrite either.
fn init() -> Result<(), String> {
    for (file, text) in [("app.nag", STARTER), (".gitignore", "*.db\n")] {
        if std::path::Path::new(file).exists() {
            return Err(format!("naga: {file} is already here"));
        }
        std::fs::write(file, text).map_err(|e| format!("naga: {file}: {e}"))?;
    }
    println!("naga: wrote app.nag — naga start opens it");
    Ok(())
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
