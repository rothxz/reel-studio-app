// Links the shared C++ engine core when built with `--features cpp-engine`.
// The core's source lives in the main (private) repo, so it is never copied
// here: point REEL_ENGINE_DIR at its modules/video-engine/cpp folder.
fn main() {
    println!("cargo:rerun-if-env-changed=REEL_ENGINE_DIR");
    println!("cargo:rerun-if-changed=engine-shim/reel_engine.cpp");
    if std::env::var_os("CARGO_FEATURE_CPP_ENGINE").is_none() {
        return;
    }
    let dir = std::env::var("REEL_ENGINE_DIR").expect(
        "the cpp-engine feature needs REEL_ENGINE_DIR=<reel-studio>/modules/video-engine/cpp",
    );
    let dir = std::path::PathBuf::from(dir);
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++17")
        .include(dir.join("include"))
        .file("engine-shim/reel_engine.cpp")
        .warnings(false);
    for entry in std::fs::read_dir(dir.join("src")).expect("REEL_ENGINE_DIR/src") {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "cpp") {
            println!("cargo:rerun-if-changed={}", path.display());
            build.file(path);
        }
    }
    build.compile("reel_engine");
}
