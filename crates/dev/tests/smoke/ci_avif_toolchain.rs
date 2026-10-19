const WORKFLOW: &str = include_str!("../../../../.github/workflows/ci-quality.yml");
const ACTION: &str = include_str!("../../../../.github/actions/setup-avif/action.yml");

#[test]
fn every_avif_test_job_installs_the_same_full_sample_decoder() {
    for (job, next_job, test_command) in [
        (
            "package-quality",
            Some("health-check"),
            "cargo test --locked -p",
        ),
        (
            "health-check",
            Some("security-audit"),
            "cargo run --locked -p dev --bin check_all",
        ),
        (
            "deep-audit",
            None,
            "cargo run --locked -p dev --bin check_all",
        ),
    ] {
        let section = WORKFLOW
            .split_once(&format!("\n  {job}:\n"))
            .expect("CI job must exist")
            .1;
        let section = next_job.map_or(section, |next| {
            section
                .split_once(&format!("\n  {next}:\n"))
                .expect("next CI job must exist")
                .0
        });
        assert_eq!(
            section
                .matches("uses: ./.github/actions/setup-avif")
                .count(),
            1,
            "{job} must not rely on the distro AVIF decoder"
        );
        let installer = section.find("install_media_dependencies\"").unwrap();
        let pinned = section.find("uses: ./.github/actions/setup-avif").unwrap();
        assert!(
            installer < pinned,
            "{job} must pin AVIF after the bootstrap"
        );
        assert!(
            pinned < section.find(test_command).unwrap(),
            "{job} must pin AVIF before testing"
        );
        assert!(
            section.rfind(">> \"$GITHUB_PATH\"").unwrap() < pinned,
            "{job} must not put another media tool directory ahead of pinned AVIF"
        );
    }
    assert!(WORKFLOW.contains("find .github/workflows .github/actions"));
}

#[test]
fn avif_setup_checks_integrity_and_decoder_capability_before_tests() {
    assert!(ACTION.contains("LIBAVIF_VERSION: 1.4.2"));
    assert!(ACTION.contains("faf58a670ffbfdc0e3559e6d37592cff277c447dd39453f1cd1d7d7f5a20b8ef"));
    let verify = ACTION.find("sha256sum --check -").unwrap();
    let extract = ACTION.find("unzip -q").unwrap();
    let execute = ACTION.find("\"$tool\" --version").unwrap();
    assert!(verify < extract && extract < execute);
    assert!(ACTION.contains("test \"$(command -v \"$tool\")\" = \"$tools_root/bin/$tool\""));
    assert!(ACTION.contains("grep -q -- '--sato'"));
    assert!(ACTION.contains("grep -q -- '--depth'"));
    assert!(ACTION.contains("echo \"$tools_root/bin\" >> \"$GITHUB_PATH\""));
}
