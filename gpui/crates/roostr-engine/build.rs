//! Builds the Odin engine (glonOdin `native/`) as a static library for the
//! target, with the flags `glonOdin/build-xcframework.mjs` uses, and records
//! the same source fingerprint that script writes to `glon-core.json`.
//!
//! The browser build links nothing: it loads the site's `engine.wasm` at run
//! time (see `src/web.rs`).
//!
//! | Variable | Default | Meaning |
//! |---|---|---|
//! | `GLON_ODIN_DIR` | `<RoostrWebsite>/../glonOdin` | Engine sources |
//! | `ODIN` | `odin` | Compiler |
//! | `MACOSX_DEPLOYMENT_TARGET` | `14.0` | macOS minimum (as the xcframework) |
//! | `IPHONEOS_DEPLOYMENT_TARGET` | `17.0` | iOS minimum (as the xcframework) |

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

/// Exports the native ABI must provide (build-xcframework.mjs `exports`).
const EXPORTS: [&str; 8] = [
    "core_abi_version",
    "core_init",
    "core_reserve",
    "core_reserve_blob",
    "core_execute",
    "core_response_pointer",
    "core_response_length",
    "core_reset",
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    for var in [
        "GLON_ODIN_DIR",
        "ODIN",
        "MACOSX_DEPLOYMENT_TARGET",
        "IPHONEOS_DEPLOYMENT_TARGET",
    ] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let root = match env::var_os("GLON_ODIN_DIR") {
        Some(dir) => PathBuf::from(dir),
        // crates/roostr-engine → gpui → RoostrWebsite, whose sibling is glonOdin.
        None => manifest_dir.join("../../../../glonOdin"),
    };
    let found = root.canonicalize();
    if env::var("CARGO_CFG_TARGET_FAMILY")
        .is_ok_and(|family| family.split(',').any(|f| f == "wasm"))
    {
        // Nothing to link; the browser smoke test still reads the fixtures.
        if let Ok(root) = found {
            println!("cargo:rustc-env=GLON_ODIN_ROOT={}", root.display());
        }
        return;
    }
    let root = found.unwrap_or_else(|_| {
        panic!(
            "roostr-engine: glonOdin sources not found at {}. Clone glonOdin beside RoostrWebsite or set GLON_ODIN_DIR.",
            root.display()
        )
    });

    let (fingerprint, source_files) = source_fingerprint(&root);
    let compiler = env::var("ODIN").unwrap_or_else(|_| "odin".into());
    let version = odin_version(&compiler);

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let library = out_dir.join("libglon.a");
    let mut args = vec![
        "build".to_string(),
        root.join("native").display().to_string(),
        "-build-mode:static".into(),
    ];
    args.extend(target_args());
    args.extend([
        "-o:speed".into(),
        "-no-entry-point".into(),
        format!("-out:{}", library.display()),
    ]);
    let status = Command::new(&compiler)
        .args(&args)
        .status()
        .unwrap_or_else(|error| missing_compiler(&compiler, &error));
    assert!(
        status.success(),
        "roostr-engine: `{compiler} {}` failed ({status})",
        args.join(" ")
    );
    check_exports(&library);

    let mut manifest = String::from("{\n  \"abiVersion\": 2,\n");
    let _ = writeln!(manifest, "  \"compiler\": {},", json_string(&version));
    manifest.push_str("  \"artifact\": \"libglon.a\",\n");
    let _ = writeln!(manifest, "  \"sourceFingerprint\": \"{fingerprint}\",");
    manifest.push_str("  \"sourceFiles\": {\n");
    for (index, (path, hash)) in source_files.iter().enumerate() {
        let comma = if index + 1 < source_files.len() {
            ","
        } else {
            ""
        };
        let _ = writeln!(manifest, "    {}: \"{hash}\"{comma}", json_string(path));
    }
    // The parity tests read glonOdin's fixtures from the same tree.
    println!("cargo:rustc-env=GLON_ODIN_ROOT={}", root.display());
    manifest.push_str("  }\n}\n");
    fs::write(out_dir.join("glon-core.json"), manifest).expect("write glon-core.json");

    println!("cargo:rustc-env=GLON_SOURCE_FINGERPRINT={fingerprint}");
    println!("cargo:rustc-env=GLON_COMPILER={version}");
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=glon");
}

