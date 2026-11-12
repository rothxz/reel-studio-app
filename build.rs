// Build-time inputs that live in the main (private) repo and are never
// copied here:
//
// - The shared C++ engine core, linked with `--features cpp-engine`: point
//   REEL_ENGINE_DIR at its modules/video-engine/cpp folder.
// - The app's public client config and the filter list, read from the app
//   checkout at REEL_APP_DIR: the asset host (tile previews on R2), the
//   catalog database's URL and publishable key (the served effects table),
//   the filter catalog + its preview manifest, and the UI dictionaries of
//   every language (src/i18n.rs). They are compiled into
//   the binary; without a checkout the app builds with none of them (an
//   exported REEL_ASSETS_URL / REEL_CATALOG_URL / REEL_CATALOG_KEY still
//   works, at build or at run time).
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=REEL_ENGINE_DIR");
    println!("cargo:rerun-if-env-changed=REEL_APP_DIR");
    println!("cargo:rerun-if-changed=engine-shim/reel_engine.cpp");
    app_inputs();
    release_inputs();
    static_ffmpeg();
    windows_resources();
    ort_dynamic();
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
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // M_PI and friends (glibc and Apple's libc define them anyway), and
        // the sources' UTF-8 comments read as UTF-8 whatever the code page,
        // and the standard lambda rules: a capture-less lambda may read a
        // local constexpr (C3493 under MSVC's legacy C++17 lambda parser).
        build
            .define("_USE_MATH_DEFINES", None)
            .flag("/utf-8")
            .flag("/Zc:lambda");
    }
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
        // The phone app's PostHog project (src/analytics.rs); none = off.
        ("REEL_POSTHOG_KEY", "EXPO_PUBLIC_POSTHOG_API_KEY"),
        ("REEL_POSTHOG_HOST", "EXPO_PUBLIC_POSTHOG_HOST"),
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

    // The shader-built reveals that double as transitions (src/transitions.rs):
    // their list (id, label, categories, length), each one's package key
    // (the package manifest's `animations` section, read there) and tile
    // animation (the preview manifest).
    let reveals = app
        .as_ref()
        .map(|a| a.join("src/features/preview/animationShaders.generated.ts"))
        .and_then(|f| watched_read(&f))
        .and_then(|ts| ts_rows(&ts))
        .unwrap_or_else(|| "[]".into());
    let manifest = data
        .as_ref()
        .map(|d| d.join("effectPackageManifest.json"))
        .and_then(|f| watched_read(&f))
        .unwrap_or_else(|| "{}".into());
    let reveal_previews = data
        .as_ref()
        .map(|d| d.join("clipAnimationPreviewManifest.json"))
        .and_then(|f| watched_read(&f))
        .unwrap_or_else(|| "{}".into());
    std::fs::write(out.join("reveals.json"), reveals).unwrap();
    std::fs::write(out.join("package_manifest.json"), manifest).unwrap();
    std::fs::write(out.join("reveal_previews.json"), reveal_previews).unwrap();

    // The UI dictionaries (src/i18n.rs): every language's strings as one
    // JSON object, `{ "km": { "settings.title": "…", … }, … }` ("{}"
    // without a checkout: the app is then English only).
    let locales = app.as_ref().map(|a| a.join("src/i18n/locales"));
    let mut json = String::from("{");
    for code in ["en", "km", "vi", "zh", "id"] {
        let Some(entries) = locales
            .as_ref()
            .and_then(|d| watched_read(&d.join(format!("{code}.ts"))))
            .and_then(|ts| ts_dictionary(&ts))
        else {
            continue;
        };
        if json.len() > 1 {
            json.push(',');
        }
        json.push_str(&format!("\"{code}\":{{"));
        for (i, (key, text)) in entries.iter().enumerate() {
            if i > 0 {
                json.push(',');
            }
            json.push_str(&format!("{}:{}", json_string(key), json_string(text)));
        }
        json.push('}');
    }
    json.push('}');
    std::fs::write(out.join("locales.json"), json).unwrap();
}

