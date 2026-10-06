//! Every case in tests/cases must print exactly what the reference prints (tests/golden).
//! Regenerate the goldens with `node spec/report.js --golden`.
use std::fs;
use std::path::Path;

#[test]
fn reports_match_the_reference() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut failed = vec![];
    let mut n = 0;
    for e in fs::read_dir(root.join("cases")).unwrap() {
        let path = e.unwrap().path();
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let want = fs::read_to_string(root.join("golden").join(format!("{name}.out"))).unwrap();
        let got = naga::report::report(&fs::read_to_string(&path).unwrap());
        n += 1;
        if got != want {
            let line = got.lines().zip(want.lines()).position(|(a, b)| a != b).unwrap_or(got.lines().count().min(want.lines().count()));
            failed.push(format!(
                "{name}: line {}\n  want {}\n  got  {}",
                line + 1,
                want.lines().nth(line).unwrap_or("<end>"),
                got.lines().nth(line).unwrap_or("<end>")
            ));
        }
    }
    failed.sort();
    assert!(failed.is_empty(), "{}/{n} differ:\n{}", failed.len(), failed.join("\n"));
}
