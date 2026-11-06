// Build-time inputs that live in the main (private) repo and are never
// copied here:
//
// - The shared C++ engine core, linked with `--features cpp-engine`: point
//   REEL_ENGINE_DIR at its modules/video-engine/cpp folder.
// - The app's public client config and the filter list, read from the app
//   checkout at REEL_APP_DIR: the asset host (tile previews on R2), the
//   catalog database's URL and publishable key (the served effects table),
//   and the filter catalog + its preview manifest. They are compiled into
//   the binary; without a checkout the app builds with none of them (an
//   exported REEL_ASSETS_URL / REEL_CATALOG_URL / REEL_CATALOG_KEY still
//   works, at build or at run time).
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=REEL_ENGINE_DIR");
    println!("cargo:rerun-if-env-changed=REEL_APP_DIR");
    println!("cargo:rerun-if-changed=engine-shim/reel_engine.cpp");
    app_inputs();
    if std::env::var_os("CARGO_FEATURE_CPP_ENGINE").is_none() {
        return;
    }
    let dir = std::env::var("REEL_ENGINE_DIR").expect(
        "the cpp-engine feature needs REEL_ENGINE_DIR=<reel-studio>/modules/video-engine/cpp",
    );
    let dir = PathBuf::from(dir);
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

/// Config + filter catalog from the app checkout (see the top comment).
fn app_inputs() {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let app = std::env::var_os("REEL_APP_DIR").map(PathBuf::from);

    // Public client config: `<app>/.env`, an exported variable wins.
    let env_file = app.as_ref().map(|a| a.join(".env"));
    if let Some(f) = &env_file {
        println!("cargo:rerun-if-changed={}", f.display());
    }
    let dotenv = env_file
        .and_then(|f| std::fs::read_to_string(f).ok())
        .unwrap_or_default();
    for (ours, theirs) in [
        ("REEL_ASSETS_URL", "EXPO_PUBLIC_ASSETS_URL"),
        ("REEL_CATALOG_URL", "EXPO_PUBLIC_SUPABASE_URL"),
        ("REEL_CATALOG_KEY", "EXPO_PUBLIC_SUPABASE_KEY"),
    ] {
        println!("cargo:rerun-if-env-changed={ours}");
        let value = std::env::var(ours)
            .ok()
            .or_else(|| dotenv_value(&dotenv, theirs))
            .unwrap_or_default();
        println!("cargo:rustc-env={ours}={value}");
    }

    // The filter list (names, tabs, built-in looks) and each filter's tile
    // key, written to OUT_DIR as JSON for `include_str!` ("[]" / "{}"
    // without a checkout).
    let data = app.as_ref().map(|a| a.join("src/data"));
    let catalog = data
        .as_ref()
        .map(|d| d.join("filterCatalog.generated.ts"))
        .and_then(|f| watched_read(&f))
        .and_then(|ts| json_array(&ts))
        .unwrap_or_else(|| "[]".into());
    let previews = data
        .as_ref()
        .map(|d| d.join("filterPreviewManifest.json"))
        .and_then(|f| watched_read(&f))
        .unwrap_or_else(|| "{}".into());
    // Each packaged filter's shader package key (the manifest's `filters`
    // section); effects carry theirs in the served catalog instead.
    let packages = data
        .as_ref()
        .map(|d| d.join("effectPackageManifest.json"))
        .and_then(|f| watched_read(&f))
        .and_then(|text| {
            let start = text.find("\"filters\"")?;
            let open = start + text[start..].find('{')?;
            let close = open + text[open..].find('}')?;
            Some(text[open..=close].to_string())
        })
        .unwrap_or_else(|| "{}".into());
    std::fs::write(out.join("filter_catalog.json"), catalog).unwrap();
    std::fs::write(out.join("filter_previews.json"), previews).unwrap();
    std::fs::write(out.join("filter_packages.json"), packages).unwrap();
}

fn watched_read(path: &Path) -> Option<String> {
    println!("cargo:rerun-if-changed={}", path.display());
    std::fs::read_to_string(path).ok()
}

/// `KEY=value` from a dotenv file (quotes stripped).
fn dotenv_value(text: &str, key: &str) -> Option<String> {
    text.lines()
        .find_map(|line| {
            let (k, v) = line.trim().split_once('=')?;
            (k.trim() == key).then(|| v.trim().trim_matches('"').trim_matches('\'').to_string())
        })
        .filter(|v| !v.is_empty())
}

/// The first `[ … ]` literal of a generated TS file: JSON rows, maybe with
/// a trailing comma after the last (which JSON doesn't allow).
fn json_array(ts: &str) -> Option<String> {
    let start = ts.find("= [")? + 2;
    let end = ts[start..].find("\n];")? + start;
    let rows = ts[start + 1..end].trim_end().trim_end_matches(',');
    Some(format!("[{rows}\n]"))
}