/// A dictionary file's object literal (`export const xx = { a: { b: "…" } }`)
/// flattened to dotted keys: `("a.b", "…")`. Keys are bare or quoted; values
/// are quoted strings, maybe joined with `+`. None if it doesn't parse.
fn ts_dictionary(ts: &str) -> Option<Vec<(String, String)>> {
    let s: Vec<char> = ts.chars().collect();
    let skip = |mut i: usize| -> usize {
        while i < s.len() {
            if s[i].is_whitespace() {
                i += 1;
            } else if s[i] == '/' && s.get(i + 1) == Some(&'/') {
                while i < s.len() && s[i] != '\n' {
                    i += 1;
                }
            } else if s[i] == '/' && s.get(i + 1) == Some(&'*') {
                i += 2;
                while i + 1 < s.len() && !(s[i] == '*' && s[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            } else {
                break;
            }
        }
        i
    };
    let string = |mut i: usize| -> Option<(String, usize)> {
        let quote = s[i];
        i += 1;
        let mut text = String::new();
        while *s.get(i)? != quote {
            if s[i] == '\\' {
                let c = *s.get(i + 1)?;
                text.push(match c {
                    'n' => '\n',
                    't' => '\t',
                    other => other,
                });
                i += 2;
            } else {
                text.push(s[i]);
                i += 1;
            }
        }
        Some((text, i + 1))
    };
    let start = ts.find("= {")?;
    let mut i = ts[..start].chars().count() + 3;
    let mut path: Vec<String> = Vec::new();
    let mut out = Vec::new();
    loop {
        i = skip(i);
        match *s.get(i)? {
            '}' => {
                i = skip(i + 1);
                if path.pop().is_none() {
                    return Some(out);
                }
                if s.get(i) == Some(&',') {
                    i += 1;
                }
                continue;
            }
            '"' | '\'' => {
                let (key, next) = string(i)?;
                path.push(key);
                i = next;
            }
            _ => {
                let begin = i;
                while i < s.len() && (s[i].is_alphanumeric() || s[i] == '_' || s[i] == '$') {
                    i += 1;
                }
                if i == begin {
                    return None;
                }
                path.push(s[begin..i].iter().collect());
            }
        }
        i = skip(i);
        if *s.get(i)? != ':' {
            return None;
        }
        i = skip(i + 1);
        if *s.get(i)? == '{' {
            i += 1;
            continue;
        }
        let mut text = String::new();
        loop {
            if !matches!(*s.get(i)?, '"' | '\'' | '`') {
                return None;
            }
            let (part, next) = string(i)?;
            text.push_str(&part);
            i = skip(next);
            if s.get(i) == Some(&'+') {
                i = skip(i + 1);
                continue;
            }
            break;
        }
        let key = path.pop()?;
        out.push((
            path.iter()
                .chain([&key])
                .cloned()
                .collect::<Vec<_>>()
                .join("."),
            text,
        ));
        if s.get(i) == Some(&',') {
            i += 1;
        }
    }
}

/// `text` as a JSON string literal.
fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// What a release build stamps in (the release workflow exports both):
/// REEL_VERSION, the version the app reports and the updater compares
/// (default: Cargo.toml's), and REEL_UPDATE_REPO, the public GitHub repo
/// whose releases it updates from.
fn release_inputs() {
    for (key, default) in [
        ("REEL_VERSION", std::env::var("CARGO_PKG_VERSION").unwrap()),
        ("REEL_UPDATE_REPO", "rothxz/reel-studio-app".to_string()),
    ] {
        println!("cargo:rerun-if-env-changed={key}");
        let value = std::env::var(key)
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or(default);
        println!("cargo:rustc-env={key}={value}");
    }
    // A release build (the workflow stamps REEL_VERSION): analytics sends
    // only from these, local builds just log (src/analytics.rs).
    let release = std::env::var("REEL_VERSION").is_ok_and(|v| !v.is_empty());
    println!(
        "cargo:rustc-env=REEL_RELEASE_BUILD={}",
        if release { "1" } else { "" }
    );
}

/// A static FFmpeg (FFMPEG_DIR from packaging/<os>/build-ffmpeg.sh, the
/// release build): ffmpeg-sys-next links its libraries, but not what they
/// need from the system, which FFmpeg lists in its pkg-config files
/// (frameworks, -lz, -lbz2, mfplat.lib…). A shared FFmpeg (Homebrew, a
/// distro's, development) needs nothing here.
fn static_ffmpeg() {
    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
    let Some(dir) = std::env::var_os("FFMPEG_DIR").map(PathBuf::from) else {
        return;
    };
    let lib = dir.join("lib");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let is_static = if os == "windows" {
        // build-ffmpeg.sh names the static libraries avcodec.lib; a shared
        // build has import libraries there and DLLs in bin/.
        lib.join("avcodec.lib").exists()
            && !std::fs::read_dir(dir.join("bin")).is_ok_and(|d| {
                d.flatten()
                    .any(|e| e.path().extension().is_some_and(|x| x == "dll"))
            })
    } else {
        lib.join("libavcodec.a").exists()
            && !lib.join("libavcodec.dylib").exists()
            && !lib.join("libavcodec.so").exists()
    };
    if !is_static {
        return;
    }
    const LIBS: [&str; 7] = [
        "avdevice",
        "avfilter",
        "avformat",
        "avcodec",
        "swresample",
        "swscale",
        "avutil",
    ];
    if os == "linux" {
        // GNU ld takes archives in order: FFmpeg's own, users first, then
        // the system libraries they need (the -l flags below). Linked here
        // so they come before those; ffmpeg-sys-next's later copies are
        // no-ops.
        println!("cargo:rustc-link-search=native={}", lib.display());
        for name in LIBS {
            if lib.join(format!("lib{name}.a")).exists() {
                println!("cargo:rustc-link-lib=static={name}");
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    for name in LIBS {
        let path = lib.join("pkgconfig").join(format!("lib{name}.pc"));
        let Some(text) = watched_read(&path) else {
            continue;
        };
        // A static-only FFmpeg lists them under `Libs:`, a shared one under
        // `Libs.private:`.
        let libs: String = text
            .lines()
            .filter_map(|l| l.strip_prefix("Libs:").or(l.strip_prefix("Libs.private:")))
            .collect::<Vec<_>>()
            .join(" ");
        let mut words = libs.split_whitespace();
        while let Some(word) = words.next() {
            let link = if word == "-framework" {
                words.next().map(|f| format!("framework={f}"))
            } else if let Some(name) = word.strip_prefix("-l").or_else(|| {
                word.strip_suffix(".lib")
                    .filter(|w| !w.contains(['/', '\\']))
            }) {
                // FFmpeg's own libraries come from ffmpeg-sys-next (by name:
                // Windows' avrt.lib is a system one).
                (!LIBS.contains(&name)).then(|| name.to_string())
            } else {
                None
            };
            if let Some(link) = link
                && seen.insert(link.clone())
            {
                println!("cargo:rustc-link-lib={link}");
            }
        }
    }
}

/// `cfg(ort_dynamic)`: targets that load ONNX Runtime at run time
/// (src/face_swap/runtime.rs) instead of linking it: Linux and Intel Macs,
/// the same split as ort's `load-dynamic` feature in Cargo.toml.
fn ort_dynamic() {
    println!("cargo:rustc-check-cfg=cfg(ort_dynamic)");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if os == "linux" || (os == "macos" && arch == "x86_64") {
        println!("cargo:rustc-cfg=ort_dynamic");
    }
}

/// Windows: the manifest (per-monitor DPI, which gpui expects; UTF-8),
/// the app icon and the version shown in Explorer and Task Manager, built
/// into the .exe. The icon is the 256 px app icon as a PNG inside an .ico.
fn windows_resources() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let png_path = root.join("docs/images/app-icon-rounded.png");
    let manifest = root.join("packaging/windows/reel-studio.manifest");
    println!("cargo:rerun-if-changed={}", png_path.display());
    println!("cargo:rerun-if-changed={}", manifest.display());
    let png = std::fs::read(&png_path).expect("docs/images/app-icon-rounded.png");
    // ICONDIR + one ICONDIRENTRY (0 = 256 px), then the PNG.
    let mut ico = vec![0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 1, 0, 32, 0];
    ico.extend((png.len() as u32).to_le_bytes());
    ico.extend(22u32.to_le_bytes());
    ico.extend(&png);
    let ico_path = out.join("reel-studio.ico");
    std::fs::write(&ico_path, ico).unwrap();

    let version = std::env::var("REEL_VERSION")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap());
    let mut parts: Vec<u32> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|p| p.parse().unwrap_or(0))
        .collect();
    parts.resize(4, 0);
    let numeric = parts
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    // rc.exe takes forward slashes, and backslashes doubled in strings.
    let path = |p: &Path| p.display().to_string().replace('\\', "/");
    let rc = format!(
        r#"#include <winver.h>
1 24 "{manifest}"
1 ICON "{ico}"
VS_VERSION_INFO VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS VOS_NT_WINDOWS32
FILETYPE VFT_APP
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "Reel Studio"
      VALUE "FileDescription", "Reel Studio"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "reel-studio"
      VALUE "OriginalFilename", "reel-studio.exe"
      VALUE "ProductName", "Reel Studio"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        manifest = path(&manifest),
        ico = path(&ico_path),
    );
    let rc_path = out.join("reel-studio.rc");
    std::fs::write(&rc_path, rc).unwrap();
    #[cfg(windows)]
    embed_resource::compile(&rc_path, embed_resource::NONE)
        .manifest_required()
        .unwrap();
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
/// A generated TS array of flat object literals (`  key: value,` one per
/// line, unquoted keys, trailing commas) as JSON.
fn ts_rows(ts: &str) -> Option<String> {
    let start = ts.find("= [")? + 2;
    let end = ts[start..].find("\n];")? + start;
    let mut out = String::from("[");
    for line in ts[start + 1..end].lines() {
        let t = line.trim();
        let quoted = match t.split_once(": ") {
            Some((k, v))
                if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') =>
            {
                format!("\"{k}\": {v}")
            }
            _ => t.to_string(),
        };
        out.push_str(&quoted);
        out.push('\n');
    }
    out.push(']');
    // Trailing commas before a closing brace / bracket.
    let mut json = String::with_capacity(out.len());
    let chars: Vec<char> = out.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == ',' {
            let next = chars[i + 1..].iter().find(|c| !c.is_whitespace());
            if matches!(next, Some('}') | Some(']')) {
                continue;
            }
        }
        json.push(c);
    }
    Some(json)
}

fn json_array(ts: &str) -> Option<String> {
    let start = ts.find("= [")? + 2;
    let end = ts[start..].find("\n];")? + start;
    let rows = ts[start + 1..end].trim_end().trim_end_matches(',');
    Some(format!("[{rows}\n]"))
}
