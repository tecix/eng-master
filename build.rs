//! Compiles the GResource bundle (icons, stylesheet) and the translations in `po/`.

use std::path::Path;
use std::process::Command;

fn main() {
    glib_build_tools::compile_resources(
        &["data/resources"],
        "data/resources/resources.gresource.xml",
        "enmaster.gresource",
    );

    // Compile po/<lang>.po to OUT_DIR/<lang>.mo; `enmaster install` writes them to ~/.local/share/locale.
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let mut entries = Vec::new();
    println!("cargo:rerun-if-changed=po/LINGUAS");
    let linguas = std::fs::read_to_string("po/LINGUAS").unwrap_or_default();
    for lang in linguas.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let po = format!("po/{lang}.po");
        println!("cargo:rerun-if-changed={po}");
        let mo = Path::new(&out_dir).join(format!("{lang}.mo"));
        let ok = Command::new("msgfmt").args(["-o"]).arg(&mo).arg(&po).status().is_ok_and(|s| s.success());
        if ok {
            entries.push(format!("(\"{lang}\", include_bytes!(\"{}\") as &[u8])", mo.display()));
        } else {
            println!("cargo:warning=could not compile {po} (is msgfmt installed?)");
        }
    }
    std::fs::write(
        Path::new(&out_dir).join("translations.rs"),
        format!("pub const TRANSLATIONS: &[(&str, &[u8])] = &[{}];\n", entries.join(", ")),
    )
    .unwrap();
}