/// `odin build -target:…` flags for the Cargo target, as build-xcframework.mjs
/// passes them per slice.
fn target_args() -> Vec<String> {
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let abi = env::var("CARGO_CFG_TARGET_ABI").unwrap_or_default();
    let odin_arch = match arch.as_str() {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => panic!("roostr-engine: no Odin target for architecture {other}"),
    };
    match os.as_str() {
        "macos" => vec![
            format!("-target:darwin_{odin_arch}"),
            format!(
                "-minimum-os-version:{}",
                env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_else(|_| "14.0".into())
            ),
        ],
        "ios" => vec![
            format!("-target:darwin_{odin_arch}"),
            if abi == "sim" || arch == "x86_64" {
                "-subtarget:iphonesimulator".into()
            } else {
                "-subtarget:iphone".into()
            },
            format!(
                "-minimum-os-version:{}",
                env::var("IPHONEOS_DEPLOYMENT_TARGET").unwrap_or_else(|_| "17.0".into())
            ),
        ],
        "linux" => vec![format!("-target:linux_{odin_arch}")],
        other => panic!("roostr-engine: no Odin target for OS {other}"),
    }
}

/// build-xcframework.mjs: sha256 of every non-test `.odin` file under `abi/`,
/// `core/`, `native/` (sorted per directory) plus the script itself, then
/// sha256 of that map's `JSON.stringify`. Equal to the iOS vendored
/// `glon-core.json` for the same sources.
fn source_fingerprint(root: &Path) -> (String, Vec<(String, String)>) {
    let mut files = Vec::new();
    for directory in ["abi", "core", "native"] {
        let dir = root.join(directory);
        println!("cargo:rerun-if-changed={}", dir.display());
        let mut names: Vec<String> = fs::read_dir(&dir)
            .unwrap_or_else(|error| panic!("roostr-engine: read {}: {error}", dir.display()))
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter(|name| name.ends_with(".odin") && !name.ends_with("_test.odin"))
            .collect();
        names.sort();
        for name in names {
            let path = dir.join(&name);
            println!("cargo:rerun-if-changed={}", path.display());
            files.push((format!("{directory}/{name}"), hash_file(&path)));
        }
    }
    let script = root.join("build-xcframework.mjs");
    println!("cargo:rerun-if-changed={}", script.display());
    files.push(("build-xcframework.mjs".into(), hash_file(&script)));

    let mut stringified = String::from("{");
    for (index, (path, hash)) in files.iter().enumerate() {
        if index > 0 {
            stringified.push(',');
        }
        let _ = write!(stringified, "{}:\"{hash}\"", json_string(path));
    }
    stringified.push('}');
    (hex(&Sha256::digest(stringified.as_bytes())), files)
}

fn hash_file(path: &Path) -> String {
    let bytes = fs::read(path)
        .unwrap_or_else(|error| panic!("roostr-engine: read {}: {error}", path.display()));
    hex(&Sha256::digest(&bytes))
}

fn odin_version(compiler: &str) -> String {
    let output = Command::new(compiler)
        .arg("version")
        .output()
        .unwrap_or_else(|error| missing_compiler(compiler, &error));
    assert!(
        output.status.success(),
        "roostr-engine: `{compiler} version` failed"
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn missing_compiler(compiler: &str, error: &std::io::Error) -> ! {
    if error.kind() == ErrorKind::NotFound {
        panic!(
            "roostr-engine: the Odin compiler `{compiler}` is not installed. The native engine is built from glonOdin with Odin (https://odin-lang.org/docs/install/); install it or set ODIN to its path."
        );
    }
    panic!("roostr-engine: cannot run `{compiler}`: {error}");
}

/// Fails the build early when the library lacks an ABI export, as
/// build-xcframework.mjs does (`nm -g`); skipped when `nm` is unavailable.
fn check_exports(library: &Path) {
    let Ok(output) = Command::new("nm").arg("-g").arg(library).output() else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let symbols = String::from_utf8_lossy(&output.stdout);
    for name in EXPORTS {
        let present = symbols.lines().any(|line| {
            line.contains(" T ")
                && (line.ends_with(&format!(" _{name}")) || line.ends_with(&format!(" {name}")))
        });
        assert!(
            present,
            "roostr-engine: {} lacks export {name}",
            library.display()
        );
    }
}

fn json_string(text: &str) -> String {
    serde_json::to_string(text).expect("string serializes")
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}
