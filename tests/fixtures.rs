//! Runs quoag on every project in `tests/fixtures` and compares its output byte for byte with the documents
//! Quarkus wrote into the project's `openapi` directory (`mvn compile quarkus:build`, profile `prod`).

use std::path::Path;
use std::process::Command;

#[test]
fn matches_the_documents_quarkus_wrote() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut projects: Vec<_> = std::fs::read_dir(&fixtures)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    projects.sort();
    assert!(
        !projects.is_empty(),
        "no fixtures in {}",
        fixtures.display()
    );
    for project in projects {
        let name = project.file_name().unwrap().to_string_lossy().to_string();
        let out = std::env::temp_dir().join(format!("quoag-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out);
        let status = Command::new(env!("CARGO_BIN_EXE_quoag"))
            .args(["--profile", "prod", "--project"])
            .arg(&project)
            .arg("--out")
            .arg(&out)
            .status()
            .unwrap();
        assert!(status.success(), "{name}: quoag failed");
        for file in ["openapi.json", "openapi.yaml"] {
            let expected = std::fs::read_to_string(project.join("openapi").join(file)).unwrap();
            let actual = std::fs::read_to_string(out.join(file)).unwrap();
            assert!(
                actual == expected,
                "{name}/{file} differs from the Quarkus output"
            );
        }
        std::fs::remove_dir_all(&out).unwrap();
    }
}
