//! Turns every `assets/icons/*.svg` into a `rune::Icon` static (`$OUT_DIR/icons.rs`): the
//! viewBox side and the body, with the root `<svg>`'s paint attributes moved onto a `<g>` so
//! Rune's icon rasterizer inherits them. Masks keep their luminance as `#fff`/`#000`, which
//! sends those icons through Rune's full-SVG (resvg) path, `currentColor` still tinted.

use std::fmt::Write as _;
use std::path::Path;

/// Paint attributes SVG inherits from the root element.
const INHERITED: [&str; 8] = [
    "fill",
    "stroke",
    "stroke-width",
    "stroke-linecap",
    "stroke-linejoin",
    "fill-rule",
    "fill-opacity",
    "stroke-opacity",
];

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = tag;
    loop {
        let i = rest.find(name)?;
        let before = rest[..i].chars().last();
        let after = &rest[i + name.len()..];
        if before.is_some_and(char::is_whitespace) && after.starts_with("=\"") {
            let v = &after[2..];
            return Some(&v[..v.find('"')?]);
        }
        rest = after;
    }
}

fn main() {
    let dir = Path::new("assets/icons");
    println!("cargo:rerun-if-changed=assets/icons");
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("assets/icons")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "svg"))
        .collect();
    files.sort();
    let mut out = String::new();
    for path in files {
        let name = path.file_stem().unwrap().to_str().unwrap().to_owned();
        let src = std::fs::read_to_string(&path).expect("icon svg");
        let open = src
            .find("<svg")
            .unwrap_or_else(|| panic!("{name}: no <svg>"));
        let open_end = open + src[open..].find('>').unwrap();
        let root = src[open..open_end].replace(['\n', '\r', '\t'], " ");
        let close = src
            .rfind("</svg>")
            .unwrap_or_else(|| panic!("{name}: no </svg>"));
        let inner = src[open_end + 1..close].trim();
        let view_box = attr(&root, "viewBox").unwrap_or_else(|| panic!("{name}: no viewBox"));
        let nums: Vec<f32> = view_box
            .split_whitespace()
            .map(|n| n.parse().unwrap())
            .collect();
        assert!(
            nums.len() == 4 && nums[0] == 0.0 && nums[1] == 0.0 && nums[2] == nums[3],
            "{name}: viewBox must be a square at the origin, is {view_box}"
        );
        let mut group = String::from("<g");
        for a in INHERITED {
            if let Some(v) = attr(&root, a) {
                write!(group, " {a}=\"{v}\"").unwrap();
            }
        }
        group.push('>');
        // Mask luminance as hex colours: Rune draws a body naming its own colours with resvg,
        // which applies masks; its coverage-only path would ignore them.
        let inner = if inner.contains("<mask") {
            inner
                .replace("fill=\"white\"", "fill=\"#fff\"")
                .replace("stroke=\"black\"", "stroke=\"#000\"")
        } else {
            inner.to_owned()
        };
        let inner: String = inner.split_whitespace().collect::<Vec<_>>().join(" ");
        let body = format!("{group}{inner}</g>");
        let ident = name.to_uppercase().replace('-', "_");
        writeln!(
            out,
            "pub(crate) static {ident}: rune::Icon = rune::Icon {{ name: {name:?}, view: {view:?}f32, body: {body:?} }};",
            view = nums[2]
        )
        .unwrap();
    }
    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("icons.rs");
    std::fs::write(dest, out).unwrap();
}
